//! Accepted ad-hoc queue dispatches use a target Action invocation, never an
//! invented pack-owned Queue binding or the queue creator's current IAM.
use super::{
    execution_runtime_admission::ExecutionAdmissionRepository, runtime_authority::RuntimeOwner,
};
use crate::{
    models::{
        ExecutionExecutableSnapshot, Id, WorkQueue, WorkQueueBatchMode, WorkQueueConfig,
        WorkQueueItem,
    },
    runtime_authority::{InvocationBinding, RequestAttribution},
    Error, Result,
};
use sqlx::PgConnection;

pub struct AdHocQueueAdmissionRepository;
impl AdHocQueueAdmissionRepository {
    /// Single dispatch retains its parent directly. Batch dispatch seals A's
    /// bounded accepted-item dependency manifest before consuming authority.
    pub fn validate_shape(queue: &WorkQueue) -> Result<()> {
        if !queue.is_adhoc {
            return Err(Error::validation("Expected an ad-hoc queue"));
        }
        let config: WorkQueueConfig = serde_json::from_value(queue.config.clone())?;
        if config.dispatch.as_ref().is_some_and(|dispatch| {
            [&dispatch.concurrency, &dispatch.batch_size]
                .into_iter()
                .flatten()
                .any(|tunable| tunable.source != crate::models::WorkQueueTunableSource::Literal)
        }) {
            return Err(Error::PermissionDenied(
                "Ad-hoc tunable input reads require their own accepted purpose binding".into(),
            ));
        }
        Ok(())
    }

