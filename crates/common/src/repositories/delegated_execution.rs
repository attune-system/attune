//! Explicit human-delegated execution authority and fenced runtime Key reads.
use super::{execution::ExecutionRepository, FindById};
use crate::{
    auth::jwt::{Claims, TokenType},
    delegation::DelegationAuthority,
    models::Id,
    secret_provenance::KeyOrigin,
    value_schema::digest,
    Error, Result,
};
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct ExecutionAuthoritySelection {
    pub execution: Id,
    pub mode: String,
    pub explicit: bool,
    pub owner: Option<Id>,
    pub selected_refs: Vec<String>,
    pub executable_snapshot: Option<serde_json::Value>,
    pub schema_digest: Option<String>,
    pub admission: Option<Id>,
    pub grant_digest: Option<String>,
    pub output_grants: Option<serde_json::Value>,
}
pub struct DelegatedExecutionRepository;

/// A native consumer receives controls, never the decrypted private fields.
pub enum ProtectedQueueAck {
    Unprotected,
    Invalid,
    Controls(crate::models::WorkQueueAck),
}

impl ProtectedQueueAck {
    fn validate(value: &serde_json::Value, items: &[Id], version: i32) -> Self {
        let Ok(Some(mut ack)) = crate::models::WorkQueueAck::from_execution_result(value) else {
            return Self::Invalid;
        };
        if ack.validate_for_items(items, version).is_err() {
            return Self::Invalid;
        }
        for item in &mut ack.items {
            item.summary = None;
            item.error = None;
        }
        Self::Controls(ack)
    }
}

