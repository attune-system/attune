//! Authentic retained recorded items for API consumer fixtures.
use crate::helpers::{Result, TestContext};
use attune_common::{
    repositories::{
        executable_snapshot::ExecutableSnapshotRepository,
        schema_contract::SchemaContractRepository, FindById,
    },
    runtime_authority::{InvocationBinding, RequestAttribution},
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Persist an authentic pre-rollout recorded item for consumer tests. New API
/// sponsorship is tested separately and stays disabled. The repository selector
/// validates the retained policy, executable, source target and approval fences;
/// only the historical item table write is fixture-owned (there is deliberately
/// no ungated production managed-item admission writer).
pub(crate) async fn pre_admitted_item(
    ctx: &TestContext,
    queue: &attune_common::models::WorkQueue,
    key: Option<&str>,
    payload: Value,
    source: attune_common::repositories::ad_hoc_queue_admission::AdHocQueueSource,
) -> Result<attune_common::models::WorkQueueItem> {
    use attune_common::repositories::{
        ad_hoc_queue_admission::{AdHocQueueAdmissionRepository, AdHocQueueSource},
        execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
        recorded_input_authority::RecordedInputAuthorityRepository as Inputs,
        runtime_target::{ComponentTarget, RuntimeTarget},
        work_queue::{CreateWorkQueueItemInput, WorkQueueItemRepository, WorkQueueRepository},
    };
    let mut tx = ctx.pool.begin().await?;
    let snapshot =
        ExecutableSnapshotRepository::resolve_for_action_ref(&mut *tx, &queue.dispatch_action_ref)
            .await?;
    let schema_source = WorkQueueRepository::item_schema_source(&mut tx, queue.id).await?;
    let contract = if queue.is_adhoc {
        let pack = attune_common::repositories::pack::PackRepository::find_by_id(
            &mut *tx,
            queue.pack.unwrap(),
        )
        .await?
        .unwrap();
        SchemaContractRepository::compile_owned_assignment(
            &mut tx,
            schema_source,
            attune_common::value_schema::SchemaContext::FlatDocument,
            &queue.item_schema,
            &pack,
        )
        .await?
    } else {
        SchemaContractRepository::materialize(
            &mut tx,
            schema_source,
            attune_common::value_schema::SchemaContext::FlatDocument,
            &queue.item_schema,
        )
        .await?
    };
    let binding = attune_common::value_schema::ValueSchemaBinding {
        authored_schema: queue.item_schema.clone(),
        contract,
    };
    attune_common::value_schema::CompiledContract::new(&binding.contract)?.validate(&payload)?;
    let (human, parent, actor) = match &source {
        AdHocQueueSource::Execution(id) => (None, Some(*id), None),
        AdHocQueueSource::Human(actor) => (Some(actor.identity_id), None, Some(actor.clone())),
    };
    let item = WorkQueueItemRepository::create_bound_pinned(
        &mut *tx,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: key.map(str::to_owned),
            priority: 0,
            status: attune_common::models::WorkQueueItemStatus::Queued,
            payload,
            metadata: json!({}),
            trace_tag: None,
            enqueue_source: "api".into(),
            requested_by_identity: human,
            requested_by_execution: parent,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
        &snapshot,
        &binding,
    )
    .await?;
    if queue.is_adhoc {
        AdHocQueueAdmissionRepository::accept(&mut tx, queue, &item, source, &snapshot).await?;
    } else {
        let selected = Admissions::select(
            &mut tx,
            queue.pack.unwrap(),
            InvocationBinding::Queue {
                queue_ref: queue.r#ref.clone(),
                action_ref: snapshot.executable.action.r#ref.clone(),
            },
            &snapshot,
            queue.permission_set_refs.as_deref().unwrap_or(&[]),
            RequestAttribution::QueueBatch {
                queue_ref: queue.r#ref.clone(),
                items: BTreeMap::from([(item.id, actor)]),
            },
            RuntimeTarget::Component(ComponentTarget::Queue, queue.id),
        )
        .await?;
        let authority = selected.authority();
        use sha2::Digest;
        let digest = hex::encode(sha2::Sha256::digest(serde_jcs::to_vec(authority)?));
        sqlx::query("INSERT INTO work_queue_item_runtime_admission(item,policy_revision,authority,authority_digest) VALUES($1,$2,$3,$4)")
            .bind(item.id).bind(authority.policy_id).bind(sqlx::types::Json(authority)).bind(digest).execute(&mut *tx).await?;
        selected.ensure_fresh_on(&mut tx).await?;
    }
    if let Some(parent) = parent {
        Inputs::copy(&mut tx, "execution", parent, "queue_item", item.id).await?;
    } else {
        Inputs::capture_human(&mut tx, "queue_item", item.id, human.unwrap()).await?;
    }
    tx.commit().await?;
    Ok(item)
}