    pub async fn accept(
        conn: &mut PgConnection,
        queue: &WorkQueue,
        item: &WorkQueueItem,
        parent: Id,
        snapshot: &ExecutionExecutableSnapshot,
    ) -> Result<()> {
        Self::validate_shape(queue)?;
        if item.queue != queue.id
            || item.executable_snapshot.as_ref() != Some(&serde_json::to_value(snapshot)?)
        {
            return Err(Error::validation("Ad-hoc accepted item target mismatch"));
        }
        let source_admission =
            ExecutionAdmissionRepository::require(conn, RuntimeOwner::Execution(parent)).await?;
        use super::FindById;
        let source = super::execution::ExecutionRepository::find_by_id(&mut *conn, parent)
            .await?
            .and_then(|execution| execution.executable_snapshot)
            .ok_or_else(|| {
                Error::PermissionDenied("Ad-hoc acceptance source executable unavailable".into())
            })?;
        crate::action_visibility::ensure_action_reference_allowed(
            &snapshot.executable.action,
            Some(&source.executable.action.pack_ref),
            "queue dispatch",
            &queue.r#ref,
        )?;
        let refs = queue.permission_set_refs.as_deref().unwrap_or(
            &snapshot
                .executable
                .action
                .default_execution_permission_set_refs,
        );
        let admission = ExecutionAdmissionRepository::select(
            conn,
            snapshot.executable.action.pack,
            InvocationBinding::Action {
                action_ref: snapshot.executable.action.r#ref.clone(),
            },
            snapshot,
            refs,
            RequestAttribution::ParentExecution {
                execution_id: parent,
            },
            super::runtime_target::RuntimeTarget::Component(
                super::runtime_target::ComponentTarget::Action,
                snapshot.executable.action.id,
            ),
        )
        .await?;
        let authority = admission.authority();
        let digest = crate::value_schema::digest(&serde_json::to_value(authority)?)
            .map_err(|_| Error::validation("Invalid ad-hoc item authority"))?;
        sqlx::query("INSERT INTO work_queue_item_runtime_admission(item,policy_revision,authority,authority_digest) VALUES ($1,$2,$3,$4)")
            .bind(item.id).bind(authority.policy_id).bind(sqlx::types::Json(authority)).bind(digest).execute(&mut *conn).await?;
        let template = serde_json::to_value(queue)?;
        if serde_json::to_vec(&template)?.len() > 1024 * 1024 {
            return Err(Error::validation(
                "Ad-hoc dispatch template exceeds its bound",
            ));
        }
        let digest = crate::value_schema::digest(&template)
            .map_err(|_| Error::validation("Invalid ad-hoc dispatch template"))?;
        let binding = super::schema_contract::SchemaContractRepository::materialize_action_pair(
            conn, snapshot,
        )
        .await?;
        let binding = serde_json::to_value(binding)?;
        if serde_json::to_vec(&binding)?.len() > 1024 * 1024 {
            return Err(Error::validation(
                "Accepted ad-hoc action schema exceeds its bound",
            ));
        }
        let schema_digest = crate::value_schema::digest(&binding)
            .map_err(|_| Error::validation("Invalid accepted action schema"))?;
        sqlx::query("INSERT INTO work_queue_item_ad_hoc_dispatch(item,source_admission,template,template_digest,schema_binding,schema_digest) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(item.id).bind(source_admission.id).bind(template).bind(digest).bind(binding).bind(schema_digest).execute(&mut *conn).await?;
        admission.ensure_fresh_on(conn).await
    }

    pub async fn template(
        conn: &mut PgConnection,
        queue: &WorkQueue,
        items: &[WorkQueueItem],
    ) -> Result<Option<WorkQueue>> {
        if !queue.is_adhoc {
            return Ok(None);
        }
        if items.is_empty() || items.len() > 256 {
            return Err(Error::validation(
                "Ad-hoc dispatch item set exceeds its bound",
            ));
        }
        let mut accepted = None;
        for item in items {
            let row: Option<(Id, serde_json::Value, String)> = sqlx::query_as("SELECT source_admission,template,template_digest FROM work_queue_item_ad_hoc_dispatch WHERE item = $1 FOR SHARE")
                .bind(item.id).fetch_optional(&mut *conn).await?;
            let Some((source_admission, body, expected)) = row else {
                if ExecutionAdmissionRepository::has_queue_item_admission(conn, items).await? {
                    return Err(Error::PermissionDenied(
                        "Accepted ad-hoc dispatch template unavailable".into(),
                    ));
                }
                return Ok(None);
            };
            let source =
                super::runtime_authority::RuntimeAuthorityRepository::require_retained_admission(
                    conn,
                    source_admission,
                )
                .await?;
            let attribution: serde_json::Value = sqlx::query_scalar("SELECT authority->'attribution' FROM work_queue_item_runtime_admission WHERE item = $1")
                .bind(item.id).fetch_one(&mut *conn).await?;
            if source.owner_kind != "execution"
                || attribution
                    != serde_json::to_value(RequestAttribution::ParentExecution {
                        execution_id: source.owner_id,
                    })?
            {
                return Err(Error::PermissionDenied(
                    "Ad-hoc dispatch source admission mismatch".into(),
                ));
            }
            if crate::value_schema::digest(&body)
                .map_err(|_| Error::validation("Invalid ad-hoc template"))?
                != expected
            {
                return Err(Error::validation(
                    "Ad-hoc dispatch template digest mismatch",
                ));
            }
            let pinned: WorkQueue = serde_json::from_value(body)?;
            Self::validate_shape(&pinned)?;
            if pinned.id != queue.id
                || pinned.r#ref != queue.r#ref
                || item.queue != queue.id
                || (pinned.batch_mode == WorkQueueBatchMode::Single && items.len() != 1)
            {
                return Err(Error::PermissionDenied(
                    "Ad-hoc dispatch source identity or shape changed".into(),
                ));
            }
            if let Some(first) = &accepted {
                if serde_json::to_value(first)? != serde_json::to_value(&pinned)? {
                    return Err(Error::PermissionDenied(
                        "Ad-hoc dispatch spans different accepted templates".into(),
                    ));
                }
            }
            accepted = Some(pinned);
        }
        Ok(accepted)
    }

    pub async fn source_effect_fence(
        conn: &mut PgConnection,
        item: Id,
    ) -> Result<super::runtime_authority::ApprovalEffectFence> {
        let source: Id = sqlx::query_scalar(
            "SELECT source_admission FROM work_queue_item_ad_hoc_dispatch WHERE item = $1 FOR SHARE",
        ).bind(item).fetch_one(&mut *conn).await?;
        super::runtime_authority::RuntimeAuthorityRepository::retained_admission_fence(conn, source)
            .await
    }

    pub async fn require_drained(conn: &mut PgConnection, queue: Id) -> Result<()> {
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM work_queue_item item JOIN work_queue_item_ad_hoc_dispatch accepted ON accepted.item = item.id WHERE item.queue = $1 AND item.status IN ('queued','leased','retry'))")
            .bind(queue).fetch_one(conn).await?;
        if pending {
            return Err(Error::validation(
                "Drain accepted ad-hoc queue items before changing dispatch behavior",
            ));
        }
        Ok(())
    }

    pub async fn schema(
        conn: &mut PgConnection,
        item: Id,
    ) -> Result<super::schema_contract::ActionSchemaBinding> {
        let (body, expected): (serde_json::Value, String) = sqlx::query_as("SELECT schema_binding,schema_digest FROM work_queue_item_ad_hoc_dispatch WHERE item = $1 FOR SHARE")
            .bind(item).fetch_one(conn).await?;
        if crate::value_schema::digest(&body)
            .map_err(|_| Error::validation("Invalid accepted schema"))?
            != expected
        {
            return Err(Error::validation("Accepted ad-hoc schema digest mismatch"));
        }
        let binding: super::schema_contract::ActionSchemaBinding = serde_json::from_value(body)?;
        binding.validate()?;
        Ok(binding)
    }

    pub async fn require_payload_binding(conn: &mut PgConnection, item: Id) -> Result<()> {
        let accepted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM work_queue_item_ad_hoc_dispatch WHERE item = $1)",
        )
        .bind(item)
        .fetch_one(conn)
        .await?;
        if !accepted {
            return Err(Error::PermissionDenied("This historical ad-hoc item has no accepted dispatch/input binding; payload replacement requires readmission".into()));
        }
        Ok(())
    }
}