impl DelegatedExecutionRepository {
    pub async fn find(
        conn: &mut PgConnection,
        execution: Id,
    ) -> Result<ExecutionAuthoritySelection> {
        sqlx::query_as("SELECT execution,mode,explicit,owner,selected_refs,executable_snapshot,schema_digest,admission,grant_digest,output_grants FROM execution_authority_selection WHERE execution=$1 FOR SHARE")
            .bind(execution).fetch_optional(conn).await?.ok_or_else(denied)
    }
    /// Only a newly created row in this transaction can be selected. Existing
    /// recorded work, including retries with lost admissions, cannot be converted.
    pub async fn select(conn: &mut PgConnection, execution: Id, source: Option<Id>) -> Result<()> {
        let row = ExecutionRepository::find_by_id(&mut *conn, execution)
            .await?
            .ok_or_else(denied)?;
        let owner = row.executor.ok_or_else(denied)?;
        let authority = DelegationAuthority::load_for_share(conn, owner).await?;
        authority
            .require_refs_for_share(conn, &row.permission_set_refs)
            .await?;
        if let Some(source) = source {
            let parent = Box::pin(Self::validate(conn, source)).await?;
            let source_row = ExecutionRepository::find_by_id(&mut *conn, source)
                .await?
                .ok_or_else(denied)?;
            if row.parent != Some(source)
                && row.original_execution != Some(source)
                && !(row.original_execution.is_some()
                    && row.original_execution == source_row.original_execution)
            {
                return Err(denied());
            }
            if row.original_execution.is_none() && row.parent == Some(source) {
                require_open_parent(conn, &parent).await?;
            }
            if row.original_execution.is_some()
                && (row.permission_set_refs != parent.selected_refs
                    || serde_json::to_value(&row.executable_snapshot)?
                        != serde_json::to_value(&source_row.executable_snapshot)?)
            {
                return Err(denied());
            }

            if parent.mode != "delegated" || parent.owner != Some(owner) {
                return Err(denied());
            }
            // Child selections must fit the parent's selected API authority.
            // The caller's current wider human grants cannot expand the ceiling.
            let parent_sets = super::identity::PermissionSetRepository::find_by_refs_for_share(
                conn,
                &crate::delegation::named_refs(&parent.selected_refs)?,
            )
            .await?;
            let grants = crate::delegation::grants_from_sets(parent_sets)?;
            let ceiling = DelegationAuthority::new(owner, Default::default(), grants);
            let child_sets = super::identity::PermissionSetRepository::find_by_refs_for_share(
                conn,
                &crate::delegation::named_refs(&row.permission_set_refs)?,
            )
            .await?;
            ceiling.require_grants(&crate::delegation::grants_from_sets(child_sets)?)?;
            if row.permission_set_refs.iter().any(|r| r == "standard")
                && !parent.selected_refs.iter().any(|r| r == "standard")
            {
                return Err(denied());
            }
        } else if row.parent.is_some() || row.original_execution.is_some() {
            return Err(denied());
        }
        let binding = super::schema_contract::SchemaContractRepository::find(
            conn,
            super::schema_contract::BindingOwner::Execution,
            execution,
        )
        .await?
        .ok_or_else(denied)?;
        let hash = digest(&serde_json::to_value(&binding)?).map_err(|_| denied())?;
        let selected_grants = Self::selected_grants(conn, &row.permission_set_refs).await?;
        let grant_digest =
            digest(&serde_json::to_value(&selected_grants)?).map_err(|_| denied())?;
        let mut output_grants = selected_grants;
        for grant in &mut output_grants {
            grant.actions.retain(|action| {
                matches!(
                    action,
                    crate::rbac::Action::Read
                        | crate::rbac::Action::Decrypt
                        | crate::rbac::Action::Use
                )
            });
        }
        output_grants.retain(|grant| !grant.actions.is_empty());
        if row.permission_set_refs.iter().any(|r| r == "standard") {
            let mut actions = vec![row.action_ref.clone()];
            if row.workflow_task.is_some() {
                if let Some(parent) = row.parent {
                    actions.push(
                        ExecutionRepository::find_by_id(&mut *conn, parent)
                            .await?
                            .ok_or_else(denied)?
                            .action_ref,
                    );
                }
            }
            actions.sort();
            actions.dedup();
            let mut packs = actions
                .iter()
                .filter_map(|a| a.split_once('.').map(|(p, _)| p.to_owned()))
                .collect::<Vec<_>>();
            packs.sort();
            packs.dedup();
            let mut owners = actions.clone();
            owners.extend(packs.iter().cloned());
            owners.sort();
            owners.dedup();
            use crate::rbac::{Action, Grant, GrantConstraints, Resource};
            for (resource, actions) in [
                (Resource::Keys, vec![Action::Read, Action::Decrypt]),
                (Resource::Artifacts, vec![Action::Read]),
            ] {
                output_grants.push(Grant {
                    resource,
                    actions,
                    constraints: Some(GrantConstraints {
                        owner_types: Some(vec![
                            crate::models::OwnerType::Pack,
                            crate::models::OwnerType::Action,
                        ]),
                        owner_refs: Some(owners.clone()),
                        ..Default::default()
                    }),
                });
            }
            // Standard cache access keeps pack/action selectors separate.
            // A combined owner-type/ref cross product would demand rights that
            // the API's scoped standard grants intentionally do not provide.
            for (owner_type, owner_refs) in [
                (crate::models::OwnerType::Pack, packs),
                (crate::models::OwnerType::Action, actions),
            ] {
                if !owner_refs.is_empty() {
                    output_grants.push(Grant {
                        resource: Resource::Caches,
                        actions: vec![Action::Read],
                        constraints: Some(GrantConstraints {
                            owner_types: Some(vec![owner_type]),
                            owner_refs: Some(owner_refs),
                            ..Default::default()
                        }),
                    });
                }
            }
        }
        let changed = sqlx::query("UPDATE execution_authority_selection s SET mode='delegated',explicit=TRUE,owner=$2,selected_refs=$3,executable_snapshot=$4,schema_digest=$5,grant_digest=$6,output_grants=$7 WHERE execution=$1 AND NOT s.explicit AND selection_transaction=pg_current_xact_id() AND admission IS NULL AND NOT EXISTS(SELECT 1 FROM runtime_authority_admission WHERE owner_kind='execution' AND owner_id=$1) AND EXISTS(SELECT 1 FROM execution WHERE id=$1 AND initial_input_transaction=pg_current_xact_id() AND worker IS NULL AND started_at IS NULL)")
            .bind(execution).bind(owner).bind(&row.permission_set_refs).bind(sqlx::types::Json(row.executable_snapshot.as_ref().ok_or_else(denied)?)).bind(hash).bind(grant_digest).bind(serde_json::to_value(output_grants)?).execute(&mut *conn).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        let snapshot = row.executable_snapshot.as_ref().ok_or_else(denied)?;
        if snapshot.executable.action.workflow_def.is_some()
            || snapshot.executable.workflow_definition.is_some()
        {
            let closed = matches!(
                row.status,
                crate::models::ExecutionStatus::Completed
                    | crate::models::ExecutionStatus::Failed
                    | crate::models::ExecutionStatus::Cancelled
                    | crate::models::ExecutionStatus::Timeout
                    | crate::models::ExecutionStatus::Abandoned
            );
            sqlx::query("INSERT INTO delegated_execution_attempt(execution,attempt,producer_kind,worker,closed) VALUES($1,$2,'workflow',NULL,$3)")
                .bind(execution).bind(Uuid::new_v4()).bind(closed).execute(conn).await?;
        }
        Ok(())
    }
    pub async fn validate(
        conn: &mut PgConnection,
        execution: Id,
    ) -> Result<ExecutionAuthoritySelection> {
        let selection = Self::find(conn, execution).await?;
        if selection.mode != "delegated" {
            return Err(denied());
        }
        if super::execution_runtime_admission::ExecutionAdmissionRepository::find(
            conn,
            super::runtime_authority::RuntimeOwner::Execution(execution),
        )
        .await?
        .is_some()
        {
            return Err(denied());
        }
        let row = ExecutionRepository::find_by_id(&mut *conn, execution)
            .await?
            .ok_or_else(denied)?;
        if row.executor != selection.owner
            || row.permission_set_refs != selection.selected_refs
            || serde_json::to_value(&row.executable_snapshot)?
                != serde_json::to_value(&selection.executable_snapshot)?
        {
            return Err(denied());
        }
        if selection.explicit {
            if Some(
                digest(&serde_json::to_value(
                    Self::selected_grants(conn, &selection.selected_refs).await?,
                )?)
                .map_err(|_| denied())?,
            ) != selection.grant_digest
            {
                return Err(denied());
            }

            let binding = super::schema_contract::SchemaContractRepository::find(
                conn,
                super::schema_contract::BindingOwner::Execution,
                execution,
            )
            .await?
            .ok_or_else(denied)?;
            if Some(digest(&serde_json::to_value(binding)?).map_err(|_| denied())?)
                != selection.schema_digest
            {
                return Err(denied());
            }
        }
        if let Some(owner) = selection.owner {
            DelegationAuthority::load_for_share(conn, owner)
                .await?
                .require_refs_for_share(conn, &selection.selected_refs)
                .await?;
        } else if selection.explicit || !selection.selected_refs.is_empty() {
            return Err(denied());
        }
        Ok(selection)
    }
    /// Callback/dataflow consumers retain the creation mode, owner and named-set
    /// ceiling, including after the child has completed.
    pub async fn validate_child(conn: &mut PgConnection, parent: Id, child: Id) -> Result<()> {
        let source = Box::pin(Self::validate(conn, parent)).await?;
        require_open_parent(conn, &source).await?;
        let selected = Box::pin(Self::validate(conn, child)).await?;
        let row = ExecutionRepository::find_by_id(&mut *conn, child)
            .await?
            .ok_or_else(denied)?;
        if row.parent != Some(parent) || source.owner != selected.owner {
            return Err(denied());
        }
        let ceiling = DelegationAuthority::new(
            source.owner.ok_or_else(denied)?,
            Default::default(),
            Self::selected_grants(conn, &source.selected_refs).await?,
        );
        ceiling.require_grants(&Self::selected_grants(conn, &selected.selected_refs).await?)?;
        if selected.selected_refs.iter().any(|r| r == "standard")
            && !source.selected_refs.iter().any(|r| r == "standard")
        {
            return Err(denied());
        }
        Ok(())
    }

