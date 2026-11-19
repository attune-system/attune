//! Durable, bounded source accounting for typed Action and managed Queue batches.
use super::runtime_authority::{
    ApprovalEffectFence, RuntimeAdmissionRecord, RuntimeAuthorityRepository,
};
use crate::{
    models::{ExecutionExecutableSnapshot, Id, WorkQueue},
    runtime_authority::{InvocationBinding, RequestAttribution, SelectedRuntimeAuthority},
    value_schema::digest,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgConnection;
use std::collections::BTreeSet;

mod queue;
pub(in crate::repositories) use queue::QueueContributorSource;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemProof {
    source_admission: Option<Id>,
    authority: SelectedRuntimeAuthority,
    executable: ExecutionExecutableSnapshot,
    template: Value,
    schema: Value,
    contributors: Vec<(String, Value, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    queue_payload: Option<queue::QueuePayload>,
}

pub(in crate::repositories) struct BatchRetentionEvidence {
    pub source: Option<Id>,
    pub policy: Id,
    pub input: Option<Id>,
    pub bytes: usize,
    pub schema: Value,
    pub executable: Value,
}

pub struct AcceptedBatchDependencyRepository;
impl AcceptedBatchDependencyRepository {
    /// Call in creation after the Action-bound admission and ALL copied input
    /// contributors exist, before any publication. A newly staged batch cannot
    /// otherwise be consumed by canonical admission validation.
    pub async fn capture(
        conn: &mut PgConnection,
        destination: Id,
        items: &[Id],
    ) -> Result<ApprovalEffectFence> {
        super::schema_type::SchemaTypeRepository::lock_selection(conn).await?;
        let ids = canonical_ids(items)?;
        let record:RuntimeAdmissionRecord = sqlx::query_as("SELECT id,owner_kind,owner_id,policy_revision,authority,authority_digest,revoked_at FROM runtime_authority_admission WHERE id = $1")
            .bind(destination).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        if record.owner_kind != "execution"
            || record.revoked_at.is_some()
            || hash(&serde_json::to_value(&record.authority.0)?)? != record.authority_digest
        {
            return Err(denied());
        }
        let mut fence =
            RuntimeAuthorityRepository::selected_effect_fence(conn, &record.authority).await?;
        let RequestAttribution::QueueBatch {
            queue_ref,
            items: attribution,
        } = &record.authority.attribution
        else {
            return Err(denied());
        };
        if !matches!(record.authority.binding, InvocationBinding::Action { .. })
            || attribution.keys().copied().collect::<Vec<_>>() != ids
        {
            return Err(denied());
        }
        let mut proofs = Vec::new();
        let mut expected_contributors = Vec::new();
        let mut expected_slots = 0usize;
        let mut proof_bytes = 0usize;
        // Read immutable acceptance records, then validate all source policies
        // before taking item/target protection. B's creation transaction must
        // already hold the same source fence before its execution insert.
        for item in &ids {
            type AcceptedDispatchRow = (
                Option<Id>,
                Id,
                Value,
                String,
                Value,
                String,
                Value,
                String,
                Value,
            );
            let row: Option<AcceptedDispatchRow> = sqlx::query_as(
                "SELECT a.source_admission,r.policy_revision,r.authority,r.authority_digest,a.template,a.template_digest,a.schema_binding,a.schema_digest,i.executable_snapshot FROM work_queue_item_ad_hoc_dispatch a JOIN work_queue_item_runtime_admission r ON r.item = a.item JOIN work_queue_item i ON i.id = a.item WHERE a.item = $1 AND i.queue = (a.template->>'id')::BIGINT AND i.queue_ref = a.template->>'ref'")
                .bind(item).fetch_optional(&mut *conn).await?;
            let (
                source,
                policy,
                authority,
                authority_digest,
                template,
                template_digest,
                schema,
                schema_digest,
                executable,
            ) = row.ok_or_else(denied)?;
            if hash(&authority)? != authority_digest
                || hash(&template)? != template_digest
                || hash(&schema)? != schema_digest
            {
                return Err(denied());
            }
            let authority: SelectedRuntimeAuthority = serde_json::from_value(authority)?;
            if policy != authority.policy_id {
                return Err(denied());
            }
            if let Some(source) = source {
                let source_record =
                    RuntimeAuthorityRepository::require_retained_admission(conn, source).await?;
                if source_record.id >= record.id
                    || source_record.owner_kind != "execution"
                    || authority.attribution
                        != (RequestAttribution::ParentExecution {
                            execution_id: source_record.owner_id,
                        })
                {
                    return Err(denied());
                }
                fence = fence.intersect(
                    &RuntimeAuthorityRepository::retained_admission_fence(conn, source).await?,
                );
            } else if !matches!(authority.attribution, RequestAttribution::Human { .. }) {
                return Err(denied());
            }
            fence = fence.intersect(
                &RuntimeAuthorityRepository::selected_effect_fence(conn, &authority).await?,
            );
            let contributors:Vec<(String,Value,String)> = sqlx::query_as("SELECT contributor,authority,authority_digest FROM (SELECT ''::TEXT AS contributor,authority,authority_digest FROM recorded_execution_input_authority WHERE owner_kind = 'queue_item' AND owner_id = $1 UNION ALL SELECT contributor,authority,authority_digest FROM recorded_queue_item_input WHERE item = $1) contributors ORDER BY contributor")
                .bind(item).fetch_all(&mut *conn).await?;
            if contributors.len() > 256 {
                return Err(denied());
            }
            expected_slots += contributors.len().max(1);
            for (name, body, expected) in &contributors {
                if hash(body)? != *expected {
                    return Err(denied());
                }
                expected_contributors.push((*item, name.clone(), body.clone(), expected.clone()));
            }
            let proof = ItemProof {
                source_admission: source,
                authority,
                executable: serde_json::from_value(executable)?,
                template,
                schema,
                contributors,
                queue_payload: None,
            };
            proof_bytes += serde_json::to_vec(&proof)?.len();
            if proof_bytes > 1024 * 1024 || expected_slots > 256 {
                return Err(denied());
            }
            if proofs
                .first()
                .is_some_and(|(_, first): &(Id, ItemProof)| first.template != proof.template)
            {
                return Err(denied());
            }
            check_item(conn, &record, *item, &proof, queue_ref).await?;
            proofs.push((*item, proof));
        }
        if expected_contributors.len() > 256 {
            return Err(denied());
        }
        let copied:Vec<(Id,String,Value,String,i32)> = sqlx::query_as("SELECT item,contributor,authority,authority_digest,expected_items FROM recorded_queue_execution_input WHERE execution = $1 ORDER BY item,contributor")
            .bind(record.owner_id).fetch_all(&mut *conn).await?;
        if copied.len() != expected_contributors.len()
            || copied.iter().zip(&expected_contributors).any(
                |((item, name, body, hash, n), expected)| {
                    (*item, name.clone(), body.clone(), hash.clone()) != *expected
                        || usize::try_from(*n).ok() != Some(expected_slots)
                },
            )
        {
            return Err(denied());
        }
        let snapshot: Value =
            sqlx::query_scalar("SELECT executable_snapshot FROM execution WHERE id = $1 FOR SHARE")
                .bind(record.owner_id)
                .fetch_one(&mut *conn)
                .await?;
        let actual: ExecutionExecutableSnapshot = serde_json::from_value(snapshot)?;
        for (_, proof) in &proofs {
            if actual.release.id != proof.executable.release.id
                || serde_json::to_value(&actual.executable)?
                    != serde_json::to_value(&proof.executable.executable)?
            {
                return Err(denied());
            }
        }
        // Protect the exact item set after source validation. No item FK is
        // retained: later validation uses the copied immutable proof instead.
        let locked: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM work_queue_item WHERE id = ANY($1) ORDER BY id FOR SHARE",
        )
        .bind(&ids)
        .fetch_all(&mut *conn)
        .await?;
        if locked != ids {
            return Err(denied());
        }
        sqlx::query("INSERT INTO runtime_accepted_batch_manifest(admission,item_ids) VALUES ($1,$2) ON CONFLICT DO NOTHING")
            .bind(destination).bind(&ids).execute(&mut *conn).await?;
        for (item, proof) in proofs {
            let value = serde_json::to_value(&proof)?;
            sqlx::query("INSERT INTO runtime_accepted_batch_item(admission,item,source_admission,target_policy,proof,proof_digest) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
                .bind(destination).bind(item).bind(proof.source_admission).bind(proof.authority.policy_id).bind(&value).bind(hash(&value)?).execute(&mut *conn).await?;
            if let Some(source) = proof.source_admission {
                sqlx::query("INSERT INTO runtime_admission_dependency(admission,dependency,dependency_kind) VALUES ($1,$2,'accepted_item') ON CONFLICT DO NOTHING")
                    .bind(destination).bind(source).execute(&mut *conn).await?;
            }
        }
        // Canonical validation requires the sealed exact manifest and every
        // independent source. It also validates idempotent capture conflicts.
        RuntimeAuthorityRepository::record_admission(
            conn,
            super::runtime_authority::RuntimeOwner::Execution(record.owner_id),
            &record.authority,
        )
        .await?;
        fence = fence.intersect(
            &RuntimeAuthorityRepository::retained_admission_fence(conn, destination).await?,
        );
        fence.ensure_fresh_on(conn).await?;
        Ok(fence)
    }

    pub(in crate::repositories) async fn copy_inherited(
        conn: &mut PgConnection,
        source: &RuntimeAdmissionRecord,
        target: &RuntimeAdmissionRecord,
    ) -> Result<()> {
        if !matches!(
            source.authority.attribution,
            RequestAttribution::QueueBatch { .. }
        ) {
            return Ok(());
        }
        let mut expected = source.authority.0.clone();
        expected.attempt = target.authority.attempt;
        if source.id >= target.id || expected != target.authority.0 {
            return Err(denied());
        }
        let original:bool=sqlx::query_scalar("SELECT xmin::text::numeric=mod(pg_current_xact_id()::text::numeric,4294967296) FROM runtime_authority_admission WHERE id=$1")
            .bind(target.id).fetch_one(&mut *conn).await?;
        let present: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_accepted_batch_manifest WHERE admission=$1)",
        )
        .bind(target.id)
        .fetch_one(&mut *conn)
        .await?;
        if !original && !present {
            return Err(denied());
        }
        Self::validate(conn, source).await?;
        sqlx::query("INSERT INTO runtime_accepted_batch_manifest(admission,item_ids) SELECT $2,item_ids FROM runtime_accepted_batch_manifest WHERE admission=$1 ON CONFLICT DO NOTHING")
            .bind(source.id).bind(target.id).execute(&mut *conn).await?;
        sqlx::query("INSERT INTO runtime_accepted_batch_item(admission,item,source_admission,target_policy,proof,proof_digest,configuration_input) SELECT $2,item,source_admission,target_policy,proof,proof_digest,configuration_input FROM runtime_accepted_batch_item WHERE admission=$1 ON CONFLICT DO NOTHING")
            .bind(source.id).bind(target.id).execute(&mut *conn).await?;
        sqlx::query("INSERT INTO runtime_admission_dependency(admission,dependency,dependency_kind) SELECT $2,dependency,'accepted_item' FROM runtime_admission_dependency WHERE admission=$1 AND dependency_kind='accepted_item' ON CONFLICT DO NOTHING")
            .bind(source.id).bind(target.id).execute(&mut *conn).await?;
        Self::validate(conn, target).await?;
        let exact:bool=sqlx::query_scalar("SELECT NOT EXISTS((SELECT item,source_admission,target_policy,proof,proof_digest,configuration_input FROM runtime_accepted_batch_item WHERE admission=$1 EXCEPT SELECT item,source_admission,target_policy,proof,proof_digest,configuration_input FROM runtime_accepted_batch_item WHERE admission=$2) UNION ALL (SELECT item,source_admission,target_policy,proof,proof_digest,configuration_input FROM runtime_accepted_batch_item WHERE admission=$2 EXCEPT SELECT item,source_admission,target_policy,proof,proof_digest,configuration_input FROM runtime_accepted_batch_item WHERE admission=$1))")
            .bind(source.id).bind(target.id).fetch_one(conn).await?;
        if !exact {
            return Err(denied());
        }
        Ok(())
    }
    pub(in crate::repositories) fn authenticate_manifest(
        row: &Value,
        master: &str,
    ) -> Result<usize> {
        let body = row.get("proof").ok_or_else(denied)?;
        if row.get("proof_digest").and_then(Value::as_str) != Some(hash(body)?.as_str()) {
            return Err(denied());
        }
        let proof: ItemProof = serde_json::from_value(body.clone())?;
        if let Some(payload) = proof.queue_payload {
            if !matches!(proof.authority.binding, InvocationBinding::Queue { .. })
                || row.get("configuration_input").and_then(Value::as_i64)
                    != Some(payload.fixed_input)
            {
                return Err(denied());
            }
            for (_, body, digest) in &proof.contributors {
                super::recorded_input_authority::RecordedInputAuthorityRepository::from_manifest(
                    body, digest,
                )?;
            }
            payload.restore(master)?;
            Ok(payload.field_count())
        } else if matches!(proof.authority.binding, InvocationBinding::Action { .. })
            && row.get("configuration_input").is_none_or(Value::is_null)
        {
            Ok(0)
        } else {
            Err(denied())
        }
    }
    pub(in crate::repositories) async fn retention_evidence(
        conn: &mut PgConnection,
        admission: Id,
    ) -> Result<Vec<BatchRetentionEvidence>> {
        let rows:Vec<(Option<Id>,Id,Value,String,Option<Id>)>=sqlx::query_as("SELECT source_admission,target_policy,proof,proof_digest,configuration_input FROM runtime_accepted_batch_item WHERE admission=$1 ORDER BY item LIMIT 257")
            .bind(admission).fetch_all(conn).await?;
        if rows.len() > 256 {
            return Err(denied());
        }
        rows.into_iter()
            .map(|(source, policy, body, expected, input)| {
                if hash(&body)? != expected {
                    return Err(denied());
                }
                let bytes = serde_json::to_vec(&body)?.len();
                let proof: ItemProof = serde_json::from_value(body)?;
                if proof.source_admission != source
                    || proof.authority.policy_id != policy
                    || proof
                        .queue_payload
                        .as_ref()
                        .map(|payload| payload.fixed_input)
                        != input
                {
                    return Err(denied());
                }
                Ok(BatchRetentionEvidence {
                    source,
                    policy,
                    input,
                    bytes,
                    schema: proof.schema,
                    executable: serde_json::to_value(proof.executable)?,
                })
            })
            .collect()
    }
    pub async fn capture_managed(
        conn: &mut PgConnection,
        destination: Id,
        queue: &WorkQueue,
        items: &[crate::models::WorkQueueItem],
        dispatch: Option<Id>,
        master: &str,
    ) -> Result<ApprovalEffectFence> {
        queue::capture(conn, destination, queue, items, dispatch, master).await
    }
    pub(in crate::repositories) async fn contributor_source(
        conn: &mut PgConnection,
        renderer: &RuntimeAdmissionRecord,
        item: Id,
        master: &str,
    ) -> Result<QueueContributorSource> {
        queue::source(conn, renderer, item, master).await
    }

    pub(crate) async fn validate(
        conn: &mut PgConnection,
        record: &RuntimeAdmissionRecord,
    ) -> Result<Vec<SelectedRuntimeAuthority>> {
        let RequestAttribution::QueueBatch { queue_ref, items } = &record.authority.attribution
        else {
            return if matches!(record.authority.binding, InvocationBinding::Queue { .. }) {
                Err(denied())
            } else {
                Ok(vec![])
            };
        };
        if matches!(record.authority.binding, InvocationBinding::Queue { .. })
            && record.owner_kind != "execution"
        {
            return Err(denied());
        }
        if !matches!(
            record.authority.binding,
            InvocationBinding::Action { .. } | InvocationBinding::Queue { .. }
        ) {
            return Ok(vec![]);
        }
        let manifest: Option<Vec<Id>> = sqlx::query_scalar(
            "SELECT item_ids FROM runtime_accepted_batch_manifest WHERE admission = $1",
        )
        .bind(record.id)
        .fetch_optional(&mut *conn)
        .await?;
        let ids = manifest.ok_or_else(denied)?;
        if canonical_ids(&ids)? != items.keys().copied().collect::<Vec<_>>() {
            return Err(denied());
        }
        let rows:Vec<(Id,Option<Id>,Id,Value,String,Option<Id>)> = sqlx::query_as("SELECT item,source_admission,target_policy,proof,proof_digest,configuration_input FROM runtime_accepted_batch_item WHERE admission = $1 ORDER BY item")
            .bind(record.id).fetch_all(&mut *conn).await?;
        if rows.iter().map(|row| row.0).collect::<Vec<_>>() != ids {
            return Err(denied());
        }
        let mut selections = Vec::new();
        let mut sources = BTreeSet::new();
        let mut ordinals = BTreeSet::new();
        let mut queue_cohort = None;
        for (item, source, policy, body, expected, fixed) in rows {
            if hash(&body)? != expected {
                return Err(denied());
            }
            let proof: ItemProof = serde_json::from_value(body)?;
            if proof.source_admission != source || proof.authority.policy_id != policy {
                return Err(denied());
            }
            if let InvocationBinding::Queue { .. } = &record.authority.binding {
                let payload = proof.queue_payload.as_ref().ok_or_else(denied)?;
                if fixed != Some(payload.fixed_input) || !ordinals.insert(payload.ordinal) {
                    return Err(denied());
                }
                queue::check_item(conn, record, item, &proof, queue_ref).await?;
                let cohort=super::queue_configuration_input::QueueConfigurationInputRepository::cohort_identity(conn,payload.fixed_input).await?;
                if queue_cohort.as_ref().is_some_and(|first| first != &cohort) {
                    return Err(denied());
                }
                queue_cohort = Some(cohort);
            } else {
                if proof.queue_payload.is_some() || fixed.is_some() {
                    return Err(denied());
                }
                check_item(conn, record, item, &proof, queue_ref).await?;
            }
            if let Some(source) = source {
                sources.insert(source);
            }
            selections.push(proof.authority);
        }
        if matches!(record.authority.binding, InvocationBinding::Queue { .. })
            && ordinals.into_iter().collect::<Vec<_>>() != (0..ids.len()).collect::<Vec<_>>()
        {
            return Err(denied());
        }
        let edges:Vec<Id> = sqlx::query_scalar("SELECT dependency FROM runtime_admission_dependency WHERE admission = $1 AND dependency_kind = 'accepted_item' ORDER BY dependency")
            .bind(record.id).fetch_all(conn).await?;
        if edges.into_iter().collect::<BTreeSet<_>>() != sources {
            return Err(denied());
        }
        Ok(selections)
    }
}

