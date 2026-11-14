#[path = "../../common/tests/helpers.rs"]
mod helpers;
#[path = "support/recorded_completion_cases.rs"]
mod recorded_completion_cases;

use std::collections::BTreeMap;

use attune_common::{
    models::{
        ActionReferenceVisibility, ExecutionStatus, OwnerType, WorkQueueBatchMode,
        WorkQueueItemStatus, WorkQueueUpdateStrategy,
    },
    repositories::{
        component_lifecycle::PackProjectionIds,
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        execution_admission::ExecutionAdmissionRepository as Admission,
        execution_secret_value::ExecutionSecretValueRepository,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
            PermissionAssignmentRepository, PermissionSetRepository, UpdateIdentityInput,
        },
        key::{CreateKeyInput, KeyRepository},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        recorded_input_authority::RecordedInputAuthorityRepository,
        rule_authority::RuleAuthorityRepository,
        runtime_authority::{RuntimeAuthorityRepository as Authority, RuntimeOwner},
        schema_contract::{BindingOwner, SchemaContractRepository},
        work_queue::{
            CreateWorkQueueInput, CreateWorkQueueItemInput, WorkQueueItemRepository,
            WorkQueueRepository,
        },
        Create, Delete, Update,
    },
    runtime_authority::{HumanAttribution, InvocationBinding, InvocationPolicy, RuntimePolicy},
    secret_provenance::SecretProvenance,
    secret_values::{prepare_secret_values, SecretValueInput, ENTITY_EXECUTION_CONFIG},
    test_database::TestDatabase,
};
use attune_worker::SecretManager;
use helpers::{create_test_pool, ActionFixture, IdentityFixture, PackFixture};
use serde_json::json;

const ENCRYPTION_KEY: &str = "recorded-input-worker-test-encryption-key";

struct Fixture {
    db: TestDatabase,
    execution: i64,
    admission: i64,
    sponsor: i64,
    humans: Vec<i64>,
    items: Vec<i64>,
    second_set: i64,
    key_ref: String,
}