    /// Native consumers may read protected terminal output only from the closed
    /// worker or native workflow producer. This never authorizes a token.
    pub async fn require_closed_output(
        conn: &mut PgConnection,
        execution: Id,
    ) -> Result<ExecutionAuthoritySelection> {
        let selection = Box::pin(Self::validate(conn, execution)).await?;
        let kind = selection_producer(&selection)?;
        let closed: Option<Id> = sqlx::query_scalar("SELECT e.id FROM execution e JOIN delegated_execution_attempt a ON a.execution=e.id WHERE e.id=$1 AND a.closed AND a.producer_kind=$2 AND ((a.producer_kind='worker' AND a.worker=e.worker) OR (a.producer_kind='workflow' AND a.worker IS NULL AND e.worker IS NULL)) AND e.status IN ('completed','failed','cancelled','timeout','abandoned') FOR SHARE OF e,a")
            .bind(execution).bind(kind).fetch_optional(conn).await?;
        if !selection.explicit || closed.is_none() {
            return Err(denied());
        }
        Ok(selection)
    }

    /// The caller holds queue lease locks and retains this transaction through
    /// the control effect. Private acknowledgement details never leave here.
    pub async fn protected_queue_ack_controls(
        conn: &mut PgConnection,
        execution: Id,
        items: &[Id],
        version: i32,
        encryption_key: Option<&str>,
    ) -> Result<ProtectedQueueAck> {
        use crate::secret_values::{restore_secret_values, ENTITY_EXECUTION_RESULT};
        let selected = Self::require_closed_output(conn, execution).await?;
        let mut secrets =
            super::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(
                &mut *conn,
                ENTITY_EXECUTION_RESULT,
                execution,
            )
            .await?;
        secrets.retain(|secret| {
            secret.json_path.is_empty()
                || secret.json_path == "/queue_ack"
                || secret.json_path.starts_with("/queue_ack/")
        });
        if secrets.is_empty() {
            return Ok(
                if selected.selected_refs.is_empty() || selection_producer(&selected)? == "workflow"
                {
                    ProtectedQueueAck::Unprotected
                } else {
                    // API-capable opaque output is protected even when provenance
                    // is missing; absence must never select public consumption.
                    ProtectedQueueAck::Invalid
                },
            );
        }
        crate::key_access::authorize_execution_control_origins(conn, execution, &secrets).await?;
        let source = ExecutionRepository::find_by_id(&mut *conn, execution)
            .await?
            .ok_or_else(denied)?;
        let Some(result) = source.result else {
            return Ok(ProtectedQueueAck::Invalid);
        };
        let key = encryption_key.ok_or_else(|| {
            Error::validation("Protected queue acknowledgement encryption is unavailable")
        })?;
        let root = secrets.iter().any(|secret| secret.json_path.is_empty());
        let value = if root {
            result
        } else {
            let Some(ack) = result.get("queue_ack") else {
                return Ok(ProtectedQueueAck::Invalid);
            };
            serde_json::json!({"queue_ack":ack})
        };
        let restored = restore_secret_values(value, &secrets, key)?;
        Ok(ProtectedQueueAck::validate(&restored, items, version))
    }

