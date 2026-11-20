//! Real independently accepted queue contributors archived by a native Wait.
use super::c_native_transport::{Boundary, Checkpoint};
use super::*;
use attune_common::{
    models::{Pack, WorkQueue, WorkQueueItemStatus},
    repositories::{
        accepted_batch_dependency::AcceptedBatchDependencyRepository as Batch,
        execution_admission::ExecutionAdmissionRepository as Admissions,
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
        },
        queue_configuration_input::QueueConfigurationInputRepository as Fixed,
        recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
        runtime_target::{ComponentTarget, RuntimeTarget},
        schema_contract::SchemaContractRepository,
        work_queue::{
            CreateWorkQueueInput, CreateWorkQueueItemInput, LeaseWorkQueueItemsInput,
            WorkQueueItemRepository, WorkQueueRepository,
        },
        FindById,
    },
};

async fn native(kind: usize, at: Checkpoint) -> Result<()> {
    match kind {
        0 => super::c_native_execution_wait::execution_wait(Some(at)).await,
        1 => {
            super::c_native_related_wait::related_wait(
                super::c_native_related_wait::Kind::Inquiry,
                true,
                Some(at),
            )
            .await
        }
        2 => {
            super::c_native_related_wait::related_wait(
                super::c_native_related_wait::Kind::Queue,
                true,
                Some(at),
            )
            .await
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn published_multi_contributor_native_wait_original_approval_expires_after_transport_waits(
) -> Result<()> {
    for kind in 0..3 {
        for boundary in [
            Boundary::Head,
            Boundary::Chunk,
            Boundary::Eof,
            Boundary::DownloadChunk,
            Boundary::DownloadEof,
            Boundary::Socket,
        ] {
            native(
                kind,
                Checkpoint::OriginalContributor {
                    boundary,
                    human: false,
                    weak: false,
                },
            )
            .await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn human_published_multi_contributor_native_wait_original_approval_expires_after_transport_waits(
) -> Result<()> {
    for kind in 0..3 {
        for boundary in [
            Boundary::Head,
            Boundary::Chunk,
            Boundary::Eof,
            Boundary::DownloadChunk,
            Boundary::DownloadEof,
            Boundary::Socket,
        ] {
            native(
                kind,
                Checkpoint::OriginalContributor {
                    boundary,
                    human: true,
                    weak: false,
                },
            )
            .await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn native_wait_cannot_publish_by_combining_independent_contributors_missing_decrypt(
) -> Result<()> {
    for kind in 0..3 {
        native(
            kind,
            Checkpoint::OriginalContributor {
                boundary: Boundary::Head,
                human: false,
                weak: true,
            },
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn queue(
    fixture: &Fixture,
    pack: &Pack,
    action: Id,
    action_ref: &str,
    permission_set_refs: &[String],
) -> Result<WorkQueue> {
    Ok(WorkQueueRepository::create(
        &fixture.ctx.pool,
        CreateWorkQueueInput {
            r#ref: format!(
                "recorded_emitter.contributors_{}",
                uuid::Uuid::new_v4().simple()
            ),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            is_adhoc: false,
            label: "Native contributors".into(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(action),
            dispatch_action_ref: action_ref.into(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: attune_common::models::WorkQueueUpdateStrategy::Immutable,
            batch_mode: attune_common::models::WorkQueueBatchMode::Batch,
            item_schema: json!({}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: Some(permission_set_refs.to_vec()),
            config: json!({}),
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: vec![],
        },
    )
    .await?)
}

pub(super) async fn capture(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    queue: &WorkQueue,
    execution: Id,
    key: Id,
    weak: bool,
    master: &str,
) -> Result<Vec<Id>> {
    let snapshot = ExecutionRepository::find_by_id(&mut **tx, execution)
        .await?
        .and_then(|execution| execution.executable_snapshot)
        .ok_or("contributor source executable is unavailable")?;
    let schema_source = WorkQueueRepository::item_schema_source(tx, queue.id).await?;
    let contract = SchemaContractRepository::materialize(
        tx,
        schema_source,
        attune_common::value_schema::SchemaContext::FlatDocument,
        &queue.item_schema,
    )
    .await?;
    let binding = attune_common::value_schema::ValueSchemaBinding {
        authored_schema: queue.item_schema.clone(),
        contract,
    };
    let mut items = Vec::new();
    for index in 0..2 {
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let identity = IdentityRepository::create(
            &mut **tx,
            CreateIdentityInput {
                login: format!("native_contributor_{nonce}"),
                display_name: None,
                password_hash: None,
                attributes: json!({}),
            },
        )
        .await?;
        let actions = if weak && index == 1 {
            json!(["read"])
        } else {
            json!(["read", "decrypt"])
        };
        let set = PermissionSetRepository::create(
            &mut **tx,
            CreatePermissionSetInput {
                r#ref: format!("fixture.contributor_{nonce}"),
                pack: None,
                pack_ref: None,
                label: None,
                description: None,
                grants: json!([{"resource":"keys","actions":actions,"constraints":{"ids":[key]}}]),
            },
        )
        .await?;
        PermissionAssignmentRepository::create(
            &mut **tx,
            CreatePermissionAssignmentInput {
                identity: identity.id,
                permset: set.id,
            },
        )
        .await?;
        let item = WorkQueueItemRepository::create_bound_pinned(
            &mut **tx,
            CreateWorkQueueItemInput {
                queue: queue.id,
                queue_ref: queue.r#ref.clone(),
                item_key: None,
                priority: 0,
                status: WorkQueueItemStatus::Queued,
                payload: json!({}),
                metadata: json!({}),
                trace_tag: None,
                enqueue_source: "native-contributor".into(),
                requested_by_identity: Some(identity.id),
                requested_by_execution: None,
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
        // Explicit admitted fixture contract, not a production gate bypass or
        // publication/E2E assertion. Metadata alone never supplies input proof.
        let selected = Admissions::select(
            tx,
            queue.pack.unwrap(),
            InvocationBinding::Queue {
                queue_ref: queue.r#ref.clone(),
                action_ref: snapshot.executable.action.r#ref.clone(),
            },
            &snapshot,
            queue.permission_set_refs.as_deref().unwrap_or(&[]),
            RequestAttribution::QueueBatch {
                queue_ref: queue.r#ref.clone(),
                items: BTreeMap::from([(
                    item.id,
                    Some(HumanAttribution {
                        identity_id: identity.id,
                        login: identity.login.clone(),
                    }),
                )]),
            },
            RuntimeTarget::Component(ComponentTarget::Queue, queue.id),
        )
        .await?;
        let authority = selected.authority();
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(serde_jcs::to_vec(authority)?)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        sqlx::query("INSERT INTO work_queue_item_runtime_admission(item,policy_revision,authority,authority_digest) VALUES($1,$2,$3,$4)")
            .bind(item.id).bind(authority.policy_id).bind(sqlx::types::Json(authority)).bind(digest)
            .execute(&mut **tx).await?;
        selected.ensure_fresh_on(tx).await?;
        Contributors::capture_human(tx, "queue_item", item.id, identity.id).await?;
        Fixed::capture(tx, queue, &item, &snapshot, master)
            .await?
            .ensure_fresh_on(tx)
            .await?;
        items.push(item.id);
    }
    Contributors::capture_queue_execution(tx, execution, &items).await?;
    let independent = Contributors::queue_authorities(tx, execution)
        .await?
        .unwrap();
    assert_eq!(independent.len(), 2);
    assert!(
        independent[0].identity_id != independent[1].identity_id,
        "contributors must retain distinct identities"
    );
    Ok(items)
}

pub(super) async fn seal_batch(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    queue: &WorkQueue,
    execution: Id,
    admission: Id,
    items: &[Id],
    master: &str,
) -> Result<()> {
    let lease = uuid::Uuid::new_v4();
    let leased = WorkQueueItemRepository::lease_next_batch(
        &mut **tx,
        LeaseWorkQueueItemsInput {
            separate_requesters: false,
            queue: queue.id,
            ready_statuses: vec![WorkQueueItemStatus::Queued],
            limit: items.len() as i64,
            batch_coalescing: None,
            leased_execution: None,
            lease_token: lease,
            lease_expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
        },
    )
    .await?;
    assert_eq!(leased.iter().map(|item| item.id).collect::<Vec<_>>(), items);
    WorkQueueItemRepository::attach_execution_to_lease(&mut **tx, lease, execution).await?;
    Batch::capture_managed(tx, admission, queue, &leased, None, master)
        .await?
        .ensure_fresh_on(tx)
        .await?;
    Ok(())
}

pub(super) async fn attribution(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    queue: &WorkQueue,
    ids: &[Id],
) -> Result<RequestAttribution> {
    let mut items = BTreeMap::new();
    for id in ids {
        let item = WorkQueueItemRepository::find_by_id(&mut **tx, *id)
            .await?
            .unwrap();
        assert_eq!(item.queue, queue.id);
        let identity =
            IdentityRepository::find_by_id(&mut **tx, item.requested_by_identity.unwrap())
                .await?
                .unwrap();
        items.insert(
            *id,
            Some(HumanAttribution {
                identity_id: identity.id,
                login: identity.login,
            }),
        );
    }
    Ok(RequestAttribution::QueueBatch {
        queue_ref: queue.r#ref.clone(),
        items,
    })
}