async fn check_item(
    conn: &mut PgConnection,
    record: &RuntimeAdmissionRecord,
    item: Id,
    proof: &ItemProof,
    queue_ref: &str,
) -> Result<()> {
    let template: WorkQueue = serde_json::from_value(proof.template.clone())?;
    let schema: super::schema_contract::ActionSchemaBinding =
        serde_json::from_value(proof.schema.clone())?;
    schema.validate()?;
    let actor = if let Some(source_admission) = proof.source_admission {
        let source: Option<(String, Id)> = sqlx::query_as(
            "SELECT owner_kind,owner_id FROM runtime_authority_admission WHERE id = $1",
        )
        .bind(source_admission)
        .fetch_optional(&mut *conn)
        .await?;
        if source.as_ref().is_none_or(|(kind, id)| {
            kind != "execution"
                || proof.authority.attribution
                    != (RequestAttribution::ParentExecution { execution_id: *id })
        }) {
            return Err(denied());
        }
        None
    } else if let RequestAttribution::Human { actor } = &proof.authority.attribution {
        Some(actor.clone())
    } else {
        return Err(denied());
    };
    if !matches!(&record.authority.attribution, RequestAttribution::QueueBatch { items, .. } if items.get(&item) == Some(&actor))
    {
        return Err(denied());
    }
    if !template.is_adhoc
        || template.r#ref != queue_ref
        || template.dispatch_action != Some(proof.executable.executable.action.id)
        || !matches!(proof.authority.binding, InvocationBinding::Action { .. })
        || proof.authority.binding != record.authority.binding
        || proof.authority.owner != record.authority.owner
        || proof.authority.target_release_id != record.authority.target_release_id
        || proof.authority.target_release_digest != record.authority.target_release_digest
        || !record
            .authority
            .selected_refs
            .iter()
            .all(|reference| proof.authority.selected_refs.contains(reference))
    {
        return Err(denied());
    }
    let expected_source = crate::value_schema::SourceIdentity::Released {
        component_ref: proof.authority.binding.action_ref().into(),
        release_id: proof.executable.release.id,
        release_digest: proof.executable.release.digest.clone(),
    };
    if schema.input.source != expected_source {
        return Err(denied());
    }
    let policy = RuntimeAuthorityRepository::find_policy(conn, proof.authority.policy_id)
        .await?
        .ok_or_else(denied)?;
    proof.authority.validate_against(&policy.policy)?;
    let narrowed = policy.policy.select(
        policy.id,
        &proof.authority.binding,
        &record.authority.selected_refs,
        record.authority.attribution.clone(),
    )?;
    if narrowed.grants != record.authority.grants {
        return Err(denied());
    }
    let executable = super::managed_executable::ManagedExecutableRepository::invocation_snapshot(
        conn,
        policy
            .policy
            .invocations
            .iter()
            .find(|entry| entry.binding == proof.authority.binding)
            .ok_or_else(denied)?,
    )
    .await?;
    if executable.release.id != proof.executable.release.id
        || serde_json::to_value(&executable.executable)?
            != serde_json::to_value(&proof.executable.executable)?
    {
        return Err(denied());
    }
    Ok(())
}
fn canonical_ids(ids: &[Id]) -> Result<Vec<Id>> {
    let canonical = ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if canonical.is_empty()
        || canonical.len() > 256
        || canonical.len() != ids.len()
        || canonical[0] <= 0
    {
        return Err(denied());
    }
    Ok(canonical)
}
fn hash(value: &Value) -> Result<String> {
    digest(value).map_err(|_| denied())
}
fn denied() -> Error {
    Error::PermissionDenied(
        "Accepted batch source proof is missing, changed or outside its ceiling".into(),
    )
}