    pub async fn start(conn: &mut PgConnection, execution: Id, worker: Id) -> Result<Uuid> {
        let selection = Box::pin(Self::validate(conn, execution)).await?;
        if !selection.explicit || selection_producer(&selection)? != "worker" {
            return Err(denied());
        }
        let assigned:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution WHERE id=$1 AND worker=$2 AND status='scheduled')").bind(execution).bind(worker).fetch_one(&mut *conn).await?;
        if !assigned {
            return Err(denied());
        }
        let attempt = Uuid::new_v4();
        let value=sqlx::query_scalar("INSERT INTO delegated_execution_attempt(execution,attempt,worker) VALUES($1,$2,$3) ON CONFLICT(execution) DO NOTHING RETURNING attempt")
            .bind(execution).bind(attempt).bind(worker).fetch_optional(conn).await?.ok_or_else(denied)?;
        Ok(value)
    }
    pub async fn validate_claims(
        conn: &mut PgConnection,
        claims: &Claims,
    ) -> Result<Option<(Id, Uuid)>> {
        if claims.token_type != TokenType::Execution
            || claims
                .admitted_execution_credential()
                .map_err(|_| denied())?
                .is_some()
        {
            return Ok(None);
        }
        let meta = claims.metadata.as_ref().ok_or_else(denied)?;
        let execution = meta
            .get("execution_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(denied)?;
        let selection = Box::pin(Self::validate(conn, execution)).await?;
        let _: Id = sqlx::query_scalar("SELECT id FROM execution WHERE id=$1 FOR SHARE")
            .bind(execution)
            .fetch_one(&mut *conn)
            .await?;
        let row = ExecutionRepository::find_by_id(&mut *conn, execution)
            .await?
            .ok_or_else(denied)?;
        let refs: Vec<String> = serde_json::from_value(
            meta.get("permission_set_refs")
                .cloned()
                .ok_or_else(denied)?,
        )?;
        if row.status != crate::models::ExecutionStatus::Running
            || selection.owner != claims.sub.parse().ok()
            || meta.get("action_ref").and_then(|v| v.as_str()) != Some(row.action_ref.as_str())
            || refs != selection.selected_refs
            || claims.exp <= chrono::Utc::now().timestamp()
        {
            return Err(denied());
        }
        if !selection.explicit {
            return Err(denied());
        }
        let attempt: Uuid =
            serde_json::from_value(meta.get("delegated_attempt").cloned().ok_or_else(denied)?)?;
        let open:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM delegated_execution_attempt WHERE execution=$1 AND attempt=$2 AND producer_kind='worker' AND worker=$3 AND NOT closed)").bind(execution).bind(attempt).bind(row.worker).fetch_one(conn).await?;
        if !open {
            return Err(denied());
        }
        Ok(Some((execution, attempt)))
    }
    /// Retain a bounded disclosure's source attempt through its final commit.
    /// Closure waits for a read already authorized on this transaction.
    pub async fn retain_claims(conn: &mut PgConnection, claims: &Claims) -> Result<()> {
        let Some((execution, attempt)) = Box::pin(Self::validate_claims(conn, claims)).await?
        else {
            return Err(denied());
        };
        let retained:Option<Id>=sqlx::query_scalar("SELECT execution FROM delegated_execution_attempt WHERE execution=$1 AND attempt=$2 AND producer_kind='worker' AND NOT closed FOR SHARE")
            .bind(execution).bind(attempt).fetch_optional(conn).await?;
        if retained.is_none() {
            return Err(denied());
        }
        Ok(())
    }
    /// Persist the human rule's match-time choice; dispatch never selects from
    /// mutable rule ownership, defaults or permission refs again.
    pub async fn accept_enforcement(
        conn: &mut PgConnection,
        enforcement: &crate::models::Enforcement,
        owner: Id,
        refs: &[String],
    ) -> Result<()> {
        let authority = DelegationAuthority::load_for_share(conn, owner).await?;
        authority.require_refs_for_share(conn, refs).await?;
        let snapshot = enforcement
            .executable_snapshot
            .as_ref()
            .ok_or_else(denied)?;
        let binding = super::schema_contract::SchemaContractRepository::find(
            conn,
            super::schema_contract::BindingOwner::Enforcement,
            enforcement.id,
        )
        .await?
        .ok_or_else(denied)?;
        let schema_digest = digest(&serde_json::to_value(binding)?).map_err(|_| denied())?;
        let grant_digest = digest(&serde_json::to_value(
            Self::selected_grants(conn, refs).await?,
        )?)
        .map_err(|_| denied())?;
        let changed = sqlx::query("INSERT INTO delegated_enforcement_selection(enforcement,owner,selected_refs,grant_digest,executable_snapshot,schema_digest) SELECT id,$2,$3,$4,$5,$6 FROM enforcement WHERE id=$1 AND status='created' AND executable_snapshot=$5 AND NOT EXISTS(SELECT 1 FROM enforcement_recorded_authority WHERE enforcement=$1) AND NOT EXISTS(SELECT 1 FROM runtime_authority_admission WHERE owner_kind='enforcement' AND owner_id=$1)")
            .bind(enforcement.id).bind(owner).bind(refs).bind(grant_digest).bind(snapshot).bind(schema_digest).execute(conn).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        Ok(())
    }

    pub async fn enforcement_selection(
        conn: &mut PgConnection,
        enforcement: &crate::models::Enforcement,
    ) -> Result<(Id, Vec<String>)> {
        if super::execution_runtime_admission::ExecutionAdmissionRepository::enforcement_is_recorded(conn, enforcement.id).await? {
            return Err(denied());
        }
        let (owner, refs, grant_digest, snapshot, schema_digest): (Id, Vec<String>, String, serde_json::Value, String) =
            sqlx::query_as("SELECT owner,selected_refs,grant_digest,executable_snapshot,schema_digest FROM delegated_enforcement_selection WHERE enforcement=$1 FOR SHARE")
                .bind(enforcement.id).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        let binding = super::schema_contract::SchemaContractRepository::find(
            conn,
            super::schema_contract::BindingOwner::Enforcement,
            enforcement.id,
        )
        .await?
        .ok_or_else(denied)?;
        if enforcement.executable_snapshot.as_ref() != Some(&snapshot)
            || digest(&serde_json::to_value(binding)?).map_err(|_| denied())? != schema_digest
            || digest(&serde_json::to_value(
                Self::selected_grants(conn, &refs).await?,
            )?)
            .map_err(|_| denied())?
                != grant_digest
        {
            return Err(denied());
        }
        DelegationAuthority::load_for_share(conn, owner)
            .await?
            .require_refs_for_share(conn, &refs)
            .await?;
        Ok((owner, refs))
    }

    pub async fn accept_queue_item(
        conn: &mut PgConnection,
        queue: &crate::models::WorkQueue,
        item: &crate::models::WorkQueueItem,
        snapshot: &crate::models::ExecutionExecutableSnapshot,
    ) -> Result<()> {
        let owner = item.requested_by_identity.ok_or_else(denied)?;
        let refs = queue.permission_set_refs.as_ref().unwrap_or(
            &snapshot
                .executable
                .action
                .default_execution_permission_set_refs,
        );
        DelegationAuthority::load_for_share(conn, owner)
            .await?
            .require_refs_for_share(conn, refs)
            .await?;
        let grants = Self::selected_grants(conn, refs).await?;
        if let Some(source) = item.requested_by_execution {
            let parent = Box::pin(Self::validate(conn, source)).await?;
            if parent.owner != Some(owner)
                || refs.iter().any(|r| r == "standard")
                    && !parent.selected_refs.iter().any(|r| r == "standard")
            {
                return Err(denied());
            }
            let ceiling = DelegationAuthority::new(
                owner,
                Default::default(),
                Self::selected_grants(conn, &parent.selected_refs).await?,
            );
            ceiling.require_grants(&grants)?;
        }
        let hash = digest(&serde_json::to_value(grants)?).map_err(|_| denied())?;
        sqlx::query("INSERT INTO delegated_queue_item_selection(item,owner,selected_refs,grant_digest) VALUES($1,$2,$3,$4)").bind(item.id).bind(owner).bind(refs).bind(hash).execute(conn).await?;
        Ok(())
    }
    pub async fn queue_refs(
        conn: &mut PgConnection,
        items: &[crate::models::WorkQueueItem],
    ) -> Result<Vec<String>> {
        let mut retained = None;
        let mut owner = None;
        for item in items {
            let (identity,refs,hash):(Id,Vec<String>,String)=sqlx::query_as("SELECT owner,selected_refs,grant_digest FROM delegated_queue_item_selection WHERE item=$1 FOR SHARE").bind(item.id).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
            if item.requested_by_identity != Some(identity)
                || owner.is_some_and(|o| o != identity)
                || retained.as_ref().is_some_and(|r| r != &refs)
            {
                return Err(denied());
            }
            DelegationAuthority::load_for_share(conn, identity)
                .await?
                .require_refs_for_share(conn, &refs)
                .await?;
            if digest(&serde_json::to_value(
                Self::selected_grants(conn, &refs).await?,
            )?)
            .map_err(|_| denied())?
                != hash
            {
                return Err(denied());
            }
            owner = Some(identity);
            retained = Some(refs);
        }
        retained.ok_or_else(denied)
    }
    async fn selected_grants(
        conn: &mut PgConnection,
        refs: &[String],
    ) -> Result<Vec<crate::rbac::Grant>> {
        let refs = crate::delegation::named_refs(refs)?;
        let mut sets =
            super::identity::PermissionSetRepository::find_by_refs_for_share(conn, &refs).await?;
        if sets.len() != refs.len() {
            return Err(denied());
        }
        sets.sort_by(|a, b| a.r#ref.cmp(&b.r#ref));
        crate::delegation::grants_from_sets(sets)
    }
    /// Conservative provenance for opaque process output from API-capable code.
    /// It retains its selected read/approved-operation ceiling as well as exact runtime Key origins.
    pub async fn output_origin(
        conn: &mut PgConnection,
        execution: Id,
    ) -> Result<Option<crate::secret_provenance::SecretOrigin>> {
        let selected = Self::find(conn, execution).await?;
        if selected.mode != "delegated" || selected.selected_refs.is_empty() {
            return Ok(None);
        }
        let binding = OutputDisclosureBinding {
            execution,
            digest: digest(selected.output_grants.as_ref().ok_or_else(denied)?)
                .map_err(|_| denied())?,
        };
        Ok(Some(crate::secret_provenance::SecretOrigin::Local {
            source_kind: "delegated_execution_output".into(),
            source_ref: Some(serde_json::to_string(&binding)?),
        }))
    }
    pub async fn output_allowed(
        conn: &mut PgConnection,
        reference: Option<&str>,
        authority: &DelegationAuthority,
    ) -> Result<bool> {
        let binding: OutputDisclosureBinding = serde_json::from_str(reference.ok_or_else(denied)?)?;
        let selected = Self::find(conn, binding.execution).await?;
        if selected.mode != "delegated" || !selected.explicit {
            return Ok(false);
        }
        let grants = selected.output_grants.ok_or_else(denied)?;
        if digest(&grants).map_err(|_| denied())? != binding.digest {
            return Ok(false);
        }
        Ok(authority.covers(&serde_json::from_value::<Vec<crate::rbac::Grant>>(grants)?))
    }
    pub async fn require_native_log(conn: &mut PgConnection, execution: Id) -> Result<()> {
        Box::pin(Self::validate(conn, execution)).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workflow_execution WHERE execution=$1)",
        )
        .bind(execution)
        .fetch_one(conn)
        .await?;
        if !exists {
            return Err(denied());
        }
        Ok(())
    }
    /// Worker transport authorization is independent of the action's API refs.
    pub async fn require_completion(
        conn: &mut PgConnection,
        execution: Id,
        worker: Id,
    ) -> Result<()> {
        Box::pin(Self::validate(conn, execution)).await?;
        let allowed:Option<Id>=sqlx::query_scalar("SELECT e.id FROM execution e JOIN delegated_execution_attempt a ON a.execution=e.id AND a.worker=e.worker WHERE e.id=$1 AND e.worker=$2 AND a.producer_kind='worker' AND e.status='running' AND NOT a.closed FOR SHARE OF e,a")
            .bind(execution).bind(worker).fetch_optional(conn).await?;
        if allowed.is_none() {
            return Err(denied());
        }
        Ok(())
    }
    pub async fn current_attempt(conn: &mut PgConnection, execution: Id) -> Result<Uuid> {
        sqlx::query_scalar(
            "SELECT attempt FROM delegated_execution_attempt WHERE execution=$1 AND producer_kind='worker' AND NOT closed",
        )
        .bind(execution)
        .fetch_optional(conn)
        .await?
        .ok_or_else(denied)
    }
    pub async fn record_key(
        conn: &mut PgConnection,
        claims: &Claims,
        origin: &KeyOrigin,
    ) -> Result<()> {
        let Some((execution, attempt)) = Box::pin(Self::validate_claims(conn, claims)).await?
        else {
            return Ok(());
        };
        // Hold closure out with a shared attempt lock. Serialize the bounded
        // origin budget independently, avoiding concurrent readers upgrading
        // their retained shared attempt locks to exclusive locks.
        let open:Option<Id>=sqlx::query_scalar("SELECT execution FROM delegated_execution_attempt WHERE execution=$1 AND attempt=$2 AND producer_kind='worker' AND NOT closed FOR SHARE").bind(execution).bind(attempt).fetch_optional(&mut *conn).await?;
        if open.is_none() {
            return Err(denied());
        }
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('delegated_execution_key_origin'), hashtext($1::text))")
            .bind(execution).execute(&mut *conn).await?;
        let body = serde_json::to_value(origin)?;
        let hash = digest(&body).map_err(|_| denied())?;
        let (count,bytes):(i64,i64)=sqlx::query_as("SELECT count(*),COALESCE(sum(octet_length(origin::TEXT)),0)::BIGINT FROM delegated_execution_key_origin WHERE execution=$1").bind(execution).fetch_one(&mut *conn).await?;
        if count >= 1024 || bytes + serde_json::to_vec(&body)?.len() as i64 > 1024 * 1024 {
            return Err(denied());
        }
        sqlx::query("INSERT INTO delegated_execution_key_origin(execution,attempt,origin_digest,origin) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING").bind(execution).bind(attempt).bind(hash).bind(body).execute(conn).await?;
        Ok(())
    }
    pub async fn close(conn: &mut PgConnection, execution: Id) -> Result<()> {
        sqlx::query("UPDATE delegated_execution_attempt SET closed=TRUE WHERE execution=$1")
            .bind(execution)
            .execute(conn)
            .await?;
        Ok(())
    }
}
async fn require_open_parent(
    conn: &mut PgConnection,
    source: &ExecutionAuthoritySelection,
) -> Result<()> {
    let kind = selection_producer(source)?;
    let open: Option<Id> = sqlx::query_scalar("SELECT e.id FROM execution e JOIN delegated_execution_attempt a ON a.execution=e.id WHERE e.id=$1 AND e.status='running' AND NOT a.closed AND a.producer_kind=$2 AND ((a.producer_kind='worker' AND a.worker=e.worker) OR (a.producer_kind='workflow' AND a.worker IS NULL AND e.worker IS NULL)) FOR SHARE OF e,a")
        .bind(source.execution).bind(kind).fetch_optional(conn).await?;
    if open.is_none() {
        return Err(denied());
    }
    Ok(())
}