async fn fixture(second_can_decrypt: bool) -> Fixture {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("worker_input")
        .create(&db)
        .await
        .unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "run")
        .with_param_schema(json!({"input":{"type":"string","secret":true}}))
        .create(&db)
        .await
        .unwrap();
    let queue = WorkQueueRepository::create(
        &db,
        CreateWorkQueueInput {
            r#ref: format!("{}.inbox", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            is_adhoc: false,
            label: "Input queue".into(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(action.id),
            dispatch_action_ref: action.r#ref.clone(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: WorkQueueUpdateStrategy::Immutable,
            batch_mode: WorkQueueBatchMode::Batch,
            item_schema: json!({}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: Some(vec!["standard".into()]),
            config: json!({}),
            reference_visibility: ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: Vec::new(),
        },
    )
    .await
    .unwrap();
    let key = KeyRepository::create_encrypted_in_pool(
        &db,
        CreateKeyInput {
            local_ref: format!("worker_input_{}", pack.id),
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Accepted input".into(),
            encrypted: true,
            encryption_key_hash: None,
            value: json!("accepted value"),
        },
        ENCRYPTION_KEY,
    )
    .await
    .unwrap();
    let origin = attune_common::key_access::key_origin(&key);
    let mut humans = Vec::new();
    let mut sets = Vec::new();
    for index in 0..2 {
        let human = IdentityFixture::new_unique("input_contributor")
            .create(&db)
            .await
            .unwrap();
        let actions = if index == 0 || second_can_decrypt {
            vec!["read", "decrypt"]
        } else {
            vec!["read"]
        };
        let set = PermissionSetRepository::create(&db, CreatePermissionSetInput {
            r#ref: format!("fixture.input_{}_{}", pack.id, index), pack: None, pack_ref: None, label: None, description: None,
            grants: json!([{"resource":"keys","actions":actions,"constraints":{"refs":[origin.key_ref]}}]),
        }).await.unwrap();
        PermissionAssignmentRepository::create(
            &db,
            CreatePermissionAssignmentInput {
                identity: human.id,
                permset: set.id,
            },
        )
        .await
        .unwrap();
        humans.push(human);
        sets.push(set.id);
    }
    let mut tx = db.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version,
            digest: "c".repeat(64),
            object_key: "packs/blobs/worker-input.tar.gz".into(),
            provider_version: "e:worker-input".into(),
            content_path: "/packs/.releases/worker-input".into(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        release.id,
        &PackProjectionIds {
            actions: vec![action.id],
            work_queues: vec![queue.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    RuleAuthorityRepository::capture_deployment(&mut tx, pack.id)
        .await
        .unwrap();
    let owner = Authority::find_owner(&mut tx, pack.id)
        .await
        .unwrap()
        .unwrap();
    let sponsor = owner.sponsor_identity_id;
    let policy = Authority::capture_policy(
        &mut tx,
        &RuntimePolicy {
            concrete_targets: BTreeMap::new(),
            owner,
            release_id: release.id,
            release_digest: release.digest.clone(),
            invocations: vec![InvocationPolicy {
                foreign_workflow_policy: None,
                metadata_revision: None,
                source_id: None,
                dispatch_template_digest: None,
                binding: InvocationBinding::Queue {
                    queue_ref: queue.r#ref.clone(),
                    action_ref: action.r#ref.clone(),
                },
                target_release_id: release.id,
                target_release_digest: release.digest,
                permission_sets: BTreeMap::new(),
                standard_access: true,
            }],
        },
    )
    .await
    .unwrap();
    Authority::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id)
        .await
        .unwrap();
    let mut items = Vec::new();
    for human in &humans {
        let item = WorkQueueItemRepository::create_pinned(
            &mut *tx,
            CreateWorkQueueItemInput {
                queue: queue.id,
                queue_ref: queue.r#ref.clone(),
                item_key: None,
                priority: 0,
                status: WorkQueueItemStatus::Queued,
                payload: json!({}),
                metadata: json!({}),
                trace_tag: None,
                enqueue_source: "worker-input-test".into(),
                requested_by_identity: Some(human.id),
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
        )
        .await
        .unwrap();
        // This consumer fixture represents an item accepted before rollout.
        // New managed-item admission stays gated. Select authentic policy/source
        // authority, then persist only the fixture-owned historical item row.
        let admitted = Admission::select(
            &mut tx,
            snapshot.executable.action.pack,
            InvocationBinding::Queue {
                queue_ref: queue.r#ref.clone(),
                action_ref: snapshot.executable.action.r#ref.clone(),
            },
            &snapshot,
            queue.permission_set_refs.as_ref().unwrap(),
            attune_common::runtime_authority::RequestAttribution::QueueBatch {
                queue_ref: queue.r#ref.clone(),
                items: BTreeMap::from([(
                    item.id,
                    Some(HumanAttribution {
                        identity_id: human.id,
                        login: human.login.clone(),
                    }),
                )]),
            },
            attune_common::repositories::runtime_target::RuntimeTarget::Component(
                attune_common::repositories::runtime_target::ComponentTarget::Queue,
                queue.id,
            ),
        )
        .await
        .unwrap();
        let authority = admitted.authority();
        use sha2::Digest;
        let digest = sha2::Sha256::digest(serde_jcs::to_vec(authority).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        sqlx::query("INSERT INTO work_queue_item_runtime_admission(item,policy_revision,authority,authority_digest) VALUES($1,$2,$3,$4)")
            .bind(item.id).bind(authority.policy_id).bind(sqlx::types::Json(authority))
            .bind(digest).execute(&mut *tx).await.unwrap();
        admitted.ensure_fresh_on(&mut tx).await.unwrap();
        RecordedInputAuthorityRepository::capture_human(&mut tx, "queue_item", item.id, human.id)
            .await
            .unwrap();
        items.push(item);
    }
    let admission = Admission::queue_batch(&mut tx, &queue, &items, &snapshot)
        .await
        .unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref,
            executor: None,
            permission_set_refs: vec!["standard".into()],
            status: ExecutionStatus::Running,
            config: Some(json!({"input":"***"})),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let record = admission
        .attach(&mut tx, RuntimeOwner::Execution(execution.id))
        .await
        .unwrap();
    let item_ids = items.iter().map(|item| item.id).collect::<Vec<_>>();
    RecordedInputAuthorityRepository::capture_queue_execution(&mut tx, execution.id, &item_ids)
        .await
        .unwrap();
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot)
        .await
        .unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
        .await
        .unwrap();
    let prepared = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/input".into(),
            value: json!("accepted value"),
            source_kind: "provenance".into(),
            source_ref: Some(
                serde_json::to_string(&SecretProvenance::key(origin.clone())).unwrap(),
            ),
        }],
        ENCRYPTION_KEY,
    )
    .unwrap();
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        ENTITY_EXECUTION_CONFIG,
        execution.id,
        &prepared,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    Fixture {
        db,
        execution: execution.id,
        admission: record.id,
        sponsor,
        humans: humans.into_iter().map(|human| human.id).collect(),
        items: item_ids,
        second_set: sets[1],
        key_ref: origin.key_ref,
    }
}

#[tokio::test]
async fn worker_restores_mixed_inputs_after_requester_and_queue_item_deletion() {
    let fixture = fixture(true).await;
    for item in &fixture.items {
        WorkQueueItemRepository::delete(&fixture.db, *item)
            .await
            .unwrap();
    }
    for human in &fixture.humans {
        IdentityRepository::delete(&fixture.db, *human)
            .await
            .unwrap();
    }
    let manager =
        SecretManager::new(fixture.db.pool().clone(), Some(ENCRYPTION_KEY.into())).unwrap();
    let restored = manager
        .restore_execution_parameters(fixture.execution, json!({"input":"***"}))
        .await
        .unwrap();
    assert_eq!(restored, json!({"input":"accepted value"}));
    IdentityRepository::update(
        &fixture.db,
        fixture.sponsor,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(manager
        .restore_execution_parameters(fixture.execution, json!({"input":"***"}))
        .await
        .is_err());
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn worker_does_not_union_contributor_grants_or_follow_later_iam_widening() {
    let fixture = fixture(false).await;
    let manager =
        SecretManager::new(fixture.db.pool().clone(), Some(ENCRYPTION_KEY.into())).unwrap();
    assert!(manager
        .restore_execution_parameters(fixture.execution, json!({"input":"***"}))
        .await
        .is_err());
    use attune_common::repositories::identity::UpdatePermissionSetInput;
    PermissionSetRepository::update(&fixture.db, fixture.second_set, UpdatePermissionSetInput {
        grants: Some(json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[fixture.key_ref]}}])),
        ..Default::default()
    }).await.unwrap();
    assert!(manager
        .restore_execution_parameters(fixture.execution, json!({"input":"***"}))
        .await
        .is_err());
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn worker_rechecks_terminal_and_explicit_admission_revocation() {
    let fixture = fixture(true).await;
    let manager =
        SecretManager::new(fixture.db.pool().clone(), Some(ENCRYPTION_KEY.into())).unwrap();
    let mut tx = fixture.db.begin().await.unwrap();
    Authority::revoke_admission(&mut tx, fixture.admission)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(manager
        .restore_execution_parameters(fixture.execution, json!({"input":"***"}))
        .await
        .is_err());
    ExecutionRepository::update(
        &fixture.db,
        fixture.execution,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(manager
        .restore_execution_parameters(fixture.execution, json!({"input":"***"}))
        .await
        .is_err());
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn existing_authority_validation_never_replays_admission_or_attempt_inserts() {
    let fixture = fixture(true).await;
    // Statement triggers also reject INSERT ... ON CONFLICT DO NOTHING. Counts
    // alone would miss the redundant creation work this path used to replay.
    sqlx::raw_sql(
        "CREATE FUNCTION reject_recreated_admission() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'existing validation attempted an admission insert'; END $$;
         CREATE TRIGGER reject_admission_insert BEFORE INSERT ON runtime_authority_admission
         FOR EACH STATEMENT EXECUTE FUNCTION reject_recreated_admission();
         CREATE TRIGGER reject_attempt_insert BEFORE INSERT ON runtime_authority_execution_attempt
         FOR EACH STATEMENT EXECUTE FUNCTION reject_recreated_admission();
         CREATE TRIGGER reject_fence_insert BEFORE INSERT ON runtime_authority_execution_fence
         FOR EACH STATEMENT EXECUTE FUNCTION reject_recreated_admission();",
    )
    .execute(&fixture.db)
    .await
    .unwrap();
    let mut tx = fixture.db.begin().await.unwrap();
    for _ in 0..2 {
        let record = attune_common::repositories::execution_admission::ExecutionAdmissionRepository::require(
            &mut tx,
            attune_common::repositories::runtime_authority::RuntimeOwner::Execution(fixture.execution),
        ).await.unwrap();
        assert_eq!(record.id, fixture.admission);
    }
    tx.rollback().await.unwrap();
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn missing_action_or_binding_without_admission_is_rejected_before_worker_claim() {
    use attune_common::{
        artifact_transport::VolumeTransport, auth::jwt::JwtConfig, log_stream::SegmentedLogConfig,
        repositories::FindById,
    };
    use attune_worker::{artifacts::ArtifactManager, runtime::RuntimeRegistry, ActionExecutor};
    let fixture = fixture(true).await;
    let source = ExecutionRepository::find_by_id(&fixture.db, fixture.execution)
        .await
        .unwrap()
        .unwrap();
    let snapshot = source.executable_snapshot.as_ref().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let artifacts = directory.path().join("artifacts");
    let executor = ActionExecutor::new(
        fixture.db.pool().clone(),
        RuntimeRegistry::new(),
        ArtifactManager::new(artifacts.clone()),
        SecretManager::new(fixture.db.pool().clone(), Some(ENCRYPTION_KEY.into())).unwrap(),
        1024,
        1024,
        false,
        false,
        "preparation-test".into(),
        uuid::Uuid::new_v4(),
        SegmentedLogConfig {
            initial_segment_bytes: 1024,
            max_segment_bytes: 1024,
            flush_interval_ms: 100,
            retry_max_attempts: 1,
            retry_attempt_timeout_ms: 100,
            retry_initial_backoff_ms: 1,
            retry_max_backoff_ms: 1,
            finalization_timeout_ms: 100,
        },
        None,
        None,
        directory.path().join("packs"),
        artifacts.clone(),
        directory.path().join("runtimes"),
        "http://127.0.0.1:1".into(),
        JwtConfig::default(),
        std::sync::Arc::new(VolumeTransport::new(artifacts.to_str().unwrap())),
    );
    let missing_action = ExecutionRepository::create(
        &fixture.db,
        CreateExecutionInput {
            action_ref: "missing.action".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let missing_binding = ExecutionRepository::create_pinned(
        &fixture.db,
        CreateExecutionInput {
            action: source.action,
            action_ref: source.action_ref,
            executor: source.executor,
            ..Default::default()
        },
        snapshot,
    )
    .await
    .unwrap();
    for execution in [missing_action, missing_binding] {
        // Neither incomplete row was admitted or assigned to this worker. The
        // production authority check must reject before claiming it, rather
        // than manufacturing authority to reach preparation-failure handling.
        let error = executor.execute(execution.id).await.unwrap_err();
        assert!(matches!(error, attune_common::Error::PermissionDenied(_)));
        let stored = ExecutionRepository::find_by_id(&fixture.db, execution.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, ExecutionStatus::Requested);
        assert!(stored.worker.is_none() && stored.started_at.is_none());
        assert!(stored.result.is_none());
        let attempts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM delegated_execution_attempt WHERE execution=$1",
        )
        .bind(execution.id)
        .fetch_one(&fixture.db)
        .await
        .unwrap();
        assert_eq!(attempts, 0);
        assert!(attune_common::repositories::artifact::ArtifactVersionRepository::find_file_versions_by_execution(
            &fixture.db, execution.id).await.unwrap().is_empty());
    }
    drop(executor);
    directory.close().unwrap();
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn delivery_mode_probe_never_downgrades_revoked_recorded_authority() {
    use attune_common::repositories::{worker_delivery::WorkerDeliveryRepository, FindById};
    let fixture = fixture(true).await;
    let execution = ExecutionRepository::find_by_id(&fixture.db, fixture.execution)
        .await
        .unwrap()
        .unwrap();
    let mut tx = fixture.db.begin().await.unwrap();
    Authority::revoke_admission(&mut tx, fixture.admission)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(attune_worker::completion::register(
        fixture.db.pool(),
        &execution,
        123,
        uuid::Uuid::new_v4(),
    )
    .await
    .is_err());
    let mut tx = fixture.db.begin().await.unwrap();
    assert!(
        WorkerDeliveryRepository::for_execution(&mut tx, execution.id)
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn delegated_registration_requires_live_owner_even_without_delivery() {
    use attune_common::repositories::{
        delegated_execution::DelegatedExecutionRepository as Delegated,
        worker_delivery::WorkerDeliveryRepository, FindById,
    };
    let fixture = fixture(true).await;
    let source = ExecutionRepository::find_by_id(&fixture.db, fixture.execution)
        .await
        .unwrap()
        .unwrap();
    let snapshot = source.executable_snapshot.as_ref().unwrap();
    let owner = fixture.humans[0];
    let mut tx = fixture.db.begin().await.unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: source.action,
            action_ref: source.action_ref.clone(),
            executor: Some(owner),
            ..Default::default()
        },
        snapshot,
    )
    .await
    .unwrap();
    let binding = SchemaContractRepository::materialize_action(&mut tx, snapshot)
        .await
        .unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
        .await
        .unwrap();
    Delegated::select(&mut tx, execution.id, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    IdentityRepository::update(
        &fixture.db,
        owner,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // The recorded-delivery no-op must still check live launch authority.
    assert!(attune_worker::completion::register(
        fixture.db.pool(),
        &execution,
        123,
        uuid::Uuid::new_v4(),
    )
    .await
    .is_err());
    let mut tx = fixture.db.begin().await.unwrap();
    assert!(Admission::existing_authority(&mut tx, execution.id)
        .await
        .is_err());
    assert!(
        WorkerDeliveryRepository::for_execution(&mut tx, execution.id)
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
    fixture.db.cleanup().await.unwrap();
}

// These launch-boundary cases execute a real POSIX shell child.
#[cfg(unix)]
#[path = "support/preparation_revocation.rs"]
mod preparation_revocation;