fn selection_producer(selection: &ExecutionAuthoritySelection) -> Result<&'static str> {
    let executable = selection
        .executable_snapshot
        .as_ref()
        .and_then(|value| value.get("executable"))
        .ok_or_else(denied)?;
    let action = executable.get("action").ok_or_else(denied)?;
    let binding = action.get("workflow_def").ok_or_else(denied)?;
    let definition = executable.get("workflow_definition").ok_or_else(denied)?;
    Ok(if !binding.is_null() || !definition.is_null() {
        "workflow"
    } else {
        "worker"
    })
}

fn denied() -> Error {
    Error::PermissionDenied(
        "Delegated execution authority is unavailable, stale or inconsistent".into(),
    )
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputDisclosureBinding {
    execution: Id,
    digest: String,
}

#[cfg(test)]
mod control_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn protected_ack_exposes_only_validated_protocol_controls() {
        let original = json!({"queue_ack":{"version":1,"items":[{"id":10,"status":"completed","summary":{"value":"private"},"error":{"value":"private"}}]}});
        let ProtectedQueueAck::Controls(ack) = ProtectedQueueAck::validate(&original, &[10], 1)
        else {
            panic!("valid controls rejected")
        };
        assert_eq!(ack.items[0].id, 10);
        assert_eq!(
            ack.items[0].status,
            crate::models::WorkQueueAckItemStatus::Completed
        );
        assert!(ack.items[0].summary.is_none() && ack.items[0].error.is_none());
        assert!(original.to_string().contains("private"));
    }

    #[test]
    fn protected_ack_parse_and_binding_errors_do_not_return_private_fields() {
        for ack in [
            json!({"version":1,"items":[{"id":10,"status":"private"}]}),
            json!({"version":1,"items":[{"id":987654321,"status":"completed"}]}),
            json!({"version":987654321,"items":[{"id":10,"status":"completed"}]}),
        ] {
            assert!(matches!(
                ProtectedQueueAck::validate(&json!({"queue_ack":ack}), &[10], 1),
                ProtectedQueueAck::Invalid
            ));
        }
    }
}
