#[path = "runtime_authority_repository_tests/c_inherited_receipt_consumers.rs"]
mod c_inherited_receipt_consumers;
#[path = "runtime_authority_repository_tests/cache_selector_composition.rs"]
mod cache_selector_composition;
#[path = "support/file_native_retention_cases.rs"]
mod file_native_retention_cases;
// The reused success fixture owns this executable's single database helper module.
use file_native_retention_cases::success_metadata_custody_cases::source::helpers;
#[path = "runtime_authority_repository_tests/native_cache_activation.rs"]
mod native_cache_activation;
#[path = "runtime_authority_repository_tests/native_cache_checkpoint.rs"]
mod native_cache_checkpoint;
#[path = "runtime_authority_repository_tests/native_group_transitions.rs"]
mod native_group_transitions;
#[path = "runtime_authority_repository_tests/native_iteration_materialization.rs"]
mod native_iteration_materialization;
#[path = "runtime_authority_repository_tests/native_iteration_outcomes.rs"]
mod native_iteration_outcomes;
#[path = "runtime_authority_repository_tests/native_result_views.rs"]
mod native_result_views;
#[path = "runtime_authority_repository_tests/native_retry_materialization.rs"]
mod native_retry_materialization;
#[path = "runtime_authority_repository_tests/native_wait_materialization.rs"]
mod native_wait_materialization;
#[path = "runtime_authority_repository_tests/pending_wait_stages.rs"]
mod pending_wait_stages;
#[path = "runtime_authority_repository_tests/requested_cache_publication.rs"]
mod requested_cache_publication;
#[path = "runtime_authority_repository_tests/requested_execution_publication.rs"]
mod requested_execution_publication;
#[path = "runtime_authority_repository_tests/staged_condition_decision.rs"]
mod staged_condition_decision;
#[path = "runtime_authority_repository_tests/staged_value_fixture.rs"]
mod staged_value_fixture;

use std::collections::BTreeMap;

use attune_common::{
    models::{ExecutionStatus, Id, PackRelease, WorkerStatus, WorkerType},
    rbac::{Action, AuthorizationContext, Resource},
    repositories::{
        component_lifecycle::PackProjectionIds,
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
            PermissionAssignmentRepository, PermissionSetRepository, UpdateIdentityInput,
        },
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        pack_retention::PackRetentionRepository,
        rule_authority::RuleAuthorityRepository,
        runtime::{CreateWorkerInput, WorkerRepository},
        runtime_authority::{
            ExternalApprovalRecord, RuntimeAdmissionRecord, RuntimeAuthorityRepository,
            RuntimeCredentialBinding, RuntimeOwner, RuntimePolicyRevision,
        },
        Create, Delete, FindById, Update,
    },
    runtime_authority::{
        HumanAttribution, InvocationBinding, InvocationPolicy, PermissionSetDefinition,
        RequestAttribution, RuntimePolicy,
    },
    test_database::TestDatabase,
};
use chrono::{Duration, Utc};
use helpers::{create_test_pool, ActionFixture, IdentityFixture, PackFixture};
use serde_json::json;

const NATIVE_CACHE_TEST_KEY: &str = "native-cache-snapshot-test-owned-encryption-key";
const NATIVE_WAIT_TEST_KEY: &str = "native-wait-test-owned-encryption-key";
const NATIVE_ITEM_TEST_KEY: &str = "native-item-test-owned-encryption-key";
const STAGED_VALUE_TEST_KEY: &str = "staged-value-test-owned-encryption-key";
const PURPOSE_RECEIPT_TEST_KEY: &str = "purpose-receipt-test-owned-encryption-key";
const SYSTEM_SNAPSHOT_TEST_KEY: &str = "system-test-owned-encryption-key";
const ACCEPTED_MANUAL_TEST_KEY: &str = "accepted-manual-test-owned-encryption-key";
const ACCEPTED_BATCH_TEST_KEY: &str = "accepted-batch-test-owned-encryption-key";
const PROOF_COLLECTION_TEST_KEY: &str = "bounded-proof-collection-test-owned-key";

fn staged_value_graph(leaf: &str) -> serde_json::Value {
    json!({"tasks":[{"name":"publish","action":leaf,"publish":{"first":"{{ parameters.supplied }}","second":"{{ first }}"}}],"output":{"token":"{{ second }}"}})
}
fn staged_value_out_schema() -> serde_json::Value {
    json!({"token":{"type":"string"}})
}
fn native_wait_response_options() -> Vec<attune_common::models::inquiry::InquiryResponseOption> {
    vec![attune_common::models::inquiry::InquiryResponseOption {
        r#ref: "answer".into(),
        label: "Answer".into(),
        style: attune_common::models::inquiry::InquiryResponseOptionStyle::Default,
        response: json!({"token":"inquiry-original"}),
    }]
}
#[test]
fn native_receipt_fixture_contracts_compile_and_positive_crypto_keys_are_valid() {
    use attune_common::value_schema::{compile_inline, SchemaContext};
    let graph = staged_value_graph("fixture.leaf");
    compile_inline(
        "fixture.staged",
        SchemaContext::FlatDocument,
        &staged_value_out_schema(),
    )
    .unwrap();
    assert_eq!(graph["output"]["token"], json!("{{ second }}"));
    compile_inline(
        "fixture.input",
        SchemaContext::FlatDocument,
        &json!({"supplied":{"type":"string"}}),
    )
    .unwrap();
    let response_schema = json!({"token":{"type":"string","secret":true}});
    attune_common::inquiry_options::validate_response_options(
        Some(&response_schema),
        &native_wait_response_options(),
    )
    .unwrap();
    assert!(
        attune_common::inquiry_options::validate_response_options(Some(&response_schema), &[])
            .is_err()
    );
    assert!(compile_inline(
        "fixture.invalid",
        SchemaContext::FlatDocument,
        &json!({"token":"{{ second }}"})
    )
    .is_err());
    assert!(
        compile_inline(
            "fixture.secret_default",
            SchemaContext::FlatDocument,
            &json!({"token":{"type":"string","secret":true,"default":"{{ second }}"}})
        )
        .is_err(),
        "defaults are prohibited beneath secret markers, including authored template strings"
    );
    for key in [
        NATIVE_CACHE_TEST_KEY,
        NATIVE_WAIT_TEST_KEY,
        NATIVE_ITEM_TEST_KEY,
        STAGED_VALUE_TEST_KEY,
        PURPOSE_RECEIPT_TEST_KEY,
        SYSTEM_SNAPSHOT_TEST_KEY,
        ACCEPTED_MANUAL_TEST_KEY,
        ACCEPTED_BATCH_TEST_KEY,
        PROOF_COLLECTION_TEST_KEY,
    ] {
        assert!(key.len() >= 32);
        attune_common::crypto::encrypt_json(&json!("fixture-key-validation"), key).unwrap();
    }
    assert!(attune_common::crypto::encrypt_json(
        &json!("negative-key-control"),
        "short-invalid-key"
    )
    .is_err());
}

fn fixture_digest(value: &impl serde::Serialize) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(serde_jcs::to_vec(value).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Fixture {
    external_pack_id: Id,
    external_trigger_id: Id,
    human_id: Id,
    approval: ExternalApprovalRecord,
    policy: RuntimePolicyRevision,
    admission: RuntimeAdmissionRecord,
    credential: RuntimeCredentialBinding,
}

async fn activate_release(
    connection: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    pack_id: Id,
    pack_ref: &str,
    actions: Vec<Id>,
    marker: char,
) -> PackRelease {
    activate_release_with_content(connection, pack_id, pack_ref, actions, marker, None).await
}

async fn activate_release_with_content(
    connection: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    pack_id: Id,
    pack_ref: &str,
    actions: Vec<Id>,
    marker: char,
    content_path: Option<String>,
) -> PackRelease {
    let digest = marker.to_string().repeat(64);
    let release = PackReleaseRepository::create_or_get(
        connection,
        CreatePackReleaseInput {
            pack: pack_id,
            pack_ref: pack_ref.into(),
            version: format!("1.0.{}", marker as u32),
            digest: digest.clone(),
            object_key: format!("packs/blobs/{digest}.tar.gz"),
            provider_version: format!("e:{digest}"),
            content_path: content_path.unwrap_or_else(|| format!("/packs/.releases/{digest}")),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(
        connection,
        pack_id,
        release.id,
        &PackProjectionIds {
            actions,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    release
}

fn owned_release_content(root: &std::path::Path, marker: char) -> String {
    let pack = root
        .join(".releases/sha256")
        .join(marker.to_string().repeat(64))
        .join("pack");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(pack.join("retention-fixture"), marker.to_string()).unwrap();
    pack.to_str().unwrap().to_owned()
}

async fn owned_queue(
    connection: &mut sqlx::PgConnection,
    owner: &attune_common::runtime_authority::PackAuthorityOwner,
    action_id: Id,
    action_ref: &str,
) -> attune_common::models::WorkQueue {
    queue_fixture(
        connection,
        owner,
        action_id,
        action_ref,
        false,
        attune_common::models::WorkQueueBatchMode::Single,
    )
    .await
}

async fn queue_fixture(
    connection: &mut sqlx::PgConnection,
    owner: &attune_common::runtime_authority::PackAuthorityOwner,
    action_id: Id,
    action_ref: &str,
    is_adhoc: bool,
    batch_mode: attune_common::models::WorkQueueBatchMode,
) -> attune_common::models::WorkQueue {
    attune_common::repositories::work_queue::WorkQueueRepository::create(
        connection,
        attune_common::repositories::work_queue::CreateWorkQueueInput {
            r#ref: format!("{}.jobs", owner.pack_ref),
            pack: Some(owner.pack_id),
            pack_ref: Some(owner.pack_ref.clone()),
            is_adhoc,
            label: "Jobs".into(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(action_id),
            dispatch_action_ref: action_ref.into(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: attune_common::models::WorkQueueUpdateStrategy::Immutable,
            batch_mode,
            item_schema: json!({}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            config: json!({}),
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: Vec::new(),
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn graph_writer_waits_before_pack_lock_so_recorded_creation_can_finish() {
    use attune_common::repositories::schema_type::SchemaTypeRepository;
    let (db, fixture) = fixture().await;
    let mut reader = db.begin().await.unwrap();
    SchemaTypeRepository::lock_selection(&mut reader)
        .await
        .unwrap();
    let pack = fixture.policy.policy.owner.pack_id;
    let pool = (*db).clone();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let writer = tokio::spawn(async move {
        let mut transaction = pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *transaction)
            .await
            .unwrap();
        started.send(pid).unwrap();
        let result = SchemaTypeRepository::lock_graph(&mut transaction, pack).await;
        transaction.rollback().await.unwrap();
        result
    });
    let pid = waiting.await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10),async {
        loop {
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid = $1 AND locktype = 'advisory' AND NOT granted)")
                .bind(pid).fetch_one(&*db).await.unwrap();
            if blocked {break;}
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let admitted = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        RuntimeAuthorityRepository::current_policy(&mut reader, pack),
    )
    .await;
    reader.rollback().await.unwrap();
    let written = tokio::time::timeout(std::time::Duration::from_secs(10), writer)
        .await
        .unwrap()
        .unwrap();
    db.cleanup().await.unwrap();
    assert!(admitted.unwrap().unwrap().is_some());
    written.unwrap();
}

#[tokio::test]
async fn accepted_queue_items_require_drain_before_template_or_policy_revision_changes() {
    use attune_common::models::WorkQueueItemStatus;
    use attune_common::repositories::work_queue::{
        CreateWorkQueueItemInput, UpdateWorkQueueInput, UpdateWorkQueueItemInput,
        WorkQueueItemRepository, WorkQueueRepository,
    };
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let snapshot = ExecutableSnapshotRepository::resolve_for_action_ref(
        &mut *tx,
        &fixture.credential.action_ref,
    )
    .await
    .unwrap();
    let queue = owned_queue(
        &mut tx,
        &fixture.policy.policy.owner,
        snapshot.executable.action.id,
        &fixture.credential.action_ref,
    )
    .await;
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
            enqueue_source: "test".into(),
            requested_by_identity: Some(fixture.human_id),
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
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let result = attune_common::repositories::managed_component_readmission::ManagedComponentReadmission::queue(&mut tx, queue.id, fixture.human_id).await;
    assert!(result
        .err()
        .unwrap()
        .to_string()
        .contains("Drain accepted queue items"));
    tx.rollback().await.unwrap();
    let result = WorkQueueRepository::update(
        &db,
        queue.id,
        UpdateWorkQueueInput {
            action_params: Some(json!({"message":"changed"})),
            ..Default::default()
        },
    )
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Drain accepted queue items"));
    let batch_change = WorkQueueRepository::update(
        &db,
        queue.id,
        UpdateWorkQueueInput {
            batch_mode: Some(attune_common::models::WorkQueueBatchMode::Batch),
            ..Default::default()
        },
    )
    .await;
    assert!(batch_change
        .unwrap_err()
        .to_string()
        .contains("Drain accepted queue items"));
    let mut next = fixture.policy.policy.0.clone();
    next.invocations[0].standard_access = false;
    let mut tx = db.begin().await.unwrap();
    let captured = RuntimeAuthorityRepository::capture_policy(&mut tx, &next)
        .await
        .unwrap();
    assert!(
        RuntimeAuthorityRepository::activate_policy(&mut tx, captured.id)
            .await
            .unwrap_err()
            .to_string()
            .contains("Drain accepted queue items")
    );
    tx.rollback().await.unwrap();
    WorkQueueItemRepository::update(
        &db,
        item.id,
        UpdateWorkQueueItemInput {
            status: Some(WorkQueueItemStatus::Cancelled),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    WorkQueueRepository::update(
        &db,
        queue.id,
        UpdateWorkQueueInput {
            action_params: Some(json!({"message":"changed"})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let captured = RuntimeAuthorityRepository::capture_policy(&mut tx, &next)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, captured.id)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn managed_policy_edit_readmits_future_scope_without_installer_runtime_coverage() {
    use attune_common::repositories::identity::UpdatePermissionSetInput;
    let (db, fixture) = fixture().await;
    let owner = &fixture.policy.policy.owner;
    let reference = fixture.policy.policy.invocations[0]
        .permission_sets
        .keys()
        .next()
        .unwrap()
        .clone();
    let managed = PermissionSetRepository::create(
        &db,
        CreatePermissionSetInput {
            r#ref: reference.clone(),
            pack: Some(owner.pack_id),
            pack_ref: Some(owner.pack_ref.clone()),
            label: None,
            description: None,
            grants: json!([{"resource":"events","actions":["create"]}]),
        },
    )
    .await
    .unwrap();
    let configure=PermissionSetRepository::create(&db,CreatePermissionSetInput {
        r#ref:"fixture.configure_policy".into(),pack:None,pack_ref:None,label:None,description:None,
        grants:json!([{"resource":"packs","actions":["configure"],"constraints":{"pack_refs":[owner.pack_ref]}}]),
    }).await.unwrap();
    PermissionAssignmentRepository::create(
        &db,
        CreatePermissionAssignmentInput {
            identity: fixture.human_id,
            permset: configure.id,
        },
    )
    .await
    .unwrap();
    let narrowed = helpers::TriggerFixture::new(
        Some(owner.pack_id),
        Some(owner.pack_ref.clone()),
        "narrowed",
    )
    .create(&db)
    .await
    .unwrap();
    let sibling =
        helpers::TriggerFixture::new(Some(owner.pack_id), Some(owner.pack_ref.clone()), "sibling")
            .create(&db)
            .await
            .unwrap();
    let mut tx = db.begin().await.unwrap();
    PermissionSetRepository::update(&mut *tx,managed.id,UpdatePermissionSetInput {
        label:None,description:None,grants:Some(json!([{"resource":"events","actions":["create"],"constraints":{"refs":[narrowed.r#ref]}}])),
    }).await.unwrap();
    let revisions =
        RuntimeAuthorityRepository::readmit_permission_set(&mut tx, &reference, fixture.human_id)
            .await
            .unwrap();
    assert_eq!(revisions.len(), 1);
    assert_ne!(revisions[0].id, fixture.policy.id);
    assert_eq!(
        revisions[0].policy.invocations[0].permission_sets[&reference]
            .grants
            .len(),
        1
    );
    let admitted =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await
            .unwrap();
    let sibling = attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve(
        &mut tx,
        attune_common::repositories::runtime_target::RuntimeTarget::EventTrigger(sibling.id),
        owner.sponsor_identity_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(admitted.allows_new_effect(&sibling, Action::Create));
    assert_eq!(admitted.authority().policy_id, fixture.policy.id);
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn concrete_effect_targets_and_fixed_approvals_never_follow_ref_reuse() {
    use attune_common::repositories::{
        runtime_target::{RuntimeTarget, RuntimeTargetRepository},
        trigger::TriggerRepository,
    };
    use attune_common::runtime_authority::AdmittedGrant;
    let (db, fixture) = fixture().await;
    TriggerRepository::delete(&db, fixture.external_trigger_id)
        .await
        .unwrap();
    let replacement = helpers::TriggerFixture::new(
        Some(fixture.external_pack_id),
        Some("shared".into()),
        "completed",
    )
    .create(&db)
    .await
    .unwrap();
    assert_ne!(replacement.id, fixture.external_trigger_id);
    let mut tx = db.begin().await.unwrap();
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await
            .unwrap();
    let target = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::EventTrigger(replacement.id),
        fixture.credential.sponsor_identity_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!authority.allows_new_effect(&target, Action::Create));
    let mut readmission = fixture.policy.policy.0.clone();
    readmission.concrete_targets.clear();
    assert!(
        RuntimeAuthorityRepository::capture_policy(&mut tx, &readmission)
            .await
            .is_err()
    );
    let approved = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        readmission.owner.pack_id,
        fixture.human_id,
        fixture.approval.grants.0.clone(),
        None,
    )
    .await
    .unwrap();
    for invocation in &mut readmission.invocations {
        for set in invocation.permission_sets.values_mut() {
            for grant in &mut set.grants {
                if let AdmittedGrant::External { approval_id, .. } = grant {
                    *approval_id = approved.id;
                }
            }
        }
    }
    let new_policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &readmission)
        .await
        .unwrap();
    assert_ne!(new_policy.id, fixture.policy.id);
    assert_ne!(
        new_policy.policy.concrete_targets,
        fixture.policy.policy.concrete_targets
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn managed_action_edit_creates_metadata_revision_without_rewriting_code_or_old_admission() {
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        managed_executable::ManagedExecutableRepository,
    };
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let binding = &fixture.admission.authority.binding;
    let original =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &fixture.policy.policy, binding)
            .await
            .unwrap();
    let original_id = original.executable.action.id;
    ActionRepository::update(
        &mut *tx,
        original_id,
        UpdateActionInput {
            entrypoint: Some("edited.py".into()),
            default_execution_permission_set_refs: Some(vec![]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let revised =
        ManagedExecutableRepository::readmit_action(&mut tx, original_id, fixture.human_id)
            .await
            .unwrap()
            .unwrap();
    assert_ne!(revised.id, fixture.policy.id);
    assert!(revised.policy.invocations[0].metadata_revision.is_some());
    let edited = ManagedExecutableRepository::resolve_invocation(&mut tx, &revised.policy, binding)
        .await
        .unwrap();
    assert_eq!(edited.executable.action.entrypoint, "edited.py");
    assert_eq!(edited.release.id, original.release.id);
    assert_eq!(edited.release.digest, original.release.digest);
    assert!(revised.policy.invocations[0].permission_sets.is_empty());
    let live = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, original_id)
        .await
        .unwrap();
    assert_eq!(live.executable.action.entrypoint, "edited.py");
    let old =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &fixture.policy.policy, binding)
            .await
            .unwrap();
    assert_eq!(
        old.executable.action.entrypoint,
        original.executable.action.entrypoint
    );
    let immutable =
        ExecutableSnapshotRepository::resolve_release_actions(&mut tx, original.release.id)
            .await
            .unwrap();
    assert_eq!(
        immutable[&fixture.credential.action_ref]
            .executable
            .action
            .entrypoint,
        original.executable.action.entrypoint
    );
    let admitted =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await
            .unwrap();
    assert_eq!(admitted.authority().policy_id, fixture.policy.id);
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn managed_queue_readmission_retains_old_authority_and_rejects_foreign_broadening() {
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        managed_component_readmission::ManagedComponentReadmission,
        managed_executable::ManagedExecutableRepository,
        schema_type::SchemaTypeRepository,
        work_queue::{UpdateWorkQueueInput, WorkQueueRepository},
        Patch,
    };
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let original = ManagedExecutableRepository::resolve_invocation(
        &mut tx,
        &fixture.policy.policy,
        &fixture.admission.authority.binding,
    )
    .await
    .unwrap();
    let queue = owned_queue(
        &mut tx,
        &fixture.policy.policy.owner,
        original.executable.action.id,
        &fixture.credential.action_ref,
    )
    .await;
    let admitted = ManagedComponentReadmission::queue(&mut tx, queue.id, fixture.human_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(admitted.policy.release_id, fixture.policy.policy.release_id);
    let old_action = admitted
        .policy
        .invocations
        .iter()
        .find(|entry| entry.binding == fixture.admission.authority.binding)
        .unwrap();
    assert_eq!(
        serde_json::to_value(old_action).unwrap(),
        serde_json::to_value(&fixture.policy.policy.invocations[0]).unwrap()
    );
    WorkQueueRepository::update(
        &mut *tx,
        queue.id,
        UpdateWorkQueueInput {
            action_params: Some(json!({"literal": "changed"})),
            permission_set_refs: Some(Patch::Set(vec!["standard".into()])),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let revised = ManagedComponentReadmission::queue(&mut tx, queue.id, fixture.human_id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(admitted.id, revised.id);
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM managed_dispatch_metadata_revision WHERE component_id = $1",
    )
    .bind(queue.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(rows, 2);
    let old = ManagedExecutableRepository::resolve_invocation(
        &mut tx,
        &fixture.policy.policy,
        &fixture.admission.authority.binding,
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(old).unwrap(),
        serde_json::to_value(&original).unwrap()
    );
    RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let foreign = PermissionSetRepository::create(
        &db,
        CreatePermissionSetInput {
            r#ref: "foreign.unapproved".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"keys","actions":["read"]}]),
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    WorkQueueRepository::update(
        &mut *tx,
        queue.id,
        UpdateWorkQueueInput {
            permission_set_refs: Some(Patch::Set(vec![foreign.r#ref])),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        ManagedComponentReadmission::queue(&mut tx, queue.id, fixture.human_id)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let foreign_action = ActionFixture::new(fixture.external_pack_id, "shared", "private_target")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    ActionRepository::update(
        &mut *tx,
        foreign_action.id,
        UpdateActionInput {
            reference_visibility: Some(attune_common::models::ActionReferenceVisibility::Private),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    WorkQueueRepository::update(
        &mut *tx,
        queue.id,
        UpdateWorkQueueInput {
            dispatch_action: Some(Patch::Set(foreign_action.id)),
            dispatch_action_ref: Some(foreign_action.r#ref),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let error = ManagedComponentReadmission::queue(&mut tx, queue.id, fixture.human_id)
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("private"), "{error}");
    assert!(error.contains("api/queues"), "{error}");
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn managed_workflow_graph_readmission_pins_exact_dynamic_targets_and_old_schema() {
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        managed_component_readmission::ManagedComponentReadmission,
        managed_executable::ManagedExecutableRepository,
        schema_type::SchemaTypeRepository,
        workflow::{CreateWorkflowDefinitionInput, WorkflowDefinitionRepository},
    };
    let (db, fixture) = fixture().await;
    let leaf = ActionFixture::new(fixture.external_pack_id, "shared", "leaf")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    activate_release(
        &mut tx,
        fixture.external_pack_id,
        "shared",
        vec![leaf.id],
        '9',
    )
    .await;
    let original = ManagedExecutableRepository::resolve_invocation(
        &mut tx,
        &fixture.policy.policy,
        &fixture.admission.authority.binding,
    )
    .await
    .unwrap();
    let workflow = WorkflowDefinitionRepository::create(&mut *tx, CreateWorkflowDefinitionInput {
        r#ref: fixture.credential.action_ref.clone(), pack: fixture.policy.policy.owner.pack_id,
        pack_ref: fixture.policy.policy.owner.pack_ref.clone(), label: "Managed workflow".into(), description: None, version: "1.0".into(),
        param_schema: Some(json!({"target": {"type":"string"}})), out_schema: None, tags: vec![],
        definition: json!({"tasks":[{"name":"choose","action":"{{ parameters.target }}","allowed_action_refs":[leaf.r#ref],"permission_set_refs":[],"retry":{"count":2}}]}),
    }).await.unwrap();
    ActionRepository::link_workflow_def(&mut *tx, original.executable.action.id, workflow.id)
        .await
        .unwrap();
    ActionRepository::update(
        &mut *tx,
        original.executable.action.id,
        UpdateActionInput {
            param_schema: Some(json!({"target":{"type":"string"}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let revised = ManagedComponentReadmission::workflow(
        &mut tx,
        original.executable.action.id,
        fixture.human_id,
    )
    .await
    .unwrap()
    .unwrap();
    let binding = InvocationBinding::WorkflowTask {
        workflow_ref: fixture.credential.action_ref.clone(),
        task_path: "/tasks/0".into(),
        action_ref: leaf.r#ref.clone(),
    };
    let child = ManagedExecutableRepository::resolve_invocation(&mut tx, &revised.policy, &binding)
        .await
        .unwrap();
    assert_eq!(child.executable.action.id, leaf.id);
    assert!(revised
        .policy
        .select(
            revised.id,
            &binding,
            &[],
            fixture.admission.authority.attribution.clone()
        )
        .is_ok());
    let disallowed = InvocationBinding::WorkflowTask {
        workflow_ref: fixture.credential.action_ref.clone(),
        task_path: "/tasks/0".into(),
        action_ref: fixture.credential.action_ref.clone(),
    };
    assert!(
        ManagedExecutableRepository::resolve_invocation(&mut tx, &revised.policy, &disallowed)
            .await
            .is_err()
    );
    let old = ManagedExecutableRepository::resolve_invocation(
        &mut tx,
        &fixture.policy.policy,
        &fixture.admission.authority.binding,
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(old).unwrap(),
        serde_json::to_value(original).unwrap()
    );
    let root = ManagedExecutableRepository::resolve_invocation(
        &mut tx,
        &revised.policy,
        &fixture.admission.authority.binding,
    )
    .await
    .unwrap();
    assert_eq!(
        root.executable.action.param_schema,
        Some(json!({"target":{"type":"string"}}))
    );
    assert_eq!(
        root.executable.workflow_definition.unwrap().definition["tasks"][0]["retry"]["count"],
        2
    );
    RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn managed_rule_readmission_retains_dispatch_metadata_without_unrelated_key_rights() {
    use attune_common::repositories::{
        managed_component_readmission::ManagedComponentReadmission,
        managed_executable::ManagedExecutableRepository,
        rule::{CreateRuleInput, RuleRepository, UpdateRuleInput},
        schema_type::SchemaTypeRepository,
    };
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let original = ManagedExecutableRepository::resolve_invocation(
        &mut tx,
        &fixture.policy.policy,
        &fixture.admission.authority.binding,
    )
    .await
    .unwrap();
    let rule = RuleRepository::create(
        &mut *tx,
        CreateRuleInput {
            r#ref: format!("{}.dispatch", fixture.policy.policy.owner.pack_ref),
            pack: fixture.policy.policy.owner.pack_id,
            pack_ref: fixture.policy.policy.owner.pack_ref.clone(),
            label: "Dispatch".into(),
            description: None,
            action: original.executable.action.id,
            action_ref: fixture.credential.action_ref.clone(),
            trigger: fixture.external_trigger_id,
            trigger_ref: "shared.completed".into(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: Some(vec![]),
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();
    let admitted = ManagedComponentReadmission::rule(&mut tx, rule.id, fixture.human_id)
        .await
        .unwrap()
        .unwrap();
    RuleRepository::update(
        &mut *tx,
        rule.id,
        UpdateRuleInput {
            action_params: Some(json!({"literal":"new"})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let revised = ManagedComponentReadmission::rule(&mut tx, rule.id, fixture.human_id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(admitted.id, revised.id);
    assert_eq!(revised.policy.release_id, original.release.id);
    let old_body: serde_json::Value = sqlx::query_scalar("SELECT metadata FROM managed_dispatch_metadata_revision WHERE policy_revision = $1 AND component_id = $2").bind(admitted.id).bind(rule.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(old_body["action_params"], json!({}));
    assert_eq!(
        old_body["managing_pack_id"],
        fixture.policy.policy.owner.pack_id
    );
    assert_eq!(
        old_body["trigger_managing_pack_id"],
        fixture.external_pack_id
    );
    // A label update does not call readmission or require captured input rights.
    RuleRepository::update(
        &mut *tx,
        rule.id,
        UpdateRuleInput {
            label: Some("New label".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        RuntimeAuthorityRepository::current_policy(&mut tx, fixture.policy.policy.owner.pack_id)
            .await
            .unwrap()
            .unwrap()
            .id,
        revised.id
    );
    RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn current_policy_replacement_does_not_rebind_existing_admission() {
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let current =
        RuntimeAuthorityRepository::current_policy(&mut tx, fixture.policy.policy.owner.pack_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(current.id, fixture.policy.id);
    let mut reduced = fixture.policy.policy.0.clone();
    reduced.invocations[0].permission_sets.clear();
    reduced.invocations[0].standard_access = false;
    let replacement = RuntimeAuthorityRepository::capture_policy(&mut tx, &reduced)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, replacement.id)
        .await
        .unwrap();
    let current = RuntimeAuthorityRepository::current_policy(&mut tx, reduced.owner.pack_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.id, replacement.id);
    assert!(current
        .policy
        .select(
            current.id,
            &fixture.admission.authority.binding,
            &fixture.admission.authority.selected_refs,
            fixture.admission.authority.attribution.clone()
        )
        .is_err());
    let admitted =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await
            .unwrap();
    assert_eq!(admitted.authority().policy_id, fixture.policy.id);
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn activation_rechecks_external_approval_after_policy_capture() {
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    RuntimeAuthorityRepository::revoke_approval(&mut tx, fixture.approval.id)
        .await
        .unwrap();
    assert!(
        RuntimeAuthorityRepository::activate_policy(&mut tx, fixture.policy.id)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn deployment_reuses_external_approval_and_only_auto_approves_actual_installer_coverage() {
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let approvals = RuntimeAuthorityRepository::admit_external_grants(
        &mut tx,
        fixture.approval.subject_pack,
        fixture.human_id,
        &fixture.approval.grants,
    )
    .await
    .unwrap();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].id, fixture.approval.id);
    RuntimeAuthorityRepository::revoke_approval(&mut tx, fixture.approval.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    // This installer has Permissions/Manage but not Events/Create. Automatic
    // delegation must not turn policy publication rights into runtime authority.
    assert!(RuntimeAuthorityRepository::admit_external_grants(
        &mut tx,
        fixture.approval.subject_pack,
        fixture.human_id,
        &fixture.approval.grants
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    let set = PermissionSetRepository::create(
        &db,
        CreatePermissionSetInput {
            r#ref: "fixture.covered_external_runtime".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: serde_json::to_value(&fixture.approval.grants.0).unwrap(),
        },
    )
    .await
    .unwrap();
    PermissionAssignmentRepository::create(
        &db,
        CreatePermissionAssignmentInput {
            identity: fixture.human_id,
            permset: set.id,
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let first = RuntimeAuthorityRepository::admit_external_grants(
        &mut tx,
        fixture.approval.subject_pack,
        fixture.human_id,
        &fixture.approval.grants,
    )
    .await
    .unwrap();
    let repeated = RuntimeAuthorityRepository::admit_external_grants(
        &mut tx,
        fixture.approval.subject_pack,
        fixture.human_id,
        &fixture.approval.grants,
    )
    .await
    .unwrap();
    assert_eq!(first, repeated);
    assert_eq!(first.len(), 1);
    assert_ne!(first[0].id, fixture.approval.id);
    let (_, count) =
        RuntimeAuthorityRepository::list_approvals(&mut tx, fixture.approval.subject_pack, 100, 0)
            .await
            .unwrap();
    assert_eq!(count, 2);
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn approval_replacement_is_new_authority_and_does_not_rebind_an_existing_run() {
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let replacement = RuntimeAuthorityRepository::replace_approval(
        &mut tx,
        fixture.approval.id,
        fixture.human_id,
        fixture.approval.grants.0.clone(),
        Some(Utc::now() + Duration::hours(1)),
    )
    .await
    .unwrap();
    assert_ne!(replacement.id, fixture.approval.id);
    let previous = RuntimeAuthorityRepository::find_approval(&mut tx, fixture.approval.id)
        .await
        .unwrap()
        .unwrap();
    assert!(previous.revoked_at.is_some());
    assert!(replacement.is_active(Utc::now()));
    let (records, total) =
        RuntimeAuthorityRepository::list_approvals(&mut tx, previous.subject_pack, 1, 0)
            .await
            .unwrap();
    assert_eq!(total, 2);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id, replacement.id);
    assert!(RuntimeAuthorityRepository::validate_execution_credential(
        &mut tx,
        &fixture.credential
    )
    .await
    .is_err());
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn replacing_a_revoked_approval_rolls_back_without_leaving_an_active_approval() {
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    RuntimeAuthorityRepository::revoke_approval(&mut tx, fixture.approval.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    assert!(RuntimeAuthorityRepository::replace_approval(
        &mut tx,
        fixture.approval.id,
        fixture.human_id,
        fixture.approval.grants.0.clone(),
        None
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    let mut connection = db.acquire().await.unwrap();
    let (records, total) = RuntimeAuthorityRepository::list_approvals(
        &mut connection,
        fixture.approval.subject_pack,
        100,
        0,
    )
    .await
    .unwrap();
    assert_eq!(total, 1);
    assert!(records.iter().all(|record| record.revoked_at.is_some()));
    drop(connection);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn policy_capture_rejects_an_action_absent_from_the_selected_release() {
    let (db, fixture) = fixture().await;
    let mut invalid = fixture.policy.policy.0.clone();
    invalid.invocations[0].binding = InvocationBinding::Action {
        action_ref: format!("{}.not_in_release", invalid.owner.pack_ref),
    };
    let mut tx = db.begin().await.unwrap();
    assert!(
        RuntimeAuthorityRepository::capture_policy(&mut tx, &invalid)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn policy_capture_rejects_an_unrelated_executable_release_even_with_a_valid_digest() {
    let (db, fixture) = fixture().await;
    let pack = PackFixture::new_unique("unrelated_release")
        .create(&db)
        .await
        .unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "other")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = activate_release(&mut tx, pack.id, &pack.r#ref, vec![action.id], 'f').await;
    let mut invalid = fixture.policy.policy.0.clone();
    invalid.invocations[0].target_release_id = release.id;
    invalid.invocations[0].target_release_digest = release.digest;
    assert!(
        RuntimeAuthorityRepository::capture_policy(&mut tx, &invalid)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn invocation_assembly_uses_pinned_defaults_and_explicit_reduction_without_live_iam() {
    use attune_common::repositories::action::{ActionRepository, UpdateActionInput};
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let snapshot = ExecutableSnapshotRepository::resolve_for_action_ref(
        &mut *tx,
        &fixture.credential.action_ref,
    )
    .await
    .unwrap();
    ActionRepository::update(
        &mut *tx,
        snapshot.executable.action.id,
        UpdateActionInput {
            default_execution_permission_set_refs: Some(vec!["unapproved.live_default".into()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let binding = fixture.admission.authority.binding.clone();
    let sets = &fixture.policy.policy.invocations[0].permission_sets;
    let inherited =
        InvocationPolicy::from_executable(binding.clone(), &snapshot, None, sets).unwrap();
    assert!(!inherited.standard_access);
    assert!(inherited.permission_sets.is_empty());
    let empty =
        InvocationPolicy::from_executable(binding.clone(), &snapshot, Some(&[]), sets).unwrap();
    assert_eq!(inherited, empty);
    let requested = vec![
        fixture.admission.authority.selected_refs[0].clone(),
        "standard".into(),
        "standard".into(),
    ];
    let explicit =
        InvocationPolicy::from_executable(binding.clone(), &snapshot, Some(&requested), sets)
            .unwrap();
    assert!(explicit.standard_access);
    assert_eq!(explicit.permission_sets, *sets);
    assert_eq!(explicit.target_release_id, snapshot.release.id);
    assert!(InvocationPolicy::from_executable(
        binding.clone(),
        &snapshot,
        Some(&["unapproved.live_default".into()]),
        sets
    )
    .is_err());
    let wrong = InvocationBinding::Action {
        action_ref: format!("{}.other", fixture.policy.policy.owner.pack_ref),
    };
    assert!(InvocationPolicy::from_executable(wrong, &snapshot, None, sets).is_err());
    let mut policy = fixture.policy.policy.0.clone();
    policy.invocations[0] = explicit;
    let recorded = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    // Reassembling the same admitted capabilities preserves the immutable revision.
    assert_eq!(recorded.id, fixture.policy.id);
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn candidate_policy_compiles_local_definitions_from_all_declarations_and_pinned_code() {
    use attune_common::pack_registry::runtime_policy::CandidateRuntimePolicyPlan;
    let (db, fixture) = fixture().await;
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("actions")).unwrap();
    std::fs::create_dir(root.path().join("permission_sets")).unwrap();
    let reference = format!("{}.local_emit", fixture.policy.policy.owner.pack_ref);
    std::fs::write(
        root.path().join("permission_sets/events.yaml"),
        format!("ref: {reference}\ngrants:\n  - resource: events\n    actions: [create]\n"),
    )
    .unwrap();
    std::fs::write(
        root.path().join("actions/emitter.yaml"),
        format!(
            "ref: {}\ndefault_execution_permission_set_refs: [{reference}]\n",
            fixture.credential.action_ref
        ),
    )
    .unwrap();
    let plan = CandidateRuntimePolicyPlan::read(root.path(), &fixture.policy.policy.owner.pack_ref)
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    // Deployment compiles the immutable release inventory, not the current
    // metadata overlay used by a new by-ref invocation.
    let executables = ExecutableSnapshotRepository::resolve_release_actions(
        &mut tx,
        fixture.policy.policy.release_id,
    )
    .await
    .unwrap();
    assert_eq!(
        executables.keys().collect::<Vec<_>>(),
        vec![&fixture.credential.action_ref]
    );
    let executable = &executables[&fixture.credential.action_ref];
    let mut policy = plan
        .compile(
            fixture.policy.policy.owner.clone(),
            &executable.release,
            &executables,
            &[],
            &BTreeMap::new(),
        )
        .unwrap();
    let expected_bindings = vec![InvocationBinding::Action {
        action_ref: fixture.credential.action_ref.clone(),
    }];
    assert_eq!(
        policy
            .invocations
            .iter()
            .map(|invocation| invocation.binding.clone())
            .collect::<Vec<_>>(),
        expected_bindings
    );
    assert_eq!(
        policy.invocations[0]
            .permission_sets
            .keys()
            .collect::<Vec<_>>(),
        vec![&reference]
    );
    let set = &policy.invocations[0].permission_sets[&reference];
    assert_eq!(
        set.grants,
        vec![attune_common::runtime_authority::AdmittedGrant::PackOwned {
            pack_id: fixture.policy.policy.owner.pack_id,
            grant: attune_common::rbac::Grant {
                resource: Resource::Events,
                actions: vec![Action::Create],
                constraints: None
            }
        }]
    );
    assert!(!policy.invocations[0].standard_access);
    // Deployment adds the immutable metadata pin after pure candidate compile.
    // Compare identical policies, not the IDs of pinned and unpinned bodies.
    attune_common::repositories::managed_executable::ManagedExecutableRepository::capture_deployment_actions(&mut tx,executable.release.id).await.unwrap();
    let metadata=attune_common::repositories::managed_executable::ManagedExecutableRepository::revision_for_snapshot(&mut tx,executable).await.unwrap();
    assert!(
        metadata.is_some(),
        "the expected policy pins the actual captured immutable deployment metadata"
    );
    assert!(policy.invocations[0].metadata_revision.is_none());
    policy.invocations[0].metadata_revision = metadata;
    let revision = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    assert_ne!(revision.id, fixture.policy.id);
    let deployed = plan
        .admit_deployment(
            &mut tx,
            fixture.policy.policy.owner.pack_id,
            fixture.human_id,
        )
        .await
        .unwrap();
    assert_eq!(
        deployed.policy, revision.policy,
        "all invocation declarations/grants and exact metadata pins must match"
    );
    assert_eq!(
        deployed
            .policy
            .invocations
            .iter()
            .map(|invocation| invocation.binding.clone())
            .collect::<Vec<_>>(),
        expected_bindings
    );
    assert_eq!(deployed.id, revision.id);
    assert_eq!(deployed.policy.invocations[0].metadata_revision, metadata);
    assert_eq!(
        RuntimeAuthorityRepository::current_policy(&mut tx, fixture.policy.policy.owner.pack_id,)
            .await
            .unwrap()
            .unwrap()
            .id,
        deployed.id
    );
    // Activation changes future admission, never the already recorded run.
    let original =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await
            .unwrap();
    assert_eq!(original.authority().policy_id, fixture.policy.id);
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn candidate_release_enumeration_preserves_disabled_declarations_and_immutable_defaults() {
    use attune_common::repositories::action::{ActionRepository, UpdateActionInput};
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let original = ExecutableSnapshotRepository::resolve_for_action_ref(
        &mut *tx,
        &fixture.credential.action_ref,
    )
    .await
    .unwrap();
    ActionRepository::update(
        &mut *tx,
        original.executable.action.id,
        UpdateActionInput {
            enabled: Some(false),
            default_execution_permission_set_refs: Some(vec!["unapproved.changed".into()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(
        &mut tx,
        fixture.policy.policy.release_id,
    )
    .await
    .unwrap();
    let pinned = &snapshots[&fixture.credential.action_ref];
    assert_eq!(pinned.executable.action.id, original.executable.action.id);
    assert_eq!(
        pinned
            .executable
            .action
            .default_execution_permission_set_refs,
        original
            .executable
            .action
            .default_execution_permission_set_refs
    );
    assert!(ExecutableSnapshotRepository::resolve_for_action_ref(
        &mut *tx,
        &fixture.credential.action_ref
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn validated_authority_checks_resolved_event_ownership_and_target_lifecycle() {
    use attune_common::repositories::runtime_target::{RuntimeTarget, RuntimeTargetRepository};
    let (db, fixture) = fixture().await;
    let owner = &fixture.policy.policy.owner;
    let enabled =
        helpers::TriggerFixture::new(Some(owner.pack_id), Some(owner.pack_ref.clone()), "enabled")
            .create(&db)
            .await
            .unwrap();
    let disabled = helpers::TriggerFixture::new(
        Some(owner.pack_id),
        Some(owner.pack_ref.clone()),
        "disabled",
    )
    .with_enabled(false)
    .create(&db)
    .await
    .unwrap();
    let other = PackFixture::new_unique("other_target")
        .create(&db)
        .await
        .unwrap();
    let foreign = helpers::TriggerFixture::new(Some(other.id), Some(other.r#ref), "unapproved")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await
            .unwrap();
    for (trigger, admitted, active) in [
        (enabled, true, true),
        (disabled, true, false),
        (foreign, false, false),
    ] {
        let target = RuntimeTargetRepository::resolve(
            &mut tx,
            RuntimeTarget::EventTrigger(trigger.id),
            fixture.credential.sponsor_identity_id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(authority.allows_target(&target, Action::Create), admitted);
        assert_eq!(authority.allows_new_effect(&target, Action::Create), active);
    }
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn new_release_rejects_stale_current_policy_but_keeps_admitted_old_execution() {
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let snapshot = ExecutableSnapshotRepository::resolve_for_action_ref(
        &mut *tx,
        &fixture.credential.action_ref,
    )
    .await
    .unwrap();
    activate_release(
        &mut tx,
        fixture.policy.policy.owner.pack_id,
        &fixture.policy.policy.owner.pack_ref,
        vec![snapshot.executable.action.id],
        'e',
    )
    .await;
    assert!(RuntimeAuthorityRepository::current_policy(
        &mut tx,
        fixture.policy.policy.owner.pack_id
    )
    .await
    .is_err());
    assert!(
        RuntimeAuthorityRepository::activate_policy(&mut tx, fixture.policy.id)
            .await
            .is_err()
    );
    let old =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await
            .unwrap();
    assert_eq!(old.authority().policy_id, fixture.policy.id);
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn policy_dependencies_retain_foreign_code_until_revision_and_runtime_retention() {
    let packs = tempfile::tempdir().unwrap();
    let source_content = owned_release_content(packs.path(), 'a');
    let target_content = owned_release_content(packs.path(), 'b');
    let (db, fixture) = fixture_with_release_content(None, Some(source_content.clone())).await;
    let target_pack = PackFixture::new_unique("authority_target")
        .create(&db)
        .await
        .unwrap();
    let target_action = ActionFixture::new(target_pack.id, &target_pack.r#ref, "run")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let target = activate_release_with_content(
        &mut tx,
        target_pack.id,
        &target_pack.r#ref,
        vec![target_action.id],
        'b',
        Some(target_content.clone()),
    )
    .await;
    let mut expanded = fixture.policy.policy.0.clone();
    let queue = attune_common::repositories::work_queue::WorkQueueRepository::create(
        &mut *tx,
        attune_common::repositories::work_queue::CreateWorkQueueInput {
            r#ref: format!("{}.jobs", expanded.owner.pack_ref),
            pack: Some(expanded.owner.pack_id),
            pack_ref: Some(expanded.owner.pack_ref.clone()),
            is_adhoc: false,
            label: "Jobs".into(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(target_action.id),
            dispatch_action_ref: target_action.r#ref.clone(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: attune_common::models::WorkQueueUpdateStrategy::Immutable,
            batch_mode: attune_common::models::WorkQueueBatchMode::Single,
            item_schema: json!({}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            config: json!({}),
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: Vec::new(),
        },
    )
    .await
    .unwrap();
    let queue_release: Option<Id> =
        sqlx::query_scalar("SELECT managed_release FROM work_queue WHERE id = $1")
            .bind(queue.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        queue_release,
        Some(fixture.policy.policy.release_id),
        "queue insertion independently pins the source release"
    );
    expanded.invocations.push(InvocationPolicy {
        dispatch_template_digest: None,
        source_id: None,
        metadata_revision: None,
        foreign_workflow_policy: None,
        binding: InvocationBinding::Queue {
            queue_ref: format!("{}.jobs", expanded.owner.pack_ref),
            action_ref: target_action.r#ref.clone(),
        },
        target_release_id: target.id,
        target_release_digest: target.digest,
        permission_sets: BTreeMap::new(),
        standard_access: false,
    });
    let revision = RuntimeAuthorityRepository::capture_policy(&mut tx, &expanded)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, revision.id)
        .await
        .unwrap();
    let snapshot = ExecutableSnapshotRepository::resolve_for_action_ref(
        &mut *tx,
        &fixture.credential.action_ref,
    )
    .await
    .unwrap();
    let selected = expanded
        .select(
            revision.id,
            &fixture.admission.authority.binding,
            &fixture.admission.authority.selected_refs,
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: fixture.credential.action_ref.clone(),
            permission_set_refs: selected.selected_refs.clone(),
            status: ExecutionStatus::Running,
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(execution.id),
        &selected,
    )
    .await
    .unwrap();
    // Replace both live releases. The old policy must still keep its foreign code.
    let target_current = activate_release_with_content(
        &mut tx,
        target_pack.id,
        &target_pack.r#ref,
        vec![target_action.id],
        'c',
        Some(owned_release_content(packs.path(), 'c')),
    )
    .await;
    // Release activation alone does not remove omitted projections. Exercise
    // production's explicit Remove reconciliation before expecting both old
    // releases to become collectible after their runtime policy owners expire.
    attune_common::repositories::component_lifecycle::ComponentLifecycleRepository::reconcile_omissions(
        &mut tx,expanded.owner.pack_id,&PackProjectionIds {actions:vec![snapshot.executable.action.id],..Default::default()},
    ).await.unwrap();
    let queue_state: (bool, Option<Id>) = sqlx::query_as(
        "SELECT retired_at IS NOT NULL,managed_release FROM work_queue WHERE id = $1",
    )
    .bind(queue.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(queue_state, (true, None));
    let next = activate_release_with_content(
        &mut tx,
        expanded.owner.pack_id,
        &expanded.owner.pack_ref,
        vec![snapshot.executable.action.id],
        'd',
        Some(owned_release_content(packs.path(), 'd')),
    )
    .await;
    let replacement = RuntimePolicy {
        concrete_targets: BTreeMap::new(),
        owner: expanded.owner.clone(),
        release_id: next.id,
        release_digest: next.digest,
        invocations: Vec::new(),
    };
    let replacement = RuntimeAuthorityRepository::capture_policy(&mut tx, &replacement)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, replacement.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let cutoff = Utc::now() + Duration::hours(1);
    let retained = PackRetentionRepository::collect(&db, packs.path(), cutoff, 0, 20)
        .await
        .unwrap();
    assert_eq!(retained.deleted, 0);
    assert!(std::path::Path::new(&source_content).exists());
    assert!(std::path::Path::new(&target_content).exists());
    let pins:Vec<Id> = sqlx::query_scalar("SELECT release FROM runtime_authority_release_dependency WHERE policy_revision = $1 ORDER BY release")
        .bind(revision.id).fetch_all(&*db).await.unwrap();
    let mut expected_pins = vec![fixture.policy.policy.release_id, target.id];
    expected_pins.sort();
    assert_eq!(
        pins, expected_pins,
        "retained policy owns both code dependencies even after queue retirement"
    );
    // Revocation is not permission to erase retained execution policy evidence.
    let mut tx = db.begin().await.unwrap();
    RuntimeAuthorityRepository::revoke_policy(&mut tx, revision.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        PackRetentionRepository::collect(&db, packs.path(), cutoff, 0, 20)
            .await
            .unwrap()
            .deleted,
        0
    );
    ExecutionRepository::delete(&db, execution.id)
        .await
        .unwrap();
    ExecutionRepository::delete(&db, fixture.credential.execution_id)
        .await
        .unwrap();
    let collected = PackRetentionRepository::collect(&db, packs.path(), cutoff, 0, 20)
        .await
        .unwrap();
    assert_eq!(collected.deleted, 2);
    let remaining: Vec<Id> = sqlx::query_scalar("SELECT id FROM pack_release ORDER BY id")
        .fetch_all(&*db)
        .await
        .unwrap();
    let mut expected_remaining = vec![next.id, target_current.id];
    expected_remaining.sort();
    assert_eq!(
        remaining, expected_remaining,
        "only the two active releases survive"
    );
    assert!(!std::path::Path::new(&source_content)
        .parent()
        .unwrap()
        .exists());
    assert!(!std::path::Path::new(&target_content)
        .parent()
        .unwrap()
        .exists());
    assert!(std::path::Path::new(&next.content_path).exists());
    assert!(std::path::Path::new(&target_current.content_path).exists());
    let stale_authority:(i64,i64,i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM runtime_authority_admission WHERE policy_revision = ANY($1)),(SELECT count(*) FROM runtime_authority_release_dependency WHERE policy_revision = ANY($1)),(SELECT count(*) FROM runtime_authority_metadata_dependency WHERE policy_revision = ANY($1)),(SELECT count(*) FROM runtime_workflow_policy_dependency WHERE policy_revision = ANY($1) OR dependency = ANY($1)),(SELECT count(*) FROM runtime_declared_input_binding WHERE policy_revision = ANY($1)),(SELECT count(*) FROM managed_executable_metadata_current c JOIN managed_executable_metadata_revision m ON m.id = c.revision WHERE m.release = ANY($2))")
        .bind(vec![revision.id,fixture.policy.id]).bind(vec![fixture.policy.policy.release_id,target.id]).fetch_one(&*db).await.unwrap();
    assert_eq!(stale_authority, (0, 0, 0, 0, 0, 0));
    let mut tx = db.begin().await.unwrap();
    assert!(
        RuntimeAuthorityRepository::find_policy(&mut tx, revision.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        RuntimeAuthorityRepository::current_policy(&mut tx, replacement.policy.owner.pack_id)
            .await
            .unwrap()
            .is_some()
    );
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

async fn fixture() -> (TestDatabase, Fixture) {
    fixture_with_input_defaults(None).await
}

#[tokio::test]
async fn accepted_ad_hoc_batch_retains_all_source_fences_after_item_and_execution_retention() {
    use attune_common::repositories::{
        accepted_batch_dependency::AcceptedBatchDependencyRepository,
        managed_executable::ManagedExecutableRepository,
        schema_contract::SchemaContractRepository,
        work_queue::{CreateWorkQueueItemInput, WorkQueueItemRepository},
    };
    let (db, fixture) = fixture_with_input_defaults(Some(
        json!({"first":{"type":"string","secret":true},"second":{"type":"string","secret":true}}),
    ))
    .await;
    let installed:bool = sqlx::query_scalar("SELECT to_regclass('work_queue_item_ad_hoc_dispatch') IS NOT NULL AND to_regclass('recorded_queue_item_input') IS NOT NULL").fetch_one(&*db).await.unwrap();
    assert!(
        installed,
        "this combined contract test requires B migrations 02004 and 02005"
    );
    let mut tx = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let binding = &fixture.policy.policy.invocations[0].binding;
    let snapshot =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &fixture.policy.policy, binding)
            .await
            .unwrap();
    let source_selection = fixture
        .policy
        .policy
        .select(
            fixture.policy.id,
            binding,
            &[],
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    let second = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            permission_set_refs: vec![],
            config: Some(json!({})),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let second_admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(second.id),
        &source_selection,
    )
    .await
    .unwrap();
    let queue = queue_fixture(
        &mut tx,
        &fixture.policy.policy.owner,
        snapshot.executable.action.id,
        &snapshot.executable.action.r#ref,
        true,
        attune_common::models::WorkQueueBatchMode::Batch,
    )
    .await;
    let template = serde_json::to_value(&queue).unwrap();
    let schema = serde_json::to_value(
        SchemaContractRepository::materialize_action_pair(&mut tx, &snapshot)
            .await
            .unwrap(),
    )
    .unwrap();
    let mut items = Vec::new();
    for source in [&fixture.admission, &second_admission] {
        let item = WorkQueueItemRepository::create_pinned(
            &mut *tx,
            CreateWorkQueueItemInput {
                queue: queue.id,
                queue_ref: queue.r#ref.clone(),
                item_key: None,
                priority: 0,
                status: attune_common::models::WorkQueueItemStatus::Queued,
                payload: json!({"literal":"unchanged"}),
                metadata: json!({}),
                trace_tag: None,
                enqueue_source: "recorded-test".into(),
                requested_by_identity: None,
                requested_by_execution: Some(source.owner_id),
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
        let accepted = fixture
            .policy
            .policy
            .select(
                fixture.policy.id,
                binding,
                &[],
                RequestAttribution::ParentExecution {
                    execution_id: source.owner_id,
                },
            )
            .unwrap();
        let authority = serde_json::to_value(&accepted).unwrap();
        sqlx::query("INSERT INTO work_queue_item_runtime_admission(item,policy_revision,authority,authority_digest) VALUES ($1,$2,$3,$4)")
        .bind(item.id).bind(accepted.policy_id).bind(&authority).bind(fixture_digest(&authority)).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO work_queue_item_ad_hoc_dispatch(item,source_admission,template,template_digest,schema_binding,schema_digest) VALUES ($1,$2,$3,$4,$5,$6)")
        .bind(item.id).bind(source.id).bind(&template).bind(fixture_digest(&template)).bind(&schema).bind(fixture_digest(&schema)).execute(&mut *tx).await.unwrap();
        items.push(item.id);
    }
    let authority = fixture
        .policy
        .policy
        .select(
            fixture.policy.id,
            binding,
            &[],
            RequestAttribution::QueueBatch {
                queue_ref: queue.r#ref.clone(),
                items: items.iter().map(|id| (*id, None)).collect(),
            },
        )
        .unwrap();
    let batch = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            permission_set_refs: vec![],
            config: Some(json!({"first":attune_common::secret_values::redaction_marker(),"second":attune_common::secret_values::redaction_marker()})),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(batch.id),
        &authority,
    )
    .await
    .unwrap();
    assert!(!admission.authority.has_api_access());
    assert!(
        RuntimeAuthorityRepository::retained_admission_fence(&mut tx, admission.id)
            .await
            .is_err(),
        "an unsealed Action batch must never authorize effects"
    );
    assert!(
        AcceptedBatchDependencyRepository::capture(&mut tx, admission.id, &items[..1])
            .await
            .is_err()
    );
    AcceptedBatchDependencyRepository::capture(&mut tx, admission.id, &items)
        .await
        .unwrap()
        .ensure_fresh_on(&mut tx)
        .await
        .unwrap();
    // Legitimate local payloads from contributors with no Key/API authority
    // become typed sources without requiring cross-requester Queue read grants.
    use attune_common::repositories::{
        accepted_input::AcceptedInputRepository as Accepted,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        inherited_input::{InputEntityKind, InputSite, InputSitePurpose},
        recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
    };
    let encryption_key = ACCEPTED_BATCH_TEST_KEY;
    let mut originals = Vec::new();
    let mut secret_values = Vec::new();
    for (item, path) in items.iter().zip(["/first", "/second"]) {
        let original = attune_common::secret_provenance::SecretProvenance {
            origins: vec![attune_common::secret_provenance::SecretOrigin::QueueItem {
                queue_id: queue.id,
                queue_ref: queue.r#ref.clone(),
                pack_ref: queue.pack_ref.clone(),
                item_id: *item,
                path: "/literal".into(),
            }],
            templates: vec![],
        };
        secret_values.push(attune_common::secret_values::SecretValueInput {
            json_path: path.into(),
            value: json!("unchanged"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&original).unwrap()),
        });
        originals.push(original);
    }
    let binding = SchemaContractRepository::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    SchemaContractRepository::attach(
        &mut tx,
        attune_common::repositories::schema_contract::BindingOwner::Execution,
        batch.id,
        &binding,
    )
    .await
    .unwrap();
    Contributors::capture_queue_execution(&mut tx, batch.id, &items)
        .await
        .unwrap();
    Secrets::upsert_many_with_conn(
        &mut tx,
        attune_common::secret_values::ENTITY_EXECUTION_CONFIG,
        batch.id,
        &attune_common::secret_values::prepare_secret_values(secret_values, encryption_key)
            .unwrap(),
    )
    .await
    .unwrap();
    Accepted::seal_execution(&mut tx, batch.id, encryption_key)
        .await
        .unwrap();
    let accepted_site = InputSite {
        execution: batch.id,
        kind: InputEntityKind::ExecutionConfig,
        purpose: InputSitePurpose::Value,
        path: "/first".into(),
    };
    assert_eq!(
        Accepted::prepare_source(
            &mut tx,
            batch.id,
            &accepted_site,
            &originals[0],
            encryption_key
        )
        .await
        .unwrap()
        .value(),
        &json!("unchanged")
    );
    IdentityRepository::update(
        &mut *tx,
        fixture.human_id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for item in &items {
        WorkQueueItemRepository::delete(&mut *tx, *item)
            .await
            .unwrap();
    }
    assert!(Accepted::prepare_source(
        &mut tx,
        batch.id,
        &accepted_site,
        &originals[0],
        encryption_key
    )
    .await
    .is_ok());
    ExecutionRepository::delete(&mut *tx, fixture.admission.owner_id)
        .await
        .unwrap();
    ExecutionRepository::delete(&mut *tx, second.id)
        .await
        .unwrap();
    RuntimeAuthorityRepository::retained_admission_fence(&mut tx, admission.id)
        .await
        .unwrap();
    let deps:Vec<Id> = sqlx::query_scalar("SELECT dependency FROM runtime_admission_dependency WHERE admission = $1 AND dependency_kind = 'accepted_item' ORDER BY dependency")
        .bind(admission.id).fetch_all(&mut *tx).await.unwrap();
    assert_eq!(deps, vec![fixture.admission.id, second_admission.id]);
    RuntimeAuthorityRepository::revoke_admission(&mut tx, second_admission.id)
        .await
        .unwrap();
    assert!(
        RuntimeAuthorityRepository::retained_admission_fence(&mut tx, admission.id)
            .await
            .is_err(),
        "the second source's independent revocation must still stop the batch"
    );
    ExecutionRepository::delete(&mut *tx, batch.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    PackRetentionRepository::collect(&db, root.path(), Utc::now() + Duration::hours(1), 0, 20)
        .await
        .unwrap();
    let first_epoch: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM runtime_authority_admission WHERE id=ANY($1) ORDER BY id",
    )
    .bind(vec![
        fixture.admission.id,
        second_admission.id,
        admission.id,
    ])
    .fetch_all(&*db)
    .await
    .unwrap();
    let mut expected_sources = vec![fixture.admission.id, second_admission.id];
    expected_sources.sort();
    assert_eq!(first_epoch,expected_sources,"one bounded SQL snapshot reclaims the batch leaf, not its referenced source parents in the same epoch");
    // The first DELETE sees the batch's dependency edges in its statement
    // snapshot. Only the next normal graph-first collector epoch can select
    // the now-unreferenced parents. Do not bypass pins or force-delete rows.
    PackRetentionRepository::collect(&db, root.path(), Utc::now() + Duration::hours(1), 0, 20)
        .await
        .unwrap();
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM runtime_authority_admission WHERE id = ANY($1)")
            .bind(vec![
                fixture.admission.id,
                second_admission.id,
                admission.id,
            ])
            .fetch_one(&*db)
            .await
            .unwrap();
    assert_eq!(
        remaining, 0,
        "bounded leaf cleanup releases source proofs only after the batch is gone"
    );
    db.cleanup().await.unwrap();
}

async fn approval_effect_after_target_wait(lifetime: Option<Duration>, expire_while_blocked: bool) {
    use attune_common::repositories::{
        event::{CreateEventInput, EventRepository},
        runtime_target::{RuntimeTarget, RuntimeTargetRepository},
    };
    let (db, fixture) = fixture_with_approval_lifetime(None, None, lifetime).await;
    let mut blocker = db.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM trigger WHERE id = $1 FOR UPDATE")
        .bind(fixture.external_trigger_id)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let pool = (*db).clone();
    let credential = fixture.credential.clone();
    let trigger = fixture.external_trigger_id;
    let (validated, ready) = tokio::sync::oneshot::channel();
    let operation = tokio::spawn(async move {
        let mut tx = pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let authority =
            RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &credential)
                .await
                .unwrap();
        assert!(authority.ensure_fresh().is_ok());
        validated.send(pid).unwrap();
        // The test releases this real row lock only after observing its waiter
        // in pg_blocking_pids, and, for the negative case, actual DB expiry.
        let target = RuntimeTargetRepository::resolve(
            &mut tx,
            RuntimeTarget::EventTrigger(trigger),
            credential.sponsor_identity_id,
        )
        .await
        .unwrap()
        .unwrap();
        let allowed = authority.allows_new_effect(&target, Action::Create);
        let fresh = authority.ensure_fresh_on(&mut tx).await.is_ok();
        let retained_fresh = authority.effect_fence().ensure_fresh().is_ok();
        if allowed && fresh {
            EventRepository::create(
                &mut *tx,
                CreateEventInput {
                    trigger: Some(trigger),
                    trigger_ref: "shared.completed".into(),
                    config: None,
                    payload: Some(json!({"after_wait":true})),
                    trace_tag: Some("approval-after-target-wait".into()),
                    source: None,
                    source_ref: None,
                    rule: None,
                    rule_ref: None,
                },
            )
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
        (allowed, fresh, retained_fresh)
    });
    let pid = tokio::time::timeout(std::time::Duration::from_secs(60), ready)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT $2 = ANY(pg_blocking_pids($1))")
                .bind(pid)
                .bind(blocker_pid)
                .fetch_one(&*db)
                .await
                .unwrap();
            if blocked {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    if expire_while_blocked {
        let expiry = fixture.approval.expires_at.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                let expired: bool = sqlx::query_scalar("SELECT clock_timestamp() >= $1")
                    .bind(expiry)
                    .fetch_one(&*db)
                    .await
                    .unwrap();
                if expired {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    blocker.rollback().await.unwrap();
    let verdict = tokio::time::timeout(std::time::Duration::from_secs(10), operation)
        .await
        .unwrap()
        .unwrap();
    let expected = !expire_while_blocked;
    assert_eq!(verdict, (expected, expected, expected));
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event WHERE trace_tag = 'approval-after-target-wait'",
    )
    .fetch_one(&*db)
    .await
    .unwrap();
    assert_eq!(
        events,
        i64::from(expected),
        "an expired source must not emit after its target lock wait"
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn approval_expiry_during_target_lock_wait_denies_effect() {
    approval_effect_after_target_wait(Some(Duration::seconds(20)), true).await;
}

#[tokio::test]
async fn unexpired_approval_survives_target_lock_wait() {
    approval_effect_after_target_wait(Some(Duration::hours(1)), false).await;
}

#[tokio::test]
async fn approval_without_expiry_survives_target_lock_wait() {
    approval_effect_after_target_wait(None, false).await;
}

#[tokio::test]
async fn retained_queue_projection_pins_source_code_after_authority_pruning_until_explicit_remove()
{
    use attune_common::repositories::component_lifecycle::ComponentLifecycleRepository;
    let packs = tempfile::tempdir().unwrap();
    let old_content = owned_release_content(packs.path(), 'a');
    let (db, fixture) = fixture_with_release_content(None, Some(old_content.clone())).await;
    let mut tx = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let snapshot = ExecutableSnapshotRepository::resolve_for_action_ref(
        &mut *tx,
        &fixture.credential.action_ref,
    )
    .await
    .unwrap();
    let queue = owned_queue(
        &mut tx,
        &fixture.policy.policy.owner,
        snapshot.executable.action.id,
        &fixture.credential.action_ref,
    )
    .await;
    let projections = PackProjectionIds {
        actions: vec![snapshot.executable.action.id],
        ..Default::default()
    };
    ComponentLifecycleRepository::reconcile_omissions_with_policy(
        &mut tx,
        fixture.policy.policy.owner.pack_id,
        &projections,
        attune_common::models::AbsentMetadataPolicy::Retain,
    )
    .await
    .unwrap();
    let current = activate_release_with_content(
        &mut tx,
        fixture.policy.policy.owner.pack_id,
        &fixture.policy.policy.owner.pack_ref,
        projections.actions.clone(),
        'd',
        Some(owned_release_content(packs.path(), 'd')),
    )
    .await;
    let replacement = RuntimePolicy {
        owner: fixture.policy.policy.owner.clone(),
        release_id: current.id,
        release_digest: current.digest.clone(),
        invocations: vec![],
        concrete_targets: BTreeMap::new(),
    };
    let replacement = RuntimeAuthorityRepository::capture_policy(&mut tx, &replacement)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, replacement.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    ExecutionRepository::delete(&db, fixture.credential.execution_id)
        .await
        .unwrap();
    let cutoff = Utc::now() + Duration::hours(1);
    for _ in 0..2 {
        assert_eq!(
            PackRetentionRepository::collect(&db, packs.path(), cutoff, 0, 20)
                .await
                .unwrap()
                .deleted,
            0,
            "another pruning pass must not erase a live retained queue's code"
        );
    }
    let mut tx = db.begin().await.unwrap();
    assert!(
        RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy.id)
            .await
            .unwrap()
            .is_none()
    );
    let authority_pins:(i64,i64,i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM runtime_authority_release_dependency WHERE release = $1),(SELECT count(*) FROM runtime_authority_admission WHERE policy_revision = $2),(SELECT count(*) FROM runtime_declared_input_binding WHERE policy_revision = $2),(SELECT count(*) FROM runtime_workflow_policy_dependency WHERE dependency = $2),(SELECT count(*) FROM runtime_authority_metadata_dependency WHERE policy_revision = $2),(SELECT count(*) FROM managed_executable_metadata_current c JOIN managed_executable_metadata_revision m ON m.id = c.revision WHERE m.release = $1)")
        .bind(fixture.policy.policy.release_id).bind(fixture.policy.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(
        authority_pins,
        (0, 0, 0, 0, 0, 0),
        "authority/declaration/metadata pruning has already completed"
    );
    let queue_pin: (bool, Option<Id>) =
        sqlx::query_as("SELECT retired_at IS NULL,managed_release FROM work_queue WHERE id = $1")
            .bind(queue.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        queue_pin,
        (true, Some(fixture.policy.policy.release_id)),
        "the omitted live queue is the independent source-code pin"
    );
    assert!(std::path::Path::new(&old_content).exists());
    // Explicit removal retires the omitted projection; collection does not
    // rewrite ownership, remove a live pin, or weaken a restrictive dependency.
    ComponentLifecycleRepository::reconcile_omissions(
        &mut tx,
        fixture.policy.policy.owner.pack_id,
        &projections,
    )
    .await
    .unwrap();
    let queue_pin: (bool, Option<Id>) = sqlx::query_as(
        "SELECT retired_at IS NOT NULL,managed_release FROM work_queue WHERE id = $1",
    )
    .bind(queue.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(queue_pin, (true, None));
    tx.commit().await.unwrap();
    assert_eq!(
        PackRetentionRepository::collect(&db, packs.path(), cutoff, 0, 20)
            .await
            .unwrap()
            .deleted,
        1
    );
    assert!(!std::path::Path::new(&old_content)
        .parent()
        .unwrap()
        .exists());
    assert!(std::path::Path::new(&current.content_path).exists());
    let remaining: Vec<Id> = sqlx::query_scalar("SELECT id FROM pack_release ORDER BY id")
        .fetch_all(&*db)
        .await
        .unwrap();
    assert_eq!(remaining, vec![current.id]);
    db.cleanup().await.unwrap();
}

async fn fixture_with_input_defaults(
    defaults: Option<serde_json::Value>,
) -> (TestDatabase, Fixture) {
    fixture_with_release_content(defaults, None).await
}

#[tokio::test]
async fn inherited_execution_admission_keeps_approval_deadline_after_target_wait() {
    use attune_common::repositories::{
        execution_admission::ExecutionAdmissionRepository, FindById,
    };
    let (db, fixture) =
        fixture_with_approval_lifetime(None, None, Some(Duration::seconds(3))).await;
    let mut tx = db.begin().await.unwrap();
    let execution = ExecutionRepository::find_by_id(&mut *tx, fixture.credential.execution_id)
        .await
        .unwrap()
        .unwrap();
    let snapshot = execution.executable_snapshot.as_ref().unwrap();
    let inherited = ExecutionAdmissionRepository::inherit(
        &mut tx,
        RuntimeOwner::Execution(execution.id),
        snapshot,
    )
    .await
    .unwrap();
    let deadline = inherited.effect_fence().approval_deadline().unwrap();
    assert_eq!(Some(deadline), fixture.approval.expires_at);
    inherited.ensure_fresh_on(&mut tx).await.unwrap();
    // Real DB clock, no global clock override. This represents a wait after
    // selection. A final fence must deny even though the object was valid then.
    sqlx::query("SELECT pg_sleep(GREATEST(0, EXTRACT(EPOCH FROM ($1::timestamptz - clock_timestamp())))::double precision)")
        .bind(deadline).execute(&mut *tx).await.unwrap();
    assert!(inherited.ensure_fresh_on(&mut tx).await.is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

async fn fixture_with_release_content(
    defaults: Option<serde_json::Value>,
    content_path: Option<String>,
) -> (TestDatabase, Fixture) {
    fixture_with_approval_lifetime(defaults, content_path, None).await
}

async fn fixture_with_approval_lifetime(
    defaults: Option<serde_json::Value>,
    content_path: Option<String>,
    lifetime: Option<Duration>,
) -> (TestDatabase, Fixture) {
    let db = create_test_pool().await.unwrap();
    let shared = PackFixture::new("shared").create(&db).await.unwrap();
    let shared_trigger =
        helpers::TriggerFixture::new(Some(shared.id), Some(shared.r#ref), "completed")
            .create(&db)
            .await
            .unwrap();
    let pack = PackFixture::new_unique("runtime_authority")
        .create(&db)
        .await
        .unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "emit")
        .create(&db)
        .await
        .unwrap();
    if let Some(schema) = defaults {
        attune_common::repositories::action::ActionRepository::update(
            &db,
            action.id,
            attune_common::repositories::action::UpdateActionInput {
                param_schema: Some(schema),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let human = IdentityFixture::new_unique("runtime_issuer")
        .create(&db)
        .await
        .unwrap();
    let role = PermissionSetRepository::create(
        &db,
        CreatePermissionSetInput {
            r#ref: "fixture.approval_admin".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource": "permissions", "actions": ["manage"]}]),
        },
    )
    .await
    .unwrap();
    PermissionAssignmentRepository::create(
        &db,
        CreatePermissionAssignmentInput {
            identity: human.id,
            permset: role.id,
        },
    )
    .await
    .unwrap();

    let mut tx = db.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version,
            digest: "a".repeat(64),
            object_key: "packs/blobs/runtime-authority.tar.gz".into(),
            provider_version: "e:runtime-authority".into(),
            content_path: content_path
                .unwrap_or_else(|| "/packs/.releases/runtime-authority".into()),
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
            ..Default::default()
        },
    )
    .await
    .unwrap();
    RuleAuthorityRepository::capture_deployment(&mut tx, pack.id)
        .await
        .unwrap();
    let owner = RuntimeAuthorityRepository::find_owner(&mut tx, pack.id)
        .await
        .unwrap()
        .unwrap();
    let external_grant = serde_json::from_value(json!({
        "resource": "events", "actions": ["create"],
        "constraints": {"refs": ["shared.completed"]}
    }))
    .unwrap();
    let approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        pack.id,
        human.id,
        vec![external_grant],
        lifetime.map(|duration| Utc::now() + duration),
    )
    .await
    .unwrap();
    let set_ref = format!("{}.emit_events", pack.r#ref);
    let definition: PermissionSetDefinition = serde_json::from_value(json!({
        "ref": set_ref,
        "grants": [
            {"resource": "events", "actions": ["create"]},
            {"resource": "events", "actions": ["create"], "scope": "external",
                "constraints": {"refs": ["shared.completed"]}}
        ]
    }))
    .unwrap();
    let set = definition
        .compile(&owner, &[approval.approval().unwrap()])
        .unwrap();
    let binding = InvocationBinding::Action {
        action_ref: action.r#ref.clone(),
    };
    let policy = RuntimePolicy {
        concrete_targets: BTreeMap::new(),
        owner,
        release_id: release.id,
        release_digest: release.digest.clone(),
        invocations: vec![InvocationPolicy {
            dispatch_template_digest: None,
            source_id: None,
            metadata_revision: None,
            foreign_workflow_policy: None,
            binding: binding.clone(),
            target_release_id: release.id,
            target_release_digest: release.digest,
            permission_sets: BTreeMap::from([(set.reference.clone(), set)]),
            standard_access: true,
        }],
    };
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let selected = policy
        .policy
        .select(
            policy.id,
            &binding,
            std::slice::from_ref(&set_ref),
            RequestAttribution::Human {
                actor: HumanAttribution {
                    identity_id: human.id,
                    login: human.login,
                },
            },
        )
        .unwrap();
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id)
        .await
        .unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            executor: Some(human.id),
            permission_set_refs: vec![set_ref],
            status: ExecutionStatus::Running,
            config: Some(json!({})),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(execution.id),
        &selected,
    )
    .await
    .unwrap();
    let credential = RuntimeCredentialBinding {
        execution_id: execution.id,
        admission_id: admission.id,
        sponsor_identity_id: admission.authority.owner.sponsor_identity_id,
        action_ref: action.r#ref,
        attempt: admission.authority.attempt,
    };
    tx.commit().await.unwrap();
    (
        db,
        Fixture {
            external_pack_id: shared.id,
            external_trigger_id: shared_trigger.id,
            human_id: human.id,
            approval,
            policy,
            admission,
            credential,
        },
    )
}

#[tokio::test]
async fn admitted_local_and_external_effects_survive_human_freeze() {
    let (db, fixture) = fixture().await;
    IdentityRepository::update(
        &db,
        fixture.human_id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let validated =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await
            .unwrap();
    let target = attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve(
        &mut tx,
        attune_common::repositories::runtime_target::RuntimeTarget::EventTrigger(
            fixture.external_trigger_id,
        ),
        fixture.credential.sponsor_identity_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(validated.allows_target(&target, Action::Create));
    let mut context = AuthorizationContext::new(fixture.credential.sponsor_identity_id);
    context.target_ref = Some("shared.completed".into());
    assert!(!validated.allows(None, Resource::Events, Action::Create, &context));
    assert!(validated.allows(
        Some(fixture.policy.policy.owner.pack_id),
        Resource::Events,
        Action::Create,
        &context
    ));
    context.target_ref = Some("shared.other".into());
    assert!(!validated.allows(Some(9999), Resource::Events, Action::Create, &context));
    assert_ne!(fixture.human_id, fixture.credential.sponsor_identity_id);
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn authored_input_permits_bind_revision_path_expression_and_do_not_grant_api_access() {
    use attune_common::repositories::{
        declared_input::DeclaredInputRepository, managed_executable::ManagedExecutableRepository,
    };
    use attune_common::secret_provenance::SecretOrigin;
    let expression = "{{ keystore['pack.local'] }}";
    let (db, fixture) = fixture_with_input_defaults(Some(
        json!({"selected":{"type":"string","default":expression}}),
    ))
    .await;
    let owner = &fixture.policy.policy.owner;
    let local = helpers::KeyFixture::new_pack(owner.pack_id, &owner.pack_ref, "local", "value")
        .create(&db)
        .await
        .unwrap();
    let foreign = helpers::KeyFixture::new_system_unique("not_local", "value")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let binding = &fixture.policy.policy.invocations[0].binding;
    let snapshot =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &fixture.policy.policy, binding)
            .await
            .unwrap();
    let authority = fixture
        .policy
        .policy
        .select(
            fixture.policy.id,
            binding,
            &[],
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    assert!(!authority.has_api_access());
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            permission_set_refs: vec![],
            config: Some(json!({})),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(execution.id),
        &authority,
    )
    .await
    .unwrap();
    let bindings = DeclaredInputRepository::bindings(&mut tx, execution.id)
        .await
        .unwrap();
    let [declaration] = bindings.as_slice() else {
        panic!("expected one immutable declaration")
    };
    assert_eq!(declaration.source_path, "/param_schema/selected/default");
    assert!(DeclaredInputRepository::permit(
        &mut tx,
        execution.id,
        &"0".repeat(64),
        &declaration.source_path,
        expression
    )
    .await
    .is_err());
    assert!(DeclaredInputRepository::permit(
        &mut tx,
        execution.id,
        &declaration.declaration_revision,
        "/config/selected",
        expression
    )
    .await
    .is_err());
    assert!(DeclaredInputRepository::permit(
        &mut tx,
        execution.id,
        &declaration.declaration_revision,
        &declaration.source_path,
        "{{ keystore['system.other'] }}"
    )
    .await
    .is_err());
    let permit = DeclaredInputRepository::permit(
        &mut tx,
        execution.id,
        &declaration.declaration_revision,
        &declaration.source_path,
        expression,
    )
    .await
    .unwrap();
    DeclaredInputRepository::authorize_key(&mut tx, &permit, local.id, false)
        .await
        .unwrap();
    assert!(
        DeclaredInputRepository::authorize_key(&mut tx, &permit, foreign.id, false)
            .await
            .is_err()
    );
    let origin = SecretOrigin::Key(attune_common::key_access::key_origin(&local));
    DeclaredInputRepository::record_delivery(&mut tx, &permit, execution.id, "/selected", &origin)
        .await
        .unwrap();
    assert!(
        DeclaredInputRepository::delivery_allowed(&mut tx, execution.id, "/selected", &origin)
            .await
            .unwrap()
    );
    assert!(!DeclaredInputRepository::delivery_allowed(
        &mut tx,
        execution.id,
        "/supplied",
        &origin
    )
    .await
    .unwrap());
    RuntimeAuthorityRepository::revoke_admission(&mut tx, admission.id)
        .await
        .unwrap();
    assert!(
        DeclaredInputRepository::delivery_allowed(&mut tx, execution.id, "/selected", &origin)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn accepted_manual_input_bridge_seals_original_ciphertext_and_never_reresolves_key() {
    use attune_common::repositories::{
        accepted_input::AcceptedInputRepository as Accepted,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        key::KeyRepository,
        recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
        schema_contract::{BindingOwner, SchemaContractRepository},
    };
    use attune_common::secret_provenance::SecretProvenance;
    let (db,fixture)=fixture_with_input_defaults(Some(json!({"token":{"type":"string","secret":true},"other":{"type":"string","secret":true},"configuration":{"type":"string","secret":true}}))).await;
    let owner = &fixture.policy.policy.owner;
    let source_key =
        helpers::KeyFixture::new_pack(owner.pack_id, &owner.pack_ref, "initial", "accepted-value")
            .create(&db)
            .await
            .unwrap();
    let mut config_fixture = PackFixture::new_unique("accepted_configuration");
    config_fixture.conf_schema = json!({"token":{"type":"string","secret":true}});
    config_fixture.config = json!({"token":"configuration-value"});
    let config_pack = config_fixture.create(&db).await.unwrap();
    let set=PermissionSetRepository::create(&db,CreatePermissionSetInput {r#ref:"fixture.accepted_inputs".into(),pack:None,pack_ref:None,label:None,description:None,grants:json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[source_key.r#ref]}},{"resource":"packs","actions":["read","decrypt"],"constraints":{"refs":[config_pack.r#ref]}}])}).await.unwrap();
    PermissionAssignmentRepository::create(
        &db,
        CreatePermissionAssignmentInput {
            identity: fixture.human_id,
            permset: set.id,
        },
    )
    .await
    .unwrap();
    let key = ACCEPTED_MANUAL_TEST_KEY;
    let original = SecretProvenance::key(attune_common::key_access::key_origin(&source_key));
    let local = attune_common::secret_values::SecretSource::ParameterSchema {
        path: "/other".into(),
    }
    .provenance();
    let mut tx = db.begin().await.unwrap();
    let config_revision=attune_common::repositories::component_configuration::ComponentConfigurationRepository::bind_pack(&mut tx,&config_pack,Some(&json!({"token":"configuration-value"})),Some(key)).await.unwrap();
    let configuration = SecretProvenance {
        origins: vec![attune_common::secret_provenance::SecretOrigin::Entity {
            entity_type: "component_configuration".into(),
            entity_id: config_revision.id,
            path: "/token".into(),
        }],
        templates: vec![],
    };
    let values = attune_common::secret_values::prepare_secret_values(
        vec![
            attune_common::secret_values::SecretValueInput {
                json_path: "/token".into(),
                value: source_key.value.clone(),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&original).unwrap()),
            },
            attune_common::secret_values::SecretValueInput {
                json_path: "/other".into(),
                value: json!("{{ keystore['system.never_resolve'] }}"),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&local).unwrap()),
            },
            attune_common::secret_values::SecretValueInput {
                json_path: "/configuration".into(),
                value: json!("configuration-value"),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&configuration).unwrap()),
            },
        ],
        key,
    )
    .unwrap();
    let binding = &fixture.policy.policy.invocations[0].binding;
    let snapshot=attune_common::repositories::managed_executable::ManagedExecutableRepository::resolve_invocation(&mut tx,&fixture.policy.policy,binding).await.unwrap();
    let authority = fixture
        .policy
        .policy
        .select(
            fixture.policy.id,
            binding,
            &[],
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    let source=ExecutionRepository::create_pinned(&mut *tx,CreateExecutionInput {action:Some(snapshot.executable.action.id),action_ref:snapshot.executable.action.r#ref.clone(),status:ExecutionStatus::Running,permission_set_refs:vec![],config:Some(json!({"token":attune_common::secret_values::redaction_marker(),"other":attune_common::secret_values::redaction_marker(),"configuration":attune_common::secret_values::redaction_marker()})),..Default::default()},&snapshot).await.unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(source.id),
        &authority,
    )
    .await
    .unwrap();
    let schema = SchemaContractRepository::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, source.id, &schema)
        .await
        .unwrap();
    Contributors::capture_human(&mut tx, "execution", source.id, fixture.human_id)
        .await
        .unwrap();
    Secrets::upsert_many_with_conn(
        &mut tx,
        attune_common::secret_values::ENTITY_EXECUTION_CONFIG,
        source.id,
        &values,
    )
    .await
    .unwrap();
    Accepted::seal_execution(&mut tx, source.id, key)
        .await
        .unwrap();
    IdentityRepository::update(
        &mut *tx,
        fixture.human_id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    KeyRepository::delete(&mut *tx, source_key.id)
        .await
        .unwrap();
    // Idempotent sealing and first typed receipt work after Key deletion and
    // human freeze. Both use the original accepted ciphertext and manifests.
    Accepted::seal_execution(&mut tx, source.id, key)
        .await
        .unwrap();
    let site = InputSite {
        execution: source.id,
        kind: InputEntityKind::ExecutionConfig,
        purpose: InputSitePurpose::Value,
        path: "/token".into(),
    };
    let prepared = Accepted::prepare_source(&mut tx, source.id, &site, &original, key)
        .await
        .unwrap();
    assert_eq!(prepared.value(), &source_key.value);
    let protected = Inputs::receipt_context(&mut tx, &site, &prepared.origin(), key)
        .await
        .unwrap();
    assert!(
        protected.public_source_sites().is_empty(),
        "protected contributor inputs are never classified public from missing/current metadata"
    );
    protected.ensure_fresh().unwrap();
    assert_eq!(
        Accepted::prepare_source(
            &mut tx,
            source.id,
            &InputSite {
                path: "/configuration".into(),
                ..site.clone()
            },
            &configuration,
            key
        )
        .await
        .unwrap()
        .value(),
        &json!("configuration-value")
    );
    assert_eq!(
        Accepted::prepare_source(
            &mut tx,
            source.id,
            &InputSite {
                path: "/other".into(),
                ..site.clone()
            },
            &local,
            key
        )
        .await
        .unwrap()
        .value(),
        &json!("{{ keystore['system.never_resolve'] }}")
    );
    let parents = Inputs::retention_parents(
        &mut tx,
        match prepared.origin() {
            attune_common::secret_provenance::SecretOrigin::Entity { entity_id, .. } => entity_id,
            _ => unreachable!(),
        },
    )
    .await
    .unwrap();
    assert_eq!(parents.accepted_inputs().len(), 1);
    assert!(parents.origins().iter().any(|origin| matches!(
        origin,
        attune_common::secret_provenance::SecretOrigin::Key(_)
    )));
    for wrong in [
        InputSite {
            kind: InputEntityKind::ExecutionResult,
            ..site.clone()
        },
        InputSite {
            purpose: InputSitePurpose::Condition,
            ..site.clone()
        },
        InputSite {
            path: "/other".into(),
            ..site.clone()
        },
    ] {
        assert!(
            Accepted::prepare_source(&mut tx, source.id, &wrong, &original, key)
                .await
                .is_err()
        );
    }
    let mut injected = original.clone();
    injected
        .origins
        .push(attune_common::secret_provenance::SecretOrigin::Entity {
            entity_type: "component_configuration".into(),
            entity_id: 999999,
            path: String::new(),
        });
    assert!(
        Accepted::prepare_source(&mut tx, source.id, &site, &injected, key)
            .await
            .is_err()
    );
    assert!(
        Inputs::delivery_allowed(&mut tx, &site, &prepared.origin(), key)
            .await
            .unwrap(),
        "the actual stored initial field now has a typed delivery receipt even after Key deletion"
    );
    let stable = Secrets::find_stored_by_entity(
        &mut *tx,
        attune_common::secret_values::ENTITY_EXECUTION_CONFIG,
        source.id,
    )
    .await
    .unwrap()
    .into_iter()
    .map(|field| attune_common::secret_values::PreparedSecretValue {
        json_path: field.json_path,
        encrypted_value: field.encrypted_value,
        encryption_key_hash: field.encryption_key_hash.unwrap(),
        source_kind: field.source_kind,
        source_ref: field.source_ref,
    })
    .collect::<Vec<_>>();
    for field in &stable {
        assert_eq!(
            &field.encrypted_value,
            &values
                .iter()
                .find(|original| original.json_path == field.json_path)
                .unwrap()
                .encrypted_value,
            "first receipt substitution must preserve the original ciphertext"
        );
    }
    let mut changed = stable.clone();
    let token = changed
        .iter()
        .position(|field| field.json_path == "/token")
        .unwrap();
    changed[token].encrypted_value =
        attune_common::crypto::encrypt_json(&source_key.value, key).unwrap();
    Secrets::replace_entity_with_conn(
        &mut tx,
        attune_common::secret_values::ENTITY_EXECUTION_CONFIG,
        source.id,
        &changed,
    )
    .await
    .unwrap();
    assert!(
        Accepted::prepare_source(&mut tx, source.id, &site, &original, key)
            .await
            .is_err()
    );
    Secrets::replace_entity_with_conn(
        &mut tx,
        attune_common::secret_values::ENTITY_EXECUTION_CONFIG,
        source.id,
        &stable,
    )
    .await
    .unwrap();
    assert!(
        Accepted::prepare_source(&mut tx, source.id, &site, &original, key)
            .await
            .is_ok()
    );
    assert!(!authority.has_api_access());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn external_input_consent_can_bind_immutable_declaration_before_renderer_creation() {
    use attune_common::repositories::{
        declared_input::DeclaredInputRepository, inherited_input::InheritedInputRepository,
        managed_executable::ManagedExecutableRepository,
    };
    let expression = "{{ keystore[parameters.selector] }}";
    let (db, fixture) = fixture_with_input_defaults(Some(
        json!({"selected":{"type":"string","default":expression}}),
    ))
    .await;
    let mut tx = db.begin().await.unwrap();
    let binding = &fixture.policy.policy.invocations[0].binding;
    let snapshot =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &fixture.policy.policy, binding)
            .await
            .unwrap();
    let revision = fixture_digest(&serde_json::to_value(&snapshot.executable).unwrap());
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let grants =
        serde_json::from_value(json!([{"resource":"keys","actions":["read","decrypt"]}])).unwrap();
    let approval = InheritedInputRepository::approve_external_declaration(
        &mut tx,
        fixture.policy.id,
        binding,
        &revision,
        "/param_schema/selected/default",
        expression,
        fixture.human_id,
        grants,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        RuntimeAuthorityRepository::find_approval(&mut tx, approval)
            .await
            .unwrap()
            .unwrap()
            .subject_pack,
        fixture.policy.policy.owner.pack_id
    );
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        before, after,
        "approving a declaration must not create or invoke an execution"
    );
    assert!(DeclaredInputRepository::declaration_for_policy(
        &mut tx,
        fixture.policy.id,
        binding,
        &revision,
        "/param_schema/other/default",
        expression
    )
    .await
    .is_err());
    assert!(DeclaredInputRepository::declaration_for_policy(
        &mut tx,
        fixture.policy.id,
        binding,
        &revision,
        "/param_schema/selected/default",
        "{{ keystore['system.changed'] }}"
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn purpose_receipts_preserve_external_input_consent_without_api_grants_or_key_rereads() {
    use attune_common::repositories::{
        accepted_input::AcceptedInputRepository as Accepted,
        declared_input::DeclaredInputRepository,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        key::KeyRepository,
        managed_executable::ManagedExecutableRepository,
        recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
        schema_contract::{BindingOwner, SchemaContractRepository},
    };
    let expression = "{{ keystore[parameters.selector] }}";
    let inherited = "{{ result().token }}";
    let (db, fixture) = fixture_with_input_defaults(Some(json!({
        "selector":{"type":"string"},
        "selected":{"type":"string","default":expression},
        "copied":{"type":"string","default":inherited}
    })))
    .await;
    let foreign = helpers::KeyFixture::new_system_unique("input_consent", "{{ supplied_literal }}")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let binding = &fixture.policy.policy.invocations[0].binding;
    let snapshot =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &fixture.policy.policy, binding)
            .await
            .unwrap();
    let authority = fixture
        .policy
        .policy
        .select(
            fixture.policy.id,
            binding,
            &[],
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    assert!(
        matches!(&authority.attribution,RequestAttribution::Human {actor} if actor.identity_id==fixture.human_id)
    );
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            permission_set_refs: vec![],
            config: Some(json!({"selector":foreign.r#ref})),
            executor: Some(fixture.human_id),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(execution.id),
        &authority,
    )
    .await
    .unwrap();
    let input_schema=attune_common::repositories::schema_contract::SchemaContractRepository::materialize_action_pair(&mut tx,&snapshot).await.unwrap();
    attune_common::repositories::schema_contract::SchemaContractRepository::attach(
        &mut tx,
        attune_common::repositories::schema_contract::BindingOwner::Execution,
        execution.id,
        &input_schema,
    )
    .await
    .unwrap();
    let declarations = DeclaredInputRepository::bindings(&mut tx, execution.id)
        .await
        .unwrap();
    let selected = declarations
        .iter()
        .find(|declaration| declaration.expression == expression)
        .unwrap();
    let copied = declarations
        .iter()
        .find(|declaration| declaration.expression == inherited)
        .unwrap();
    let key = PURPOSE_RECEIPT_TEST_KEY;
    let mut denied_session = Inputs::begin_render(
        &mut tx,
        execution.id,
        &selected.declaration_revision,
        &selected.source_path,
        expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    assert!(
        Inputs::bind_key(&mut tx, &mut denied_session, &foreign.r#ref, key)
            .await
            .is_err()
    );
    let grants:Vec<attune_common::rbac::Grant>=serde_json::from_value(json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[foreign.r#ref]}}])).unwrap();
    let approval = Inputs::approve_external_input(
        &mut tx,
        execution.id,
        &selected.declaration_revision,
        &selected.source_path,
        expression,
        fixture.human_id,
        grants.clone(),
        Some(Utc::now() + Duration::hours(1)),
    )
    .await
    .unwrap();
    assert!(RuntimeAuthorityRepository::admit_external_grants(
        &mut tx,
        fixture.policy.policy.owner.pack_id,
        fixture.human_id,
        &grants
    )
    .await
    .is_err());
    // Capture the actual original creator's input consent before freezing them.
    // Descendants inherit this accepted snapshot, never a fresh human capture.
    Contributors::capture_human(&mut tx, "execution", execution.id, fixture.human_id)
        .await
        .unwrap();
    Accepted::seal_execution(&mut tx, execution.id, key)
        .await
        .unwrap();
    IdentityRepository::update(
        &mut *tx,
        fixture.human_id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        execution.id,
        &selected.declaration_revision,
        &selected.source_path,
        expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_public_parameter(&mut tx, &mut session, "/selector")
        .await
        .unwrap();
    let rendered = Inputs::evaluate_with_keys(&mut tx, session, key)
        .await
        .unwrap();
    assert_eq!(rendered.value(), &foreign.value);
    let result_site = InputSite {
        execution: execution.id,
        kind: InputEntityKind::ExecutionResult,
        purpose: InputSitePurpose::Value,
        path: "/token".into(),
    };
    let prepared = Inputs::prepare_destination(&mut tx, &rendered, &result_site, key)
        .await
        .unwrap();
    assert!(
        Inputs::record_prepared(&mut tx, &rendered, &prepared, rendered.value(), &[], key)
            .await
            .is_err()
    );
    let receipt = Inputs::record_prepared(
        &mut tx,
        &rendered,
        &prepared,
        rendered.value(),
        rendered.selections(),
        key,
    )
    .await
    .unwrap();
    receipt.ensure_fresh().unwrap();
    assert_eq!(
        receipt.public_source_sites(),
        &[InputSite {
            execution: execution.id,
            kind: InputEntityKind::ExecutionConfig,
            purpose: InputSitePurpose::Value,
            path: "/selector".into()
        }]
    );
    assert!(receipt.outcome_metadata().is_empty());
    assert!(
        Inputs::prepare_recorded_source(&mut tx, execution.id, &receipt, key)
            .await
            .is_err(),
        "receipt alone must not claim an unpersisted value field"
    );
    let wrong_kind = InputSite {
        kind: InputEntityKind::ExecutionConfig,
        ..result_site.clone()
    };
    assert!(
        !Inputs::delivery_allowed(&mut tx, &wrong_kind, &receipt.origin(), key)
            .await
            .unwrap()
    );
    assert!(
        Inputs::receipt_context(&mut tx, &wrong_kind, &receipt.origin(), key)
            .await
            .is_err()
    );
    ExecutionRepository::update(
        &mut *tx,
        execution.id,
        UpdateExecutionInput {
            result: Some(json!({"token":attune_common::secret_values::redaction_marker()})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let persisted = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/token".into(),
            value: foreign.value.clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&receipt.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::upsert_many_with_conn(&mut tx,attune_common::secret_values::ENTITY_EXECUTION_RESULT,execution.id,&persisted).await.unwrap();
    let self_source = Inputs::prepare_recorded_source(&mut tx, execution.id, &receipt, key)
        .await
        .unwrap();
    assert_eq!(self_source.value(), &foreign.value);
    let reread = Inputs::receipt_context(&mut tx, &result_site, &receipt.origin(), key)
        .await
        .unwrap();
    assert!(
        Inputs::prepare_recorded_source(&mut tx, execution.id, &reread, key)
            .await
            .is_err(),
        "a reread context is not the actual newly issued publication context"
    );
    // The Key may disappear after its exact read-time owner and approval were
    // recorded. Inheritance validates the fixed proof, never another Key lookup.
    KeyRepository::delete(&mut *tx, foreign.id).await.unwrap();
    let mut descendants = Vec::new();
    let mut parent = execution.id;
    for index in 0..2 {
        let descendant = ExecutionRepository::create_pinned(
            &mut *tx,
            CreateExecutionInput {
                action: Some(snapshot.executable.action.id),
                action_ref: snapshot.executable.action.r#ref.clone(),
                status: ExecutionStatus::Running,
                parent: Some(parent),
                permission_set_refs: vec![],
                config: Some(if index == 1 {
                    json!({"copied":attune_common::secret_values::redaction_marker()})
                } else {
                    json!({})
                }),
                ..Default::default()
            },
            &snapshot,
        )
        .await
        .unwrap();
        let authority = fixture
            .policy
            .policy
            .select(
                fixture.policy.id,
                binding,
                &[],
                RequestAttribution::ParentExecution {
                    execution_id: parent,
                },
            )
            .unwrap();
        RuntimeAuthorityRepository::record_admission(
            &mut tx,
            RuntimeOwner::Execution(descendant.id),
            &authority,
        )
        .await
        .unwrap();
        Contributors::copy(&mut tx, "execution", parent, "execution", descendant.id)
            .await
            .unwrap();
        SchemaContractRepository::attach(
            &mut tx,
            BindingOwner::Execution,
            descendant.id,
            &input_schema,
        )
        .await
        .unwrap();
        let fresh_capture =
            Contributors::capture_human(&mut tx, "execution", descendant.id, fixture.human_id)
                .await
                .unwrap_err();
        assert!(matches!(fresh_capture,attune_common::Error::PermissionDenied(message) if message=="Input requester is not active"),"a new capture must still reject the frozen requester; only original accepted consent can be inherited");
        if index == 0 {
            Accepted::seal_execution(&mut tx, descendant.id, key)
                .await
                .unwrap();
        }
        parent = descendant.id;
        descendants.push(descendant);
    }
    let source = Inputs::prepare_source(
        &mut tx,
        descendants[0].id,
        &result_site,
        &receipt.origin(),
        key,
    )
    .await
    .unwrap();
    let mut next = Inputs::begin_render(
        &mut tx,
        descendants[0].id,
        &copied.declaration_revision,
        &copied.source_path,
        inherited,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_prepared(&mut next, "/result/token", &source, &foreign.value).unwrap();
    let propagated = Inputs::evaluate(&mut tx, next).await.unwrap();
    assert_eq!(propagated.value(), &foreign.value);
    let destination = InputSite {
        execution: descendants[1].id,
        kind: InputEntityKind::ExecutionConfig,
        purpose: InputSitePurpose::Value,
        path: "/copied".into(),
    };
    let inherited_receipt = Inputs::record_render(
        &mut tx,
        &propagated,
        &destination,
        propagated.value(),
        propagated.selections(),
        key,
    )
    .await
    .unwrap();
    assert_eq!(
        inherited_receipt.public_source_sites(),
        receipt.public_source_sites(),
        "verified public source classification survives typed inheritance"
    );
    assert!(inherited_receipt.source_origins().contains(
        &attune_common::secret_provenance::SecretOrigin::Key(
            attune_common::key_access::key_origin(&foreign)
        )
    ));
    // Seal the ACTUAL destination field. Its live row now carries a different
    // first-delivery receipt, while the archive retains this original typed
    // source at precisely the same config site. Rediscovering via that live row
    // would loop back to the first receipt instead of its original parent.
    // The immutable redacted config shape was set at creation. Its exact secret
    // receipt and ciphertext are attached before this creation transaction ends.
    let original_child_fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: destination.path.clone(),
            value: foreign.value.clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&inherited_receipt.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(
        &mut tx,
        attune_common::secret_values::ENTITY_EXECUTION_CONFIG,
        destination.execution,
        &original_child_fields,
    )
    .await
    .unwrap();
    Accepted::seal_execution(&mut tx, destination.execution, key)
        .await
        .unwrap();
    let delivered = Secrets::find_stored_by_entity(
        &mut *tx,
        attune_common::secret_values::ENTITY_EXECUTION_CONFIG,
        destination.execution,
    )
    .await
    .unwrap();
    let delivered_provenance: attune_common::secret_provenance::SecretProvenance =
        serde_json::from_str(delivered[0].source_ref.as_deref().unwrap()).unwrap();
    let first_origin = delivered_provenance.origins[0].clone();
    assert_ne!(first_origin, inherited_receipt.origin());
    assert_eq!(
        delivered[0].encrypted_value, original_child_fields[0].encrypted_value,
        "sealing must retain the original ciphertext bytes"
    );
    assert!(
        Inputs::delivery_allowed(&mut tx, &destination, &first_origin, key)
            .await
            .unwrap()
    );
    let first = Inputs::receipt_context(&mut tx, &destination, &first_origin, key)
        .await
        .unwrap();
    assert!(
        first.source_origins().contains(&inherited_receipt.origin()),
        "do not discard the accepted field's original typed parent to avoid a cycle"
    );
    let original_source = first.source_receipt(&inherited_receipt.origin()).unwrap();
    assert_eq!(original_source.site(), &destination);
    assert_eq!(original_source.origin(), inherited_receipt.origin());
    assert_ne!(original_source.origin(), first_origin);
    let first_id = match &first_origin {
        attune_common::secret_provenance::SecretOrigin::Entity { entity_id, .. } => *entity_id,
        _ => unreachable!(),
    };
    assert_eq!(original_source.retained_by(), first_id);
    assert!(
        first.source_receipt(&first_origin).is_err(),
        "the root cannot become its own source descriptor"
    );
    original_source.ensure_fresh_on(&mut tx).await.unwrap();
    let original_context = Inputs::prepare_receipt_source(&mut tx, original_source, key)
        .await
        .unwrap();
    assert_eq!(original_context.site(), &destination);
    assert_eq!(original_context.origin(), inherited_receipt.origin());
    assert!(
        original_context.source_origins().contains(
            &attune_common::secret_provenance::SecretOrigin::Key(
                attune_common::key_access::key_origin(&foreign)
            )
        ),
        "original source identity/reader obligations survive descriptor preparation"
    );
    assert!(
        !original_context.source_origins().contains(&first_origin),
        "preparation must not rediscover the live replacement first receipt"
    );
    assert!(
        Inputs::prepare_receipt_source(&mut tx, original_source, "wrong-original-source-key")
            .await
            .is_err()
    );
    let mut unrelated_origin = inherited_receipt.origin();
    if let attune_common::secret_provenance::SecretOrigin::Entity { entity_id, .. } =
        &mut unrelated_origin
    {
        *entity_id += 1000000;
    }
    assert!(first.source_receipt(&unrelated_origin).is_err());
    let mut wrong_purpose = inherited_receipt.origin();
    if let attune_common::secret_provenance::SecretOrigin::Entity { entity_type, .. } =
        &mut wrong_purpose
    {
        *entity_type = "declared_input:execution_config:control".into();
    }
    assert!(
        first.source_receipt(&wrong_purpose).is_err(),
        "a source ID cannot establish a different purpose's descriptor"
    );
    assert!(
        !RuntimeAuthorityRepository::execution_admission(&mut tx, execution.id)
            .await
            .unwrap()
            .unwrap()
            .authority
            .has_api_access()
    );
    tx.commit().await.unwrap();
    let mut gc = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut gc)
        .await
        .unwrap();
    ExecutionRepository::delete(&mut *gc, execution.id)
        .await
        .unwrap();
    gc.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    assert!(
        Inputs::prepare_receipt_source(&mut tx, original_source, key)
            .await
            .is_err(),
        "an opaque original-source descriptor cannot cross transaction boundaries"
    );
    let retained_first = Inputs::receipt_context(&mut tx, &destination, &first_origin, key)
        .await
        .unwrap();
    let retained_source = retained_first
        .source_receipt(&inherited_receipt.origin())
        .unwrap();
    assert!(Inputs::prepare_receipt_source(&mut tx,retained_source,key).await.is_ok(),"reacquire the authenticated root and original descriptor after retention, not the live config's first receipt");
    assert!(
        Inputs::receipt_context(&mut tx, &destination, &inherited_receipt.origin(), key)
            .await
            .is_ok()
    );
    RuntimeAuthorityRepository::revoke_approval(&mut tx, approval)
        .await
        .unwrap();
    assert!(
        Inputs::prepare_receipt_source(&mut tx, retained_source, key)
            .await
            .is_err(),
        "original selected input approval revocation still propagates through accepted descriptors"
    );
    assert!(
        Inputs::receipt_context(&mut tx, &destination, &first_origin, key)
            .await
            .is_err()
    );
    assert!(
        Inputs::receipt_context(&mut tx, &destination, &inherited_receipt.origin(), key)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut different_transaction = db.begin().await.unwrap();
    assert!(inherited_receipt
        .ensure_fresh_on(&mut different_transaction)
        .await
        .is_err());
    different_transaction.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn foreign_workflow_handoff_pins_publisher_policy_graph_metadata_and_revocation() {
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        managed_component_readmission::ManagedComponentReadmission,
        managed_executable::ManagedExecutableRepository,
        workflow::{CreateWorkflowDefinitionInput, WorkflowDefinitionRepository},
        workflow_policy::WorkflowPolicyRepository,
    };
    let (db, fixture) = fixture().await;
    let publisher = PackFixture::new_unique("publisher_graph")
        .create(&db)
        .await
        .unwrap();
    let workflow = ActionFixture::new(publisher.id, &publisher.r#ref, "graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(publisher.id, &publisher.r#ref, "leaf")
        .create(&db)
        .await
        .unwrap();
    let graph_set_ref = format!("{}.external_effects", publisher.r#ref);
    let graph = WorkflowDefinitionRepository::create(&db,CreateWorkflowDefinitionInput {
        r#ref:workflow.r#ref.clone(),pack:publisher.id,pack_ref:publisher.r#ref.clone(),label:"Graph".into(),description:None,
        version:"1.0.0".into(),param_schema:None,out_schema:None,tags:vec![],
        definition:json!({"vars":{"literal":{"allowed":true}},"tasks":[{"name":"leaf","action":leaf.r#ref,"with_items":"{{ [1, 2, 3] }}","batch_size":2,"params":{"token":"{{ keystore['pack.token'] }}"},"permission_set_refs":[graph_set_ref.clone()]}]}),
    }).await.unwrap();
    ActionRepository::link_workflow_def(&db, workflow.id, graph.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = activate_release(
        &mut tx,
        publisher.id,
        &publisher.r#ref,
        vec![workflow.id, leaf.id],
        'b',
    )
    .await;
    let owner = RuntimeAuthorityRepository::ensure_owner(&mut tx, publisher.id)
        .await
        .unwrap();
    let graph_approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        publisher.id,
        fixture.human_id,
        fixture.approval.grants.0.clone(),
        Some(Utc::now() + Duration::hours(1)),
    )
    .await
    .unwrap();
    let graph_set = attune_common::runtime_authority::AdmittedPermissionSet::external(
        &graph_set_ref,
        &owner,
        &graph_approval.grants,
        &[graph_approval.approval().unwrap()],
    )
    .unwrap();
    let graph_sets = BTreeMap::from([(graph_set_ref.clone(), graph_set)]);
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let bindings = [
        InvocationBinding::Action {
            action_ref: workflow.r#ref.clone(),
        },
        InvocationBinding::Action {
            action_ref: leaf.r#ref.clone(),
        },
        InvocationBinding::WorkflowTask {
            workflow_ref: workflow.r#ref.clone(),
            task_path: "/tasks/leaf".into(),
            action_ref: leaf.r#ref.clone(),
        },
    ];
    let graph_policy = RuntimePolicy {
        owner,
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: BTreeMap::new(),
        invocations: bindings
            .iter()
            .map(|binding| {
                let refs = if matches!(binding, InvocationBinding::WorkflowTask { .. }) {
                    vec![graph_set_ref.clone()]
                } else {
                    vec![]
                };
                InvocationPolicy::from_executable(
                    binding.clone(),
                    &snapshots[binding.action_ref()],
                    Some(&refs),
                    &graph_sets,
                )
                .unwrap()
            })
            .collect(),
    };
    let admitted = RuntimeAuthorityRepository::capture_policy(&mut tx, &graph_policy)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, admitted.id)
        .await
        .unwrap();
    let native_snapshot =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &admitted.policy, &bindings[0])
            .await
            .unwrap();
    let native_authority = admitted
        .policy
        .select(
            admitted.id,
            &bindings[0],
            &[],
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    let native = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(workflow.id),
            action_ref: workflow.r#ref.clone(),
            status: ExecutionStatus::Running,
            permission_set_refs: vec![],
            config: Some(json!({})),
            ..Default::default()
        },
        &native_snapshot,
    )
    .await
    .unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(native.id),
        &native_authority,
    )
    .await
    .unwrap();
    let native_revision =
        fixture_digest(&serde_json::to_value(&native_snapshot.executable).unwrap());
    use attune_common::repositories::inherited_input::{
        InheritedInputRepository as Inputs, InputSitePurpose,
    };
    let declarations =
        attune_common::repositories::declared_input::DeclaredInputRepository::bindings(
            &mut tx, native.id,
        )
        .await
        .unwrap();
    let with_items = declarations
        .iter()
        .find(|declaration| declaration.source_path == "/workflow/tasks/0/with_items")
        .unwrap();
    let session = Inputs::begin_render(
        &mut tx,
        native.id,
        &with_items.declaration_revision,
        &with_items.source_path,
        &with_items.expression,
        InputSitePurpose::Control,
    )
    .await
    .unwrap();
    let array = Inputs::evaluate(&mut tx, session).await.unwrap();
    assert_eq!(array.value(), &json!([1, 2, 3]));
    let item = Inputs::prepare_iteration_item(&mut tx, &array, 1, Some(2), NATIVE_ITEM_TEST_KEY)
        .await
        .unwrap();
    assert_eq!(item.value(), &json!([3]));
    assert_eq!(
        item.index(),
        1,
        "index is the actual batch ordinal, not an element offset"
    );
    assert_eq!(item.source_site().path, with_items.source_path);
    assert!(
        Inputs::prepare_iteration_item(&mut tx, &array, 2, Some(2), NATIVE_ITEM_TEST_KEY)
            .await
            .is_err()
    );
    assert!(
        Inputs::prepare_iteration_item(&mut tx, &array, 0, None, NATIVE_ITEM_TEST_KEY)
            .await
            .is_err(),
        "the caller cannot replace the retained task's batch shape"
    );
    let wrong_purpose = Inputs::begin_render(
        &mut tx,
        native.id,
        &with_items.declaration_revision,
        &with_items.source_path,
        &with_items.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    let wrong_purpose = Inputs::evaluate(&mut tx, wrong_purpose).await.unwrap();
    assert!(
        Inputs::prepare_iteration_item(&mut tx, &wrong_purpose, 0, None, NATIVE_ITEM_TEST_KEY)
            .await
            .is_err()
    );
    let literal = attune_common::repositories::inherited_input::InheritedInputRepository::literal(
        &mut tx,
        native.id,
        &native_revision,
        "/workflow/vars/literal",
    )
    .await
    .unwrap();
    assert_eq!(literal.value(), &json!({"allowed":true}));
    assert!(literal.selections().is_empty());
    assert!(
        attune_common::repositories::inherited_input::InheritedInputRepository::literal(
            &mut tx,
            native.id,
            &native_revision,
            "/workflow/tasks/0/params/token"
        )
        .await
        .is_err()
    );
    assert!(
        attune_common::repositories::inherited_input::InheritedInputRepository::literal(
            &mut tx,
            native.id,
            &"0".repeat(64),
            "/workflow/vars/literal"
        )
        .await
        .is_err()
    );
    let queue = owned_queue(
        &mut tx,
        &fixture.policy.policy.owner,
        workflow.id,
        &workflow.r#ref,
    )
    .await;
    sqlx::query("UPDATE work_queue SET managed_release = $1 WHERE id = $2")
        .bind(fixture.policy.policy.release_id)
        .bind(queue.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let binding = InvocationBinding::Queue {
        queue_ref: queue.r#ref.clone(),
        action_ref: workflow.r#ref.clone(),
    };
    let mut outer = fixture.policy.policy.0.clone();
    outer.invocations.push(
        InvocationPolicy::from_executable(
            binding.clone(),
            &snapshots[&workflow.r#ref],
            Some(&[]),
            &BTreeMap::new(),
        )
        .unwrap(),
    );
    let outer = RuntimeAuthorityRepository::capture_policy(&mut tx, &outer)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, outer.id)
        .await
        .unwrap();
    let pin = outer
        .policy
        .invocations
        .last()
        .unwrap()
        .foreign_workflow_policy
        .as_ref()
        .unwrap();
    assert_eq!(pin.revision, admitted.id);
    let selected = outer
        .policy
        .select(
            outer.id,
            &binding,
            &[],
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    assert!(!selected.has_api_access());
    let graph_fence = RuntimeAuthorityRepository::selected_effect_fence(&mut tx, &selected)
        .await
        .unwrap();
    assert_eq!(graph_fence.approval_deadline(),graph_approval.expires_at,
        "a credential-free outer selection still retains its pinned publisher graph approval expiry");
    assert!(!outer.policy.invocations.iter().any(|entry|matches!(&entry.binding,InvocationBinding::WorkflowTask {workflow_ref,..} if workflow_ref == &workflow.r#ref)));
    let mut replacement = admitted.policy.0.clone();
    replacement.invocations[0].standard_access = true;
    let replacement = RuntimeAuthorityRepository::capture_policy(&mut tx, &replacement)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, replacement.id)
        .await
        .unwrap();
    ActionRepository::update(
        &mut *tx,
        workflow.id,
        UpdateActionInput {
            param_schema: Some(json!({"new_input":{"type":"string"}})),
            default_execution_permission_set_refs: Some(vec!["standard".into()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let replacement =
        ManagedExecutableRepository::readmit_action(&mut tx, workflow.id, fixture.human_id)
            .await
            .unwrap()
            .unwrap();
    let pinned = ManagedExecutableRepository::resolve_invocation(&mut tx, &outer.policy, &binding)
        .await
        .unwrap();
    assert!(pinned.pack_executables.contains_key(&leaf.r#ref));
    let retained = WorkflowPolicyRepository::for_invocation(&mut tx, &outer, &binding, &pinned)
        .await
        .unwrap();
    assert_eq!(retained.id, admitted.id);
    assert_ne!(retained.id, replacement.id);
    assert_eq!(retained.policy.owner.pack_id, publisher.id);
    attune_common::repositories::work_queue::WorkQueueRepository::update(
        &mut *tx,
        queue.id,
        attune_common::repositories::work_queue::UpdateWorkQueueInput {
            config: Some(json!({"dispatch":{"separate_requesters":true}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let readmitted = ManagedComponentReadmission::queue(&mut tx, queue.id, fixture.human_id)
        .await
        .unwrap()
        .unwrap();
    let retained_invocation = readmitted
        .policy
        .invocations
        .iter()
        .find(|entry| entry.binding == binding)
        .unwrap();
    assert!(!retained_invocation.standard_access);
    assert_eq!(
        retained_invocation
            .foreign_workflow_policy
            .as_ref()
            .unwrap()
            .revision,
        admitted.id
    );
    let still_pinned =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &readmitted.policy, &binding)
            .await
            .unwrap();
    assert_eq!(
        serde_json::to_value(&still_pinned.executable).unwrap(),
        serde_json::to_value(&pinned.executable).unwrap()
    );
    retained
        .policy
        .invocation_for_retained_workflow(&bindings[2], &pinned)
        .unwrap();
    let mut forged = pinned.clone();
    forged.executable.action.id += 1;
    assert!(
        WorkflowPolicyRepository::for_invocation(&mut tx, &outer, &binding, &forged)
            .await
            .is_err()
    );
    RuntimeAuthorityRepository::revoke_policy(&mut tx, admitted.id)
        .await
        .unwrap();
    assert!(
        RuntimeAuthorityRepository::require_policy(&mut tx, outer.id)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn child_admission_keeps_parent_revocation_without_requiring_parent_running() {
    use attune_common::repositories::managed_executable::ManagedExecutableRepository;
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let binding = &fixture.policy.policy.invocations[0].binding;
    let snapshot =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &fixture.policy.policy, binding)
            .await
            .unwrap();
    let selected = fixture
        .policy
        .policy
        .select(
            fixture.policy.id,
            binding,
            &[],
            RequestAttribution::ParentExecution {
                execution_id: fixture.admission.owner_id,
            },
        )
        .unwrap();
    let child = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            parent: Some(fixture.admission.owner_id),
            status: ExecutionStatus::Running,
            permission_set_refs: vec![],
            config: Some(json!({})),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(child.id),
        &selected,
    )
    .await
    .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        fixture.admission.owner_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(child.id),
        &selected,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    // Reclamation must enter a fresh exclusive graph gate, before any source
    // validation locks. It cannot upgrade the preceding read transaction.
    let mut gc = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut gc)
        .await
        .unwrap();
    ExecutionRepository::delete(&mut *gc, fixture.admission.owner_id)
        .await
        .unwrap();
    gc.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(child.id),
        &selected,
    )
    .await
    .unwrap();
    RuntimeAuthorityRepository::revoke_admission(&mut tx, fixture.admission.id)
        .await
        .unwrap();
    assert!(RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(child.id),
        &selected
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn bounded_proof_collection_skips_oldest_file_pins_and_releases_after_exact_cleanup() {
    use attune_common::repositories::{
        file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        pack_retention::PackRetentionRepository,
    };
    let (db, fixture) = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    // Seed dormant collector records directly. Their execution rows have already
    // been retained away. Receipt authentication is covered by separate tests;
    // this fixture exercises candidate bounds and physical cleanup ownership.
    let mut admissions = Vec::new();
    for owner in [900001_i64, 900002, 900003] {
        let id: i64 = sqlx::query_scalar("INSERT INTO runtime_authority_admission(owner_kind,owner_id,policy_revision,authority,authority_digest) SELECT 'execution',$1,policy_revision,authority,authority_digest FROM runtime_authority_admission WHERE id=$2 RETURNING id")
            .bind(owner).bind(fixture.admission.id).fetch_one(&mut *tx).await.unwrap();
        admissions.push(id);
    }
    let mut receipts = Vec::new();
    let key = PROOF_COLLECTION_TEST_KEY;
    for (owner, admission) in [(900002_i64, admissions[1]), (900003, admissions[2])] {
        let proof = json!({"site":{"execution":owner,"kind":"execution_result","purpose":"value","path":"/token"},"renderer":admission,"declaration":null,"literal":null,"selections":[]});
        let receipt: i64 = sqlx::query_scalar("INSERT INTO runtime_input_receipt(execution,entity_kind,destination_purpose,destination_path,destination_admission,renderer_admission,proof,proof_digest,encrypted_value,encryption_key_hash) VALUES($1,'execution_result','value','/token',$2,$2,$3,$4,$5,$6) RETURNING id")
            .bind(owner).bind(admission).bind(&proof).bind(fixture_digest(&proof))
            .bind(attune_common::crypto::encrypt_json(&json!("held"),key).unwrap())
            .bind(attune_common::crypto::hash_encryption_key(key))
            .fetch_one(&mut *tx).await.unwrap();
        receipts.push(receipt);
    }
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_key,staging_path) VALUES(900010,900011,900012,'collector/900010','collector/staging/900010')")
        .execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) VALUES(900010,'runtime_authority_admission',$1),(900010,'runtime_input_receipt',$2)")
        .bind(admissions[0]).bind(receipts[0]).execute(&mut *tx).await.unwrap();
    let mut snapshots = Vec::new();
    for _ in 0..2 {
        let id:i64=sqlx::query_scalar("INSERT INTO runtime_native_input_snapshot(renderer_admission,policy_revision,body,body_digest,encrypted_value,encryption_key_hash) VALUES($1,$2,'{}',$3,$4,$5) RETURNING id")
            .bind(admissions[0]).bind(fixture.policy.id).bind(fixture_digest(&json!({}))).bind(attune_common::crypto::encrypt_json(&json!("collector-only-fixture"),key).unwrap()).bind(attune_common::crypto::hash_encryption_key(key)).fetch_one(&mut *tx).await.unwrap();
        snapshots.push(id);
    }
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) VALUES(900010,'runtime_native_input_snapshot',$1)").bind(snapshots[0]).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    let collect = || PackRetentionRepository::collect(&db, root.path(), chrono::Utc::now(), 0, 1);
    collect().await.unwrap();
    let retained: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM runtime_native_input_snapshot WHERE id=ANY($1) ORDER BY id",
    )
    .bind(&snapshots)
    .fetch_all(&*db)
    .await
    .unwrap();
    assert_eq!(
        retained,
        vec![snapshots[0]],
        "LIMIT 1 must skip the oldest pinned native snapshot rather than spin on its guard"
    );
    let retained: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM runtime_input_receipt WHERE id=ANY($1) ORDER BY id")
            .bind(&receipts)
            .fetch_all(&*db)
            .await
            .unwrap();
    assert_eq!(
        retained,
        vec![receipts[0]],
        "LIMIT 1 must skip the oldest pinned receipt and reclaim the later unpinned receipt"
    );
    let retained: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM runtime_authority_admission WHERE id=ANY($1) ORDER BY id",
    )
    .bind(&admissions)
    .fetch_all(&*db)
    .await
    .unwrap();
    assert_eq!(retained,admissions[..2],"LIMIT 1 must skip the oldest pinned admission while preserving the pinned receipt's admission");

    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    assert_eq!(
        Evidence::request_cleanup(&mut tx, &[900010]).await.unwrap(),
        vec![900010]
    );
    sqlx::query("UPDATE file_evidence_owner SET metadata_gone=true WHERE version=900010")
        .execute(&mut *tx)
        .await
        .unwrap();
    Evidence::object_removed(&mut tx, "collector/900010", None)
        .await
        .unwrap();
    assert!(
        !Evidence::eligible(&mut tx, "runtime_input_receipt", receipts[0])
            .await
            .unwrap(),
        "object completion alone cannot release a remaining staging obligation"
    );
    Evidence::staging_removed(&mut tx, "collector/staging/wrong", 900010)
        .await
        .unwrap();
    assert!(
        !Evidence::eligible(&mut tx, "runtime_authority_admission", admissions[0])
            .await
            .unwrap(),
        "the wrong staging path must not release evidence"
    );
    tx.commit().await.unwrap();
    collect().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM runtime_input_receipt WHERE id=$1")
            .bind(receipts[0])
            .fetch_one(&*db)
            .await
            .unwrap(),
        1
    );

    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    Evidence::staging_removed(&mut tx, "collector/staging/900010", 900010)
        .await
        .unwrap();
    assert!(
        Evidence::eligible(&mut tx, "runtime_input_receipt", receipts[0])
            .await
            .unwrap()
    );
    assert!(
        Evidence::eligible(&mut tx, "runtime_authority_admission", admissions[0])
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    // One bounded pass reclaims the receipt and first admission. Its remaining
    // now-unreferenced admission is reclaimed on the next pass.
    collect().await.unwrap();
    collect().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM runtime_native_input_snapshot WHERE id=ANY($1)"
        )
        .bind(&snapshots)
        .fetch_one(&*db)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM runtime_input_receipt WHERE id=ANY($1)")
            .bind(&receipts)
            .fetch_one(&*db)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM runtime_authority_admission WHERE id=ANY($1)"
        )
        .bind(&admissions)
        .fetch_one(&*db)
        .await
        .unwrap(),
        0
    );
    root.close().unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn system_snapshot_uses_original_execution_creation_and_is_transaction_bound() {
    use attune_common::repositories::{
        declared_input::DeclaredInputRepository,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
    };
    let expression = "{{ system.workflow_start }}";
    let (db, fixture) = fixture_with_input_defaults(Some(
        json!({"started":{"type":"string","default":expression}}),
    ))
    .await;
    let execution = fixture.admission.owner_id;
    let mut tx = db.begin().await.unwrap();
    let declarations = DeclaredInputRepository::bindings(&mut tx, execution)
        .await
        .unwrap();
    let declaration = declarations
        .iter()
        .find(|declaration| declaration.expression == expression)
        .unwrap();
    let system = Inputs::prepare_system(&mut tx, execution).await.unwrap();
    let again = Inputs::prepare_system(&mut tx, execution).await.unwrap();
    let created: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT created FROM execution WHERE id=$1")
            .bind(execution)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(system.value(), &json!({"workflow_start":created}));
    assert_eq!(
        again.value(),
        system.value(),
        "reconstruction cannot manufacture a later workflow start"
    );
    let mut session = Inputs::begin_render(
        &mut tx,
        execution,
        &declaration.declaration_revision,
        &declaration.source_path,
        expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_prepared_system(&mut session, &system).unwrap();
    let rendered = Inputs::evaluate(&mut tx, session).await.unwrap();
    assert_eq!(rendered.value(), &json!(created));
    let site = InputSite {
        execution,
        kind: InputEntityKind::ExecutionResult,
        purpose: InputSitePurpose::Value,
        path: "/started".into(),
    };
    let receipt = Inputs::record_render(
        &mut tx,
        &rendered,
        &site,
        rendered.value(),
        rendered.selections(),
        SYSTEM_SNAPSHOT_TEST_KEY,
    )
    .await
    .unwrap();
    receipt.ensure_fresh().unwrap();
    assert!(
        receipt.public_source_sites().is_empty(),
        "system metadata is not a public config/result field exception"
    );
    assert!(receipt.outcome_metadata().is_empty());
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        execution,
        &declaration.declaration_revision,
        &declaration.source_path,
        expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    assert!(
        Inputs::bind_prepared_system(&mut session, &system).is_err(),
        "an old transaction snapshot must be prepared again"
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn staged_values_bind_canonical_same_advance_variables_and_require_actual_publication() {
    staged_value_publication_case(false, None).await;
}

#[tokio::test]
async fn staged_condition_controls_dependent_value_without_publishing_a_boolean_variable() {
    staged_value_publication_case(true, None).await;
}

#[tokio::test]
async fn staged_condition_retains_each_original_approval_expiry_across_target_wait() {
    for original in 0..2 {
        staged_value_publication_case(true, Some(original)).await;
    }
}

async fn staged_value_publication_case(with_condition: bool, expiring_original: Option<usize>) {
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        declared_input::DeclaredInputRepository,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            WorkflowDefinitionRepository, WorkflowExecutionRepository,
        },
    };
    let (db, fixture) = fixture().await;
    let owner = &fixture.policy.policy.owner;
    let key = STAGED_VALUE_TEST_KEY;
    let literal = json!("{{ keystore['system.must_not_be_reparsed'] }}");
    let source_key =
        helpers::KeyFixture::new_system_unique("staged_source", literal.as_str().unwrap())
            .create(&db)
            .await
            .unwrap();
    let gate_key = helpers::KeyFixture::new_system_unique("staged_gate", "yes")
        .create(&db)
        .await
        .unwrap();
    let first_expression = if with_condition {
        "{{ keystore[parameters.supplied] }}"
    } else {
        "{{ parameters.supplied }}"
    };
    let condition_expression = "{{ first != null and keystore[parameters.gate] }}";
    let graph_action = ActionFixture::new(owner.pack_id, &owner.pack_ref, "staged_graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(owner.pack_id, &owner.pack_ref, "staged_leaf")
        .create(&db)
        .await
        .unwrap();
    let authored = staged_value_fixture::definition(&leaf.r#ref, with_condition);
    authored.assert_coherent_and_compiles();
    ActionRepository::update(
        &db,
        graph_action.id,
        UpdateActionInput {
            param_schema: Some(authored.action_input.clone()),
            out_schema: Some(authored.action_output.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let definition = authored.graph.clone();
    let graph = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: graph_action.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "Staged publication".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: Some(authored.workflow_input.clone()),
            out_schema: Some(authored.workflow_output.clone()),
            tags: vec![],
            definition: definition.clone(),
        },
    )
    .await
    .unwrap();
    ActionRepository::link_workflow_def(&db, graph_action.id, graph.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = activate_release(
        &mut tx,
        owner.pack_id,
        &owner.pack_ref,
        vec![graph_action.id, leaf.id],
        'b',
    )
    .await;
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let snapshot = &snapshots[&graph_action.r#ref];
    let root_binding = InvocationBinding::Action {
        action_ref: graph_action.r#ref.clone(),
    };
    let policy = RuntimePolicy {
        owner: owner.clone(),
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: BTreeMap::new(),
        invocations: vec![InvocationPolicy::from_executable(
            root_binding.clone(),
            snapshot,
            Some(&[]),
            &BTreeMap::new(),
        )
        .unwrap()],
    };
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let root = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(graph_action.id),
            action_ref: graph_action.r#ref.clone(),
            config: Some(if with_condition {
                json!({"supplied":source_key.r#ref,"gate":gate_key.r#ref})
            } else {
                json!({"supplied":literal})
            }),
            executor: Some(fixture.human_id),
            status: ExecutionStatus::Running,
            permission_set_refs: vec![],
            ..Default::default()
        },
        snapshot,
    )
    .await
    .unwrap();
    let authority = policy
        .policy
        .select(
            policy.id,
            &root_binding,
            &[],
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(root.id),
        &authority,
    )
    .await
    .unwrap();
    let schema=attune_common::repositories::schema_contract::SchemaContractRepository::materialize_action_pair(&mut tx,snapshot).await.unwrap();
    attune_common::repositories::schema_contract::SchemaContractRepository::attach(
        &mut tx,
        attune_common::repositories::schema_contract::BindingOwner::Execution,
        root.id,
        &schema,
    )
    .await
    .unwrap();
    let workflow = WorkflowExecutionRepository::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: root.id,
            workflow_def: graph.id,
            task_graph: definition,
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
        .await
        .unwrap();
    let declaration = |expression: &str| {
        declarations
            .iter()
            .find(|declaration| declaration.expression == expression)
            .unwrap()
    };
    let first_binding = declaration(first_expression);
    let mut original_approvals = Vec::new();
    let expiry = Utc::now() + Duration::seconds(10);
    if with_condition {
        for (index, (binding, source)) in [
            (first_binding, &source_key),
            (declaration(condition_expression), &gate_key),
        ]
        .into_iter()
        .enumerate()
        {
            let grants = serde_json::from_value(json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[source.r#ref]}}])).unwrap();
            original_approvals.push(
                Inputs::approve_external_input(
                    &mut tx,
                    root.id,
                    &binding.declaration_revision,
                    &binding.source_path,
                    &binding.expression,
                    fixture.human_id,
                    grants,
                    Some(if expiring_original == Some(index) {
                        expiry
                    } else {
                        Utc::now() + Duration::hours(1)
                    }),
                )
                .await
                .unwrap(),
            );
        }
        assert_ne!(
            original_approvals[0], original_approvals[1],
            "original declarations retain independent consents"
        );
    }
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &first_binding.declaration_revision,
        &first_binding.source_path,
        &first_binding.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_public_parameter(&mut tx, &mut session, "/supplied")
        .await
        .unwrap();
    let rendered = Inputs::evaluate_with_keys(&mut tx, session, key)
        .await
        .unwrap();
    if with_condition {
        assert!(rendered.selections().iter().any(|selected| matches!(
            &selected.source,
            attune_common::repositories::inherited_input::SelectionSource::Authored { input_authority: Some(id), .. } if *id == original_approvals[0]
        )));
    }
    let variable_site = |name: &str| InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Value,
        path: format!("/{name}"),
    };
    let first = Inputs::prepare_staged_value(&mut tx, &rendered, &variable_site("first"), key)
        .await
        .unwrap();
    assert_eq!(first.value(), &literal);
    let control = if with_condition {
        let mut successful_control = None;
        for (expression, succeeds) in [(condition_expression, true), ("{{ first == null }}", false)]
        {
            let binding = declaration(expression);
            let mut session = Inputs::begin_render(
                &mut tx,
                root.id,
                &binding.declaration_revision,
                &binding.source_path,
                &binding.expression,
                InputSitePurpose::Condition,
            )
            .await
            .unwrap();
            Inputs::bind_staged_value(&mut session, "/first", &first).unwrap();
            if succeeds {
                Inputs::bind_public_parameter(&mut tx, &mut session, "/gate")
                    .await
                    .unwrap();
            }
            let condition = Inputs::evaluate_condition_with_keys(&mut tx, session, key)
                .await
                .unwrap();
            assert_eq!(condition.value(), &json!(succeeds));
            if succeeds {
                assert!(condition.selections().iter().any(|selected| matches!(
                    &selected.source,
                    attune_common::repositories::inherited_input::SelectionSource::Authored { input_authority: Some(id), .. } if *id == original_approvals[1]
                )));
                assert!(!condition.selections().iter().any(|selected| matches!(
                    &selected.source,
                    attune_common::repositories::inherited_input::SelectionSource::Authored { input_authority: Some(id), .. } if *id == original_approvals[0]
                )), "earlier consent is inherited, never flattened into the condition declaration");
            }
            let site = InputSite {
                purpose: InputSitePurpose::Condition,
                path: binding.source_path.clone(),
                ..variable_site("unused")
            };
            assert!(Inputs::prepare_staged_value(
                &mut tx,
                &condition,
                &variable_site("fake_boolean"),
                key
            )
            .await
            .is_err());
            let wrong = InputSite {
                path: declaration(if succeeds {
                    "{{ first == null }}"
                } else {
                    condition_expression
                })
                .source_path
                .clone(),
                ..site.clone()
            };
            assert!(
                Inputs::prepare_condition_control(&mut tx, &condition, &wrong, key)
                    .await
                    .is_err()
            );
            let prepared_condition = Inputs::prepare_destination(&mut tx, &condition, &site, key)
                .await
                .unwrap();
            assert!(Inputs::record_prepared(
                &mut tx,
                &condition,
                &prepared_condition,
                condition.value(),
                condition.selections(),
                key
            )
            .await
            .is_err());
            let prepared = Inputs::prepare_condition_control(&mut tx, &condition, &site, key).await;
            if succeeds {
                let prepared = prepared.unwrap();
                assert!(Inputs::receipt_context(&mut tx, prepared.site(), &prepared.origin(), key).await.is_err(), "normal replay cannot disclose pending Condition ancestry even in its own transaction");
                assert!(Inputs::delivery_allowed(
                    &mut tx,
                    prepared.site(),
                    &prepared.origin(),
                    key
                )
                .await
                .is_err());
                successful_control = Some(prepared);
            } else {
                assert!(
                    prepared.is_err(),
                    "a failed condition cannot issue an effect control"
                );
            }
        }
        successful_control
    } else {
        None
    };
    let second_binding = declaration("{{ first }}");
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &second_binding.declaration_revision,
        &second_binding.source_path,
        &second_binding.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    assert!(Inputs::bind_staged_value(&mut session, "/keystore", &first).is_err());
    Inputs::bind_staged_value(&mut session, "/first", &first).unwrap();
    if let Some(control) = &control {
        assert!(
            Inputs::bind_prepared(&mut session, "/fake_boolean", control, control.value()).is_err()
        );
        Inputs::attach_prepared_control(&mut session, control).unwrap();
    }
    let second_render = Inputs::evaluate(&mut tx, session).await.unwrap();
    if let Some(control) = &control {
        assert!(second_render.selections().iter().any(|selected| matches!(
            &selected.source,
            attune_common::repositories::inherited_input::SelectionSource::Control { site, .. } if site == control.site()
        )));
    }
    let second =
        Inputs::prepare_staged_value(&mut tx, &second_render, &variable_site("second"), key)
            .await
            .unwrap();
    let output_binding = declaration("{{ second }}");
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &output_binding.declaration_revision,
        &output_binding.source_path,
        &output_binding.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_staged_value(&mut session, "/second", &second).unwrap();
    let output = Inputs::evaluate(&mut tx, session).await.unwrap();
    assert_eq!(
        output.value(),
        &literal,
        "staged text must not be interpolated"
    );
    let result_site = InputSite {
        kind: InputEntityKind::ExecutionResult,
        path: "/token".into(),
        ..variable_site("unused")
    };
    let prepared = Inputs::prepare_destination(&mut tx, &output, &result_site, key)
        .await
        .unwrap();
    assert!(
        Inputs::record_prepared(
            &mut tx,
            &output,
            &prepared,
            output.value(),
            output.selections(),
            key
        )
        .await
        .is_err(),
        "pending stages are renderable before targets but not publishable"
    );
    assert!(
        Inputs::finalize_staged_value(&mut tx, &first, key)
            .await
            .is_err(),
        "an absent original variable is not a publication"
    );
    let release_blocker = if expiring_original.is_some() {
        let mut blocker = db.begin().await.unwrap();
        sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
            .bind(workflow.id)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let waiter_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let pool = (*db).clone();
        Some(tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(60), async {
                let mut observed_wait = false;
                loop {
                    let blocked: bool = sqlx::query_scalar("SELECT $2 = ANY(pg_blocking_pids($1))")
                        .bind(waiter_pid)
                        .bind(blocker_pid)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                    let expired: bool = sqlx::query_scalar("SELECT clock_timestamp() >= $1")
                        .bind(expiry)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                    if blocked && !observed_wait {
                        assert!(
                            !expired,
                            "approval expired before the observed target-wait barrier"
                        );
                        observed_wait = true;
                    }
                    if observed_wait && blocked && expired {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            blocker.rollback().await.unwrap();
        }))
    } else {
        None
    };
    tokio::time::timeout(std::time::Duration::from_secs(70),
        sqlx::query("UPDATE workflow_execution SET variables=$2 WHERE id=$1").bind(workflow.id).bind(json!({"first":attune_common::secret_values::redaction_marker(),"second":attune_common::secret_values::redaction_marker()})).execute(&mut *tx)
    ).await.unwrap().unwrap();
    if let Some(release_blocker) = release_blocker {
        release_blocker.await.unwrap();
    }
    let fields = attune_common::secret_values::prepare_secret_values(
        vec![
            attune_common::secret_values::SecretValueInput {
                json_path: "/first".into(),
                value: literal.clone(),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&first.provenance()).unwrap()),
            },
            attune_common::secret_values::SecretValueInput {
                json_path: "/second".into(),
                value: literal.clone(),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&second.provenance()).unwrap()),
            },
        ],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(&mut tx, "workflow_variables", root.id, &fields)
        .await
        .unwrap();
    if let Some(original) = expiring_original {
        assert_eq!(
            first.ensure_fresh_on(&mut tx).await.is_err(),
            original == 0,
            "the condition consent does not become authority on the earlier VALUE"
        );
        if original == 0 {
            assert!(Inputs::finalize_staged_value(&mut tx, &first, key)
                .await
                .is_err());
        } else {
            Inputs::finalize_staged_value(&mut tx, &first, key)
                .await
                .unwrap();
        }
        assert!(
            second.ensure_fresh_on(&mut tx).await.is_err(),
            "either original expiry fences the controlled VALUE"
        );
        assert!(Inputs::finalize_staged_value(&mut tx, &second, key)
            .await
            .is_err());
        assert!(Inputs::record_prepared(
            &mut tx,
            &output,
            &prepared,
            output.value(),
            output.selections(),
            key
        )
        .await
        .is_err());
        tx.rollback().await.unwrap();
        db.cleanup().await.unwrap();
        return;
    }
    assert!(
        Inputs::finalize_staged_value(&mut tx, &second, key)
            .await
            .is_err(),
        "all transitive publications must be finalized in dependency order"
    );
    Inputs::finalize_staged_value(&mut tx, &first, key)
        .await
        .unwrap();
    if let Some(control) = &control {
        let replayed = Inputs::receipt_context(&mut tx, control.site(), &control.origin(), key)
            .await
            .unwrap();
        assert_eq!(replayed.site(), control.site());
        assert_eq!(replayed.source_receipts().len(), 1);
    }
    assert!(
        Inputs::record_prepared(
            &mut tx,
            &output,
            &prepared,
            output.value(),
            output.selections(),
            key
        )
        .await
        .is_err(),
        "one finalized ancestor is not the whole source closure"
    );
    Inputs::finalize_staged_value(&mut tx, &second, key)
        .await
        .unwrap();
    assert!(Inputs::record_prepared(
        &mut tx,
        &output,
        &prepared,
        &json!("caller replacement"),
        output.selections(),
        key
    )
    .await
    .is_err());
    let receipt = Inputs::record_prepared(
        &mut tx,
        &output,
        &prepared,
        output.value(),
        output.selections(),
        key,
    )
    .await
    .unwrap();
    assert!(receipt
        .public_source_sites()
        .iter()
        .any(|site| site.kind == InputEntityKind::ExecutionConfig && site.path == "/supplied"));
    assert_eq!(
        receipt.source_receipts().len(),
        if with_condition { 3 } else { 2 }
    );
    assert!(receipt.outcome_metadata().is_empty());
    assert!(!authority.has_api_access());
    let unpublished =
        Inputs::prepare_staged_value(&mut tx, &rendered, &variable_site("unpublished"), key)
            .await
            .unwrap();
    let unpublished_control = if with_condition {
        let binding = declaration(condition_expression);
        let mut session = Inputs::begin_render(
            &mut tx,
            root.id,
            &binding.declaration_revision,
            &binding.source_path,
            &binding.expression,
            InputSitePurpose::Condition,
        )
        .await
        .unwrap();
        Inputs::bind_staged_value(&mut session, "/first", &unpublished).unwrap();
        Inputs::bind_public_parameter(&mut tx, &mut session, "/gate")
            .await
            .unwrap();
        let condition = Inputs::evaluate_condition_with_keys(&mut tx, session, key)
            .await
            .unwrap();
        let site = InputSite {
            purpose: InputSitePurpose::Condition,
            path: binding.source_path.clone(),
            ..variable_site("unused")
        };
        Some(
            Inputs::prepare_condition_control(&mut tx, &condition, &site, key)
                .await
                .unwrap(),
        )
    } else {
        None
    };
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &output_binding.declaration_revision,
        &output_binding.source_path,
        &output_binding.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    assert!(
        Inputs::bind_staged_value(&mut session, "/second", &second).is_err(),
        "staging cannot cross transactions"
    );
    if let Some(control) = &control {
        assert!(
            Inputs::attach_prepared_control(&mut session, control).is_err(),
            "held controls cannot cross transactions"
        );
        assert!(
            Inputs::receipt_context(&mut tx, control.site(), &control.origin(), key)
                .await
                .is_ok(),
            "published immutable control ancestry replays in a later transaction"
        );
    }
    if let Some(control) = &unpublished_control {
        assert!(Inputs::attach_prepared_control(&mut session, control).is_err());
        assert!(Inputs::receipt_context(&mut tx, control.site(), &control.origin(), key).await.is_err(), "committed pending CONTROL ancestry cannot be replayed or disclosed in another transaction");
    }
    assert!(
        Inputs::receipt_context(
            &mut tx,
            unpublished.site(),
            &unpublished.provenance().origins[0],
            key
        )
        .await
        .is_err(),
        "a committed but unpublished staging record cannot become immutable source evidence later"
    );
    assert!(
        Inputs::receipt_context(&mut tx, &result_site, &receipt.origin(), key)
            .await
            .is_ok()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_cache_snapshot_reads_pinned_pages_and_rejects_stale_cursor_or_transaction() {
    native_cache_reader_policy_case(false, 3600).await;
}

async fn run_native_cache_fixture(case: Option<native_cache_checkpoint::Case>) {
    native_cache_reader_policy_case_with_variant(false, 3600, case).await;
}

#[tokio::test]
async fn native_cache_reader_requires_exact_active_generation_even_without_a_deadline() {
    native_cache_reader_policy_case(true, 0).await;
}

#[tokio::test]
async fn native_cache_reader_retains_original_finite_freshness_deadline() {
    native_cache_reader_policy_case(true, 3600).await;
}

async fn native_cache_reader_policy_case(require_fresh: bool, freshness_seconds: i64) {
    native_cache_reader_policy_case_with_variant(require_fresh, freshness_seconds, None).await;
}

async fn native_cache_reader_policy_case_with_variant(
    require_fresh: bool,
    freshness_seconds: i64,
    case: Option<native_cache_checkpoint::Case>,
) {
    use attune_common::{
        config::CacheAdmissionConfig,
        repositories::{
            cache::{
                CacheEntryInput, CacheEntryRepository, CacheGenerationRepository,
                CacheIngestRepository, CacheNamespacePolicy, CacheNamespaceRepository,
                CacheOwnerScope, CacheTransactionMode, CreateCacheGenerationInput,
                CreateCacheGenerationResult, CreateCacheNamespaceInput,
            },
            declared_input::DeclaredInputRepository,
            execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
            inherited_input::{
                InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
            },
            managed_executable::ManagedExecutableRepository,
            native_input_snapshot::NativeInputSnapshotRepository as Snapshots,
            runtime_target::{ComponentTarget, RuntimeTarget},
            workflow::{
                CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
                WorkflowDefinitionRepository, WorkflowExecutionRepository,
            },
            workflow_cache_iteration::{
                UpdateWorkflowCacheIterationProgressInput, WorkflowCacheIterationRepository,
            },
        },
    };
    let (db, fixture) = fixture().await;
    let owner = &fixture.policy.policy.owner;
    let graph_action = ActionFixture::new(owner.pack_id, &owner.pack_ref, "cached_graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(owner.pack_id, &owner.pack_ref, "cached_leaf")
        .create(&db)
        .await
        .unwrap();
    attune_common::repositories::action::ActionRepository::update(
        &db,
        graph_action.id,
        attune_common::repositories::action::UpdateActionInput {
            param_schema: Some(json!({"cache_name":{"type":"string"}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    if case.is_some() {
        attune_common::repositories::action::ActionRepository::update(
            &db,
            leaf.id,
            attune_common::repositories::action::UpdateActionInput {
                param_schema: Some(json!({"token":{"type":"object"},"ordinal":{"type":"integer"}})),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let mut definition = json!({"tasks":[{"name":"cached","action":leaf.r#ref,"iterate_cache":{"namespace":"{{ parameters.cache_name }}","page_size":1,"require_fresh":require_fresh},"batch_size":2,"permission_set_refs":["standard"],"params":{"token":"{{ item[0].value }}","ordinal":"{{ index }}"}}]});
    if !require_fresh {
        definition["tasks"][0]["iterate_cache"]
            .as_object_mut()
            .unwrap()
            .remove("require_fresh");
    }
    if let Some(case) = case {
        native_cache_checkpoint::configure(case, &mut definition);
    }
    let producer = if case.is_some_and(native_cache_checkpoint::Case::activation) {
        let producer =
            ActionFixture::new(owner.pack_id, &owner.pack_ref, "cache_selector_producer")
                .create(&db)
                .await
                .unwrap();
        native_cache_activation::configure(case.unwrap(), &mut definition, &producer.r#ref);
        Some(producer)
    } else {
        None
    };
    let graph = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: graph_action.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "Cache proof".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            tags: vec![],
            definition: definition.clone(),
        },
    )
    .await
    .unwrap();
    attune_common::repositories::action::ActionRepository::link_workflow_def(
        &db,
        graph_action.id,
        graph.id,
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let mut release_actions = vec![graph_action.id, leaf.id];
    release_actions.extend(producer.as_ref().map(|action| action.id));
    let release = activate_release(
        &mut tx,
        owner.pack_id,
        &owner.pack_ref,
        release_actions,
        'c',
    )
    .await;
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let root_binding = InvocationBinding::Action {
        action_ref: graph_action.r#ref.clone(),
    };
    let task_binding = InvocationBinding::WorkflowTask {
        workflow_ref: graph_action.r#ref.clone(),
        task_path: "/tasks/cached".into(),
        action_ref: leaf.r#ref.clone(),
    };
    let mut policy = RuntimePolicy {
        owner: owner.clone(),
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: BTreeMap::new(),
        invocations: vec![
            InvocationPolicy::from_executable(
                root_binding.clone(),
                &snapshots[&graph_action.r#ref],
                Some(&[]),
                &BTreeMap::new(),
            )
            .unwrap(),
            InvocationPolicy::from_executable(
                task_binding.clone(),
                &snapshots[&leaf.r#ref],
                Some(&["standard".into()]),
                &BTreeMap::new(),
            )
            .unwrap(),
        ],
    };
    if let Some(producer) = &producer {
        for (name, action, refs) in [
            ("choose", producer, vec![]),
            ("cached2", &leaf, vec!["standard".into()]),
        ] {
            policy.invocations.push(
                InvocationPolicy::from_executable(
                    InvocationBinding::WorkflowTask {
                        workflow_ref: graph_action.r#ref.clone(),
                        task_path: format!("/tasks/{name}"),
                        action_ref: action.r#ref.clone(),
                    },
                    &snapshots[&action.r#ref],
                    Some(&refs),
                    &BTreeMap::new(),
                )
                .unwrap(),
            );
        }
    }
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let snapshot =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &policy.policy, &root_binding)
            .await
            .unwrap();
    let authority = policy
        .policy
        .select(
            policy.id,
            &root_binding,
            &[],
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    let mut root = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(graph_action.id),
            action_ref: graph_action.r#ref.clone(),
            executor: case.map(|_| fixture.human_id),
            status: if case.is_some() {
                ExecutionStatus::Requested
            } else {
                ExecutionStatus::Running
            },
            config: case.is_none().then(|| json!({"cache_name":"native-input"})),
            permission_set_refs: vec![],
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let root_admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(root.id),
        &authority,
    )
    .await
    .unwrap();
    let schema=attune_common::repositories::schema_contract::SchemaContractRepository::materialize_action_pair(&mut tx,&snapshot).await.unwrap();
    attune_common::repositories::schema_contract::SchemaContractRepository::attach(
        &mut tx,
        attune_common::repositories::schema_contract::BindingOwner::Execution,
        root.id,
        &schema,
    )
    .await
    .unwrap();
    if case.is_some() {
        assert!(matches!(&root_admission.authority.attribution,
            RequestAttribution::Human { actor } if actor.identity_id == fixture.human_id));
        assert!(
            root.config.is_none(),
            "caller cases must initialize their original input exactly once"
        );
        root = ExecutionRepository::initialize_config_with_conn(
            &mut tx,
            root.id,
            Some(root_admission.id),
            &snapshot,
            Some(json!({"cache_name":"native-input"})),
            &[],
        )
        .await
        .unwrap();
        // Runtime admission is not input acceptance. Capture this actual human
        // supplier before the first seal, so children inherit a genuine ceiling.
        attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(
            &mut tx, "execution", root.id, fixture.human_id,
        ).await.unwrap();
        attune_common::repositories::declared_input_materialization::DeclaredInputMaterializationRepository::seal_execution_inputs(
            &mut tx, root.id, NATIVE_CACHE_TEST_KEY).await.unwrap();
        let accepted: (serde_json::Value, i64, bool) = sqlx::query_as(
            "SELECT body->'config',jsonb_array_length(body->'contributors')::BIGINT,e.transaction_id=pg_current_xact_id()::TEXT FROM runtime_accepted_input a JOIN runtime_accepted_input_epoch e ON e.admission=a.admission WHERE a.execution=$1",
        ).bind(root.id).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(accepted, (json!({"cache_name":"native-input"}), 1, true),
            "the first seal must retain the original flat input and its real contributor in the creation transaction");
        root = ExecutionRepository::update(
            &mut *tx,
            root.id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Running),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let workflow = WorkflowExecutionRepository::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: root.id,
            workflow_def: graph.id,
            task_graph: definition,
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    let completed_producer = if let Some(producer) = &producer {
        Some(
            native_cache_activation::create_producer(
                &mut tx,
                &root,
                &workflow,
                &snapshots[&producer.r#ref],
                NATIVE_CACHE_TEST_KEY,
            )
            .await,
        )
    } else {
        None
    };
    tx.commit().await.unwrap();
    let namespace = CacheNamespaceRepository::create_api_with_policy(
        &db,
        CreateCacheNamespaceInput {
            mode: Default::default(),
            owner: CacheOwnerScope::pack(owner.pack_id, Some(owner.pack_ref.clone())),
            namespace: "native-input".into(),
            policy: CacheNamespacePolicy {
                freshness_target_seconds: freshness_seconds,
                ..Default::default()
            },
        },
        &CacheAdmissionConfig::default(),
    )
    .await
    .unwrap();
    let generation = CacheGenerationRepository::create_or_get(
        &db,
        &CreateCacheGenerationInput {
            namespace: namespace.id,
            client_refresh_id: "native-input-owned".into(),
            expected_active_generation: None,
            expected_chunk_count: 1,
            expected_count: Some(if case.is_some_and(native_cache_checkpoint::Case::empty) {
                0
            } else {
                5
            }),
            expected_bytes: None,
            checksum_algorithm: None,
            checksum: None,
            source_revision: None,
            created_by: None,
            created_by_execution: None,
        },
    )
    .await
    .unwrap();
    let CreateCacheGenerationResult::Created(generation) = generation else {
        panic!("owned generation must be new")
    };
    let entries = ["a", "b", "c", "d", "e"]
        .into_iter()
        .filter(|_| !case.is_some_and(native_cache_checkpoint::Case::empty))
        .map(|id| CacheEntryInput {
            external_id: id.into(),
            value: json!({"token":if id=="a" {"{{ keystore['system.never_resolve'] }}"}else {id}}),
            source_updated_at: None,
            source_checksum: Some(format!("source-{id}")),
        })
        .collect::<Vec<_>>();
    CacheIngestRepository::insert_chunk(
        &db,
        generation.id,
        0,
        "native-input-owned-chunk",
        &entries,
    )
    .await
    .unwrap();
    CacheGenerationRepository::seal(&db, generation.id)
        .await
        .unwrap();
    CacheGenerationRepository::promote(
        &db,
        namespace.id,
        generation.id,
        None,
        Utc::now() + Duration::hours(1),
    )
    .await
    .unwrap();
    if let Some(case) = case {
        if let Some(producer) = &completed_producer {
            native_cache_activation::check(case, &db, &root, &workflow, producer).await;
        } else {
            native_cache_checkpoint::check(case, &db, &root, &workflow, fixture.human_id).await;
        }
        db.cleanup().await.unwrap();
        return;
    }
    let replacement = CacheGenerationRepository::create_or_get(
        &db,
        &CreateCacheGenerationInput {
            namespace: namespace.id,
            client_refresh_id: "native-reader-replacement".into(),
            expected_active_generation: Some(generation.id),
            expected_chunk_count: 1,
            expected_count: Some(1),
            expected_bytes: None,
            checksum_algorithm: None,
            checksum: None,
            source_revision: None,
            created_by: None,
            created_by_execution: None,
        },
    )
    .await
    .unwrap();
    let CreateCacheGenerationResult::Created(replacement) = replacement else {
        panic!("replacement generation must be new")
    };
    CacheIngestRepository::insert_chunk(
        &db,
        replacement.id,
        0,
        "native-reader-replacement-chunk",
        &[CacheEntryInput {
            external_id: "replacement".into(),
            value: json!({"token":"wrong-generation"}),
            source_updated_at: None,
            source_checksum: None,
        }],
    )
    .await
    .unwrap();
    CacheGenerationRepository::seal(&db, replacement.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let task = Admissions::select(
        &mut tx,
        owner.pack_id,
        task_binding.clone(),
        &snapshots[&leaf.r#ref],
        &["standard".into()],
        RequestAttribution::ParentExecution {
            execution_id: root.id,
        },
        RuntimeTarget::Component(ComponentTarget::Action, graph_action.id),
    )
    .await
    .unwrap();
    assert!(
        Snapshots::prepare_cache_initial(
            &mut tx,
            root.id,
            "/workflow/tasks/0",
            &task,
            workflow.id,
            65536,
            &[],
            NATIVE_CACHE_TEST_KEY
        )
        .await
        .is_err(),
        "a dynamic cache selector cannot be certified from its raw iteration row alone"
    );
    let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
        .await
        .unwrap();
    let selector = declarations
        .iter()
        .find(|declaration| declaration.source_path == "/workflow/tasks/0/iterate_cache/namespace")
        .unwrap();
    let mut selector_session = Inputs::begin_render(
        &mut tx,
        root.id,
        &selector.declaration_revision,
        &selector.source_path,
        &selector.expression,
        InputSitePurpose::Control,
    )
    .await
    .unwrap();
    Inputs::bind_public_parameter(&mut tx, &mut selector_session, "/cache_name")
        .await
        .unwrap();
    let selector_render = Inputs::evaluate(&mut tx, selector_session).await.unwrap();
    let control_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Control,
        path: selector.source_path.clone(),
    };
    let control = Inputs::record_render(
        &mut tx,
        &selector_render,
        &control_site,
        selector_render.value(),
        selector_render.selections(),
        NATIVE_CACHE_TEST_KEY,
    )
    .await
    .unwrap();
    let prepared = Snapshots::prepare_cache_initial(
        &mut tx,
        root.id,
        "/workflow/tasks/0",
        &task,
        workflow.id,
        65536,
        &[&control],
        NATIVE_CACHE_TEST_KEY,
    )
    .await
    .unwrap();
    let next = Snapshots::prepare_cache_next(&mut tx, &prepared, 65536, NATIVE_CACHE_TEST_KEY)
        .await
        .unwrap();
    let next2 = Snapshots::prepare_cache_next(&mut tx, &next, 65536, NATIVE_CACHE_TEST_KEY)
        .await
        .unwrap();
    let eof = Snapshots::prepare_cache_next(&mut tx, &next2, 65536, NATIVE_CACHE_TEST_KEY)
        .await
        .unwrap();
    assert_eq!(
        (next.index(), next.count(), next.after_external_id()),
        (1, 2, Some("d"))
    );
    assert_eq!(
        (next2.index(), next2.count(), next2.after_external_id()),
        (2, 1, Some("e"))
    );
    assert_eq!(
        (
            eof.index(),
            eof.count(),
            eof.after_external_id(),
            eof.value()
        ),
        (3, 0, Some("e"), None)
    );
    assert!(eof.exhausted());
    assert!(
        Snapshots::prepare_cache_next(&mut tx, &eof, 65536, NATIVE_CACHE_TEST_KEY)
            .await
            .is_err()
    );
    assert_eq!(prepared.count(), 2);
    assert_eq!(prepared.index(), 0);
    assert_eq!(prepared.after_external_id(), Some("b"));
    let pending_snapshot:Id=sqlx::query_scalar("SELECT id FROM runtime_native_input_snapshot WHERE body#>>'{source,kind}'='cache' AND body#>>'{source,ordinal}'='0' ORDER BY id DESC LIMIT 1").fetch_one(&mut *tx).await.unwrap();
    assert!(
        Snapshots::storage_evidence(&mut tx, pending_snapshot)
            .await
            .is_err(),
        "keyless final storage inventory requires actual target binding too"
    );
    assert!(!prepared.exhausted());
    assert_eq!(prepared.value().unwrap()[0]["external_id"], json!("a"));
    let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
        .await
        .unwrap();
    let declaration = declarations
        .iter()
        .find(|declaration| declaration.expression == "{{ item[0].value }}")
        .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &declaration.declaration_revision,
        &declaration.source_path,
        &declaration.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_cache_input(&mut session, &prepared).unwrap();
    let rendered = Inputs::evaluate(&mut tx, session).await.unwrap();
    assert_eq!(
        rendered.value(),
        &json!({"token":"{{ keystore['system.never_resolve'] }}"}),
        "the actual selected cache value must not be recursively interpolated"
    );
    let destination = InputSite {
        execution: root.id,
        kind: InputEntityKind::ExecutionResult,
        purpose: InputSitePurpose::Value,
        path: "/cache".into(),
    };
    let receipt_destination =
        Inputs::prepare_destination(&mut tx, &rendered, &destination, NATIVE_CACHE_TEST_KEY)
            .await
            .unwrap();
    assert!(Inputs::record_prepared(&mut tx,&rendered,&receipt_destination,rendered.value(),rendered.selections(),NATIVE_CACHE_TEST_KEY).await.is_err(),"a prospective page is renderable before targets, not publishable before actual iteration binding");
    let iteration = WorkflowCacheIterationRepository::create(
        &mut *tx,
        prepared.initial_create_input().unwrap(),
    )
    .await
    .unwrap();
    assert!(
        Snapshots::bind_cache_frontier(&mut tx, &next, iteration.id)
            .await
            .is_err(),
        "a successor cannot skip its predecessor binding/progress"
    );
    sqlx::query("SAVEPOINT initial_cache_membership")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE cache_namespace SET active_generation=NULL WHERE id=$1")
        .bind(namespace.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        Snapshots::bind_cache_frontier(&mut tx, &prepared, iteration.id)
            .await
            .is_err(),
        "initial active-generation membership must still match at target binding"
    );
    sqlx::query("ROLLBACK TO SAVEPOINT initial_cache_membership")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("RELEASE SAVEPOINT initial_cache_membership")
        .execute(&mut *tx)
        .await
        .unwrap();
    Snapshots::bind_cache_frontier(&mut tx, &prepared, iteration.id)
        .await
        .unwrap();
    assert!(
        Snapshots::bind_cache_frontier(&mut tx, &prepared, iteration.id)
            .await
            .is_err(),
        "a bound frontier cannot be replayed"
    );
    let receipt = Inputs::record_prepared(
        &mut tx,
        &rendered,
        &receipt_destination,
        rendered.value(),
        rendered.selections(),
        NATIVE_CACHE_TEST_KEY,
    )
    .await
    .unwrap();
    assert_eq!(
        receipt.native_source_metadata()[0].generation(),
        Some(generation.id)
    );
    assert_eq!(receipt.public_source_sites().len(), 1);
    assert_eq!(
        receipt.public_source_sites()[0].path,
        "/cache_name",
        "only the authenticated selector parameter is a public leaf, not cache data"
    );
    let frozen = Snapshots::source_evidence(
        &mut tx,
        &receipt.native_source_metadata()[0],
        NATIVE_CACHE_TEST_KEY,
    )
    .await
    .unwrap();
    assert_eq!(frozen.receipts().len(), 1);
    assert!(frozen.fields().is_empty());
    let policy = frozen.cache_read_policy().unwrap();
    assert_eq!(policy.require_fresh(), require_fresh);
    assert_eq!(
        policy.fresh_until().is_some(),
        require_fresh && freshness_seconds > 0
    );
    policy.ensure_fresh().unwrap();
    sqlx::query("SAVEPOINT original_cache_reader_policy")
        .execute(&mut *tx)
        .await
        .unwrap();
    CacheGenerationRepository::promote_conn(
        &mut tx,
        namespace.id,
        replacement.id,
        Some(generation.id),
        Utc::now() + Duration::hours(1),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE cache_namespace SET freshness_target_seconds=0 WHERE id=$1")
        .bind(namespace.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let retired = Snapshots::source_evidence(
        &mut tx,
        &receipt.native_source_metadata()[0],
        NATIVE_CACHE_TEST_KEY,
    )
    .await;
    assert_eq!(retired.is_err(),require_fresh,"only the original contract decides whether a retained retired page needs a live generation; changing current namespace policy cannot waive it");
    assert_eq!(
        receipt.native_source_metadata()[0].generation(),
        Some(generation.id),
        "rotation to a fresh replacement must never rebase the original metadata"
    );
    sqlx::query("ROLLBACK TO SAVEPOINT original_cache_reader_policy")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("RELEASE SAVEPOINT original_cache_reader_policy")
        .execute(&mut *tx)
        .await
        .unwrap();
    let original_owner =
        serde_json::to_value(receipt.native_source_metadata()[0].read_owner()).unwrap();
    assert_eq!(original_owner["reference"], json!("native-input"));
    assert_eq!(original_owner["pack"], json!(owner.pack_id));
    let snapshot = receipt.native_source_metadata()[0].snapshot();
    let storage = Snapshots::storage_evidence(&mut tx, snapshot)
        .await
        .unwrap();
    assert_eq!(storage.metadata(), &receipt.native_source_metadata()[0]);
    let physical = storage.cache().unwrap();
    assert_eq!(
        (
            physical.namespace(),
            physical.generation(),
            physical.iteration(),
            physical.workflow()
        ),
        (namespace.id, generation.id, iteration.id, workflow.id)
    );
    assert_eq!(physical.ordinal(), 0);
    assert_eq!(
        physical
            .entries()
            .iter()
            .map(|entry| entry.external_id())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    let original_entry_ids:Vec<Id>=sqlx::query_scalar("SELECT id FROM cache_entry WHERE generation=$1 AND external_id IN ('a','b') ORDER BY external_id").bind(generation.id).fetch_all(&mut *tx).await.unwrap();
    assert_eq!(
        physical
            .entries()
            .iter()
            .map(|entry| entry.id())
            .collect::<Vec<_>>(),
        original_entry_ids
    );
    assert_eq!(storage.frozen_owner().pack(), Some(owner.pack_id));
    assert_eq!(storage.frozen_owner().reference(), Some("native-input"));
    for parent in [
        attune_common::repositories::native_input_snapshot::NativeStorageParent::CacheNamespace {
            namespace: namespace.id,
        },
        attune_common::repositories::native_input_snapshot::NativeStorageParent::CacheGeneration {
            generation: generation.id,
        },
        attune_common::repositories::native_input_snapshot::NativeStorageParent::CacheIteration {
            iteration: iteration.id,
        },
        attune_common::repositories::native_input_snapshot::NativeStorageParent::Pack {
            pack: owner.pack_id,
        },
    ] {
        assert!(storage.physical_parents().contains(&parent));
    }
    for entry in &original_entry_ids {
        assert!(storage.physical_parents().contains(
            &attune_common::repositories::native_input_snapshot::NativeStorageParent::CacheEntry {
                generation: generation.id,
                entry: *entry
            }
        ));
    }
    assert_eq!(storage.archive_owner(), ("native_input_snapshot", snapshot));
    assert!(storage.archive_fields().is_empty());
    assert!(storage.contributor_manifests().is_empty());
    assert_eq!(storage.receipt_edges().len(), 1);
    assert_eq!(storage.receipt_edges()[0].1, control_site);
    let recorded_renderer: Id = sqlx::query_scalar(
        "SELECT id FROM runtime_authority_admission WHERE owner_kind='execution' AND owner_id=$1",
    )
    .bind(root.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(storage.renderer_admission(), recorded_renderer);
    let stored_body: serde_json::Value =
        sqlx::query_scalar("SELECT body FROM runtime_native_input_snapshot WHERE id=$1")
            .bind(snapshot)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let storage_bytes = serde_json::to_vec(&stored_body).unwrap().len()
        + serde_json::to_vec(storage.encrypted_projection())
            .unwrap()
            .len()
        + 64
        + storage.encryption_key_hash().len()
        + serde_json::to_vec(&stored_body["source"]["parents"])
            .unwrap()
            .len()
        + serde_json::to_vec(&json!({"budget":recorded_renderer,"admissions":[],"release":null,"cache_frontier":[Id::MAX,null,stored_body["source"]["frontier"]["prepared_transaction"],0,Id::MAX,stored_body["source"]["frontier"]["prepared_transaction"]]}))
            .unwrap()
            .len();
    assert_eq!(storage.retained_bytes(),storage_bytes,"charge both immutable body/ciphertext and their normalized storage associations, without reconstructing data from plaintext");
    assert!(
        storage.retained_bytes()
            >= serde_json::to_vec(storage.encrypted_projection())
                .unwrap()
                .len()
    );
    sqlx::query("SAVEPOINT keyless_storage_shadow")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO runtime_native_input_snapshot_admission(snapshot,admission) VALUES($1,$2)",
    )
    .bind(snapshot)
    .bind(recorded_renderer)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert!(
        Snapshots::storage_evidence(&mut tx, snapshot)
            .await
            .is_err(),
        "a cache projection cannot silently accept an added normalized source-admission edge"
    );
    sqlx::query("ROLLBACK TO SAVEPOINT keyless_storage_shadow")
        .execute(&mut *tx)
        .await
        .unwrap();
    let corrupt_snapshot:Id=sqlx::query_scalar("INSERT INTO runtime_native_input_snapshot(renderer_admission,policy_revision,body,body_digest,encrypted_value,encryption_key_hash) SELECT renderer_admission,policy_revision,body,'invalid-storage-digest',encrypted_value,encryption_key_hash FROM runtime_native_input_snapshot WHERE id=$1 RETURNING id").bind(snapshot).fetch_one(&mut *tx).await.unwrap();
    assert!(
        Snapshots::storage_evidence(&mut tx, corrupt_snapshot)
            .await
            .is_err(),
        "keyless storage inventory still authenticates the immutable body digest"
    );
    sqlx::query("ROLLBACK TO SAVEPOINT keyless_storage_shadow")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("RELEASE SAVEPOINT keyless_storage_shadow")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(Snapshots::source_evidence(
        &mut tx,
        &receipt.native_source_metadata()[0],
        "wrong-native-source-key"
    )
    .await
    .is_err());
    let reference = receipt.origin();
    let receipt_id = match &reference {
        attune_common::secret_provenance::SecretOrigin::Entity { entity_id, .. } => *entity_id,
        _ => unreachable!(),
    };
    assert_eq!(
        Inputs::retention_parents(&mut tx, receipt_id)
            .await
            .unwrap()
            .native_snapshots()
            .len(),
        1
    );
    sqlx::query("SAVEPOINT stale_cache_target")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE workflow_cache_iteration SET last_external_id='replacement',next_batch_index=99,scanned_count=99,dispatched_count=99 WHERE id=$1").bind(iteration.id).execute(&mut *tx).await.unwrap();
    assert!(
        Snapshots::bind_cache_frontier(&mut tx, &next, iteration.id)
            .await
            .is_err(),
        "an actual target cursor change cannot consume the prepared successor"
    );
    sqlx::query("ROLLBACK TO SAVEPOINT stale_cache_target")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("RELEASE SAVEPOINT stale_cache_target")
        .execute(&mut *tx)
        .await
        .unwrap();
    WorkflowCacheIterationRepository::update_scan_progress(
        &mut *tx,
        iteration.id,
        UpdateWorkflowCacheIterationProgressInput {
            last_external_id: "b".into(),
            next_batch_index: 1,
            scanned_count: 2,
            dispatched_count: 1,
        },
    )
    .await
    .unwrap()
    .unwrap();
    Snapshots::bind_cache_frontier(&mut tx, &next, iteration.id)
        .await
        .unwrap();
    assert!(
        Snapshots::bind_cache_frontier(&mut tx, &next2, iteration.id)
            .await
            .is_err(),
        "each next frontier needs actual predecessor progress, not just its binding flag"
    );
    WorkflowCacheIterationRepository::update_scan_progress(
        &mut *tx,
        iteration.id,
        UpdateWorkflowCacheIterationProgressInput {
            last_external_id: "d".into(),
            next_batch_index: 2,
            scanned_count: 4,
            dispatched_count: 2,
        },
    )
    .await
    .unwrap()
    .unwrap();
    Snapshots::bind_cache_frontier(&mut tx, &next2, iteration.id)
        .await
        .unwrap();
    WorkflowCacheIterationRepository::update_scan_progress(
        &mut *tx,
        iteration.id,
        UpdateWorkflowCacheIterationProgressInput {
            last_external_id: "e".into(),
            next_batch_index: 3,
            scanned_count: 5,
            dispatched_count: 3,
        },
    )
    .await
    .unwrap()
    .unwrap();
    Snapshots::bind_cache_frontier(&mut tx, &eof, iteration.id)
        .await
        .unwrap();
    let eof_snapshot:Id=sqlx::query_scalar("SELECT id FROM runtime_native_input_snapshot WHERE body#>>'{source,kind}'='cache' AND body#>>'{source,ordinal}'='3' ORDER BY id DESC LIMIT 1").fetch_one(&mut *tx).await.unwrap();
    let eof_storage = Snapshots::storage_evidence(&mut tx, eof_snapshot)
        .await
        .unwrap();
    assert_eq!(eof_storage.cache().unwrap().iteration(), iteration.id);
    assert!(eof_storage.physical_parents().iter().any(|parent|matches!(parent,attune_common::repositories::native_input_snapshot::NativeStorageParent::NativeSnapshot { .. })),"physical closure retains the opaque predecessor snapshot, never a receipt with the same numeric ID");
    assert!(eof_storage.retained_bytes()>storage.retained_bytes(),"successor reservation includes every retained predecessor without resetting its byte budget");
    assert!(
        prepared.require_current(&mut tx).await.is_err(),
        "the real target cursor cannot reuse a stale source page"
    );
    let retained_storage = Snapshots::storage_evidence(&mut tx, snapshot)
        .await
        .unwrap();
    assert_eq!(
        retained_storage.cache().unwrap().ordinal(),
        0,
        "keyless inventory uses the frozen page, not the now-advanced iteration row"
    );
    assert_eq!(
        retained_storage
            .cache()
            .unwrap()
            .entries()
            .iter()
            .map(|entry| entry.id())
            .collect::<Vec<_>>(),
        original_entry_ids
    );
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let held_read = Snapshots::source_evidence(
        &mut tx,
        &receipt.native_source_metadata()[0],
        NATIVE_CACHE_TEST_KEY,
    )
    .await
    .unwrap();
    let mut writer = db.begin().await.unwrap();
    let lock = sqlx::query("SELECT id FROM cache_generation WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(generation.id)
        .execute(&mut *writer)
        .await;
    assert_eq!(lock.is_err(),require_fresh,"the reader holds the exact original generation FOR SHARE only when its original task requires freshness");
    writer.rollback().await.unwrap();
    held_read
        .cache_read_policy()
        .unwrap()
        .ensure_fresh()
        .unwrap();
    assert!(
        Snapshots::bind_cache_frontier(&mut tx, &next, iteration.id)
            .await
            .is_err(),
        "a prospective binding cannot cross transactions"
    );
    assert!(
        Snapshots::prepare_cache_next(&mut tx, &next2, 65536, NATIVE_CACHE_TEST_KEY)
            .await
            .is_err(),
        "in-memory plans cannot become post-commit continuation tokens"
    );
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &declaration.declaration_revision,
        &declaration.source_path,
        &declaration.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    assert!(Inputs::bind_cache_input(&mut session, &prepared).is_err());
    assert!(
        Inputs::receipt_context(&mut tx, &destination, &reference, "wrong-key")
            .await
            .is_err()
    );
    assert!(
        Inputs::receipt_context(&mut tx, &destination, &reference, NATIVE_CACHE_TEST_KEY)
            .await
            .is_ok()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_wait_snapshots_release_real_targets_and_keep_original_response_after_source_gc() {
    native_wait_snapshot_fixture(false, false, None).await;
}

#[tokio::test]
async fn native_wait_materializer_selected_approval_expiry_after_actual_target_wait_denies_delivery(
) {
    native_wait_snapshot_fixture(true, false, None).await;
}

#[tokio::test]
async fn native_wait_terminal_recovery_expiry_after_actual_target_wait_rolls_back_decisions_and_children(
) {
    native_wait_snapshot_fixture(true, true, None).await;
}

#[tokio::test]
async fn native_wait_registered_callers_release_and_replay_all_kinds_after_source_gc_and_issuer_freeze(
) {
    for index in 0..3 {
        native_wait_snapshot_fixture(
            false,
            false,
            Some(native_wait_materialization::Case::Delivery(index)),
        )
        .await;
    }
}

#[tokio::test]
async fn native_wait_terminal_callers_recover_all_failed_timeout_cancelled_abandoned_outcomes() {
    for (index, status, publication) in [
        (0, "failed", "native_failed"),
        (0, "abandoned", "native_failed"),
        (0, "timeout", "native_timeout"),
        (0, "cancelled", "native_cancelled"),
        (1, "timeout", "native_timeout"),
        (1, "cancelled", "native_cancelled"),
        (2, "failed", "native_failed"),
        (2, "skipped", "native_failed"),
        (2, "cancelled", "native_cancelled"),
    ] {
        native_wait_snapshot_fixture(
            false,
            false,
            Some(native_wait_materialization::Case::Terminal(
                index,
                status,
                publication,
            )),
        )
        .await;
    }
}

#[tokio::test]
async fn native_wait_terminal_independent_publications_overwrite_subtrees_in_order_and_replay() {
    for index in [0, 2] {
        native_wait_snapshot_fixture(
            false,
            false,
            Some(native_wait_materialization::Case::Overwrite(index)),
        )
        .await;
    }
}

#[tokio::test]
async fn native_initial_wait_registers_and_delivers_already_terminal_real_sources_atomically() {
    for index in 0..3 {
        native_wait_snapshot_fixture(
            false,
            false,
            Some(native_wait_materialization::Case::Initial(
                index,
                native_wait_materialization::InitialCase::Terminal,
            )),
        )
        .await;
    }
}

#[tokio::test]
async fn native_initial_wait_waiting_inquiry_retains_target_until_release() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::Initial(
            1,
            native_wait_materialization::InitialCase::InProgress,
        )),
    )
    .await;
}

#[tokio::test]
async fn native_initial_wait_inprogress_wrong_transaction_missing_release_and_concurrent_registration(
) {
    use native_wait_materialization::InitialCase;
    for mode in [
        InitialCase::InProgress,
        InitialCase::WrongTransaction,
        InitialCase::MissingRelease,
        InitialCase::ConcurrentIdentical,
        InitialCase::Conflicting,
        InitialCase::CancelledParent,
        InitialCase::MissingReservation,
    ] {
        native_wait_snapshot_fixture(
            false,
            false,
            Some(native_wait_materialization::Case::Initial(0, mode)),
        )
        .await;
    }
}

#[tokio::test]
async fn native_initial_wait_expiry_is_observed_after_the_actual_workflow_target_wait() {
    native_wait_snapshot_fixture(
        true,
        false,
        Some(native_wait_materialization::Case::Initial(
            0,
            native_wait_materialization::InitialCase::Expiry,
        )),
    )
    .await;
}

#[tokio::test]
async fn native_wait_value_condition_dependent_publication_real_recovery_and_gc_replay() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::Dependent(0)),
    )
    .await;
}

#[tokio::test]
async fn native_wait_dependent_value_frame_activates_actual_cache_successor() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::Cache(0, false)),
    )
    .await;
}

#[tokio::test]
async fn native_wait_dependent_value_frame_preserves_empty_cache_no_child_checkpoint() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::Cache(0, true)),
    )
    .await;
}

#[tokio::test]
async fn native_wait_dependent_publication_expiry_after_observed_actual_target_wait_denies_all_stages(
) {
    native_wait_snapshot_fixture(
        true,
        false,
        Some(native_wait_materialization::Case::DependentExpiry(0)),
    )
    .await;
}

#[tokio::test]
async fn native_wait_empty_cache_frame_expires_at_observed_workflow_target_wait() {
    native_wait_snapshot_fixture(
        true,
        false,
        Some(native_wait_materialization::Case::CacheExpiry(0)),
    )
    .await;
}

#[tokio::test]
async fn native_wait_staged_publication_reaches_actual_recovery_child_and_survives_gc_replay() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::DependentChild(0)),
    )
    .await;
}

#[tokio::test]
async fn native_prospective_initial_terminal_recovery_uses_real_reserved_wait_and_staged_children()
{
    for state in ["failed", "timeout", "cancelled"] {
        native_wait_snapshot_fixture(
            false,
            false,
            Some(native_wait_materialization::Case::Initial(
                0,
                native_wait_materialization::InitialCase::Recovery(state),
            )),
        )
        .await;
    }
}

#[tokio::test]
async fn native_prospective_initial_terminal_recovery_expiry_at_actual_workflow_target_wait() {
    native_wait_snapshot_fixture(
        true,
        false,
        Some(native_wait_materialization::Case::Initial(
            0,
            native_wait_materialization::InitialCase::RecoveryExpiry,
        )),
    )
    .await;
}

#[tokio::test]
async fn native_prospective_initial_ordered_array_publications_keep_overwritten_value_history() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::Initial(
            0,
            native_wait_materialization::InitialCase::RecoveryArray,
        )),
    )
    .await;
}

#[tokio::test]
async fn empty_wait_recovery_control_reopens_after_plan_drop_and_expires_at_original_consent() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::EmptyControlExpiry(0)),
    )
    .await;
}

async fn native_wait_snapshot_fixture(
    expire_while_blocked: bool,
    terminal_expiry: bool,
    caller_case: Option<native_wait_materialization::Case>,
) {
    use attune_common::models::{WorkQueueItemStatus, WorkflowTaskWaitTarget};
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        declared_input::DeclaredInputRepository,
        execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        inquiry::{CreateWorkflowInquiryInput, InquiryRepository},
        inquiry_schema::InquirySchemaRepository,
        native_input_snapshot::NativeInputSnapshotRepository as Snapshots,
        runtime_target::{ComponentTarget, RuntimeTarget},
        schema_contract::{BindingOwner, SchemaContractRepository},
        work_queue::{CreateWorkQueueItemInput, WorkQueueItemRepository},
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            WorkflowDefinitionRepository, WorkflowExecutionRepository,
        },
        workflow_task_wait::{CreateWorkflowTaskWaitInput, WorkflowTaskWaitRepository},
    };
    let (db, fixture) = fixture().await;
    let owner = &fixture.policy.policy.owner;
    let key = NATIVE_WAIT_TEST_KEY;
    let original_key = helpers::KeyFixture::new_pack(
        owner.pack_id,
        &owner.pack_ref,
        "wait_original",
        "execution-original",
    )
    .create(&db)
    .await
    .unwrap();
    let input_set=PermissionSetRepository::create(&db,CreatePermissionSetInput {r#ref:"fixture.native_wait_original_input".into(),pack:None,pack_ref:None,label:None,description:None,grants:json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[original_key.r#ref]}}])}).await.unwrap();
    PermissionAssignmentRepository::create(
        &db,
        CreatePermissionAssignmentInput {
            identity: fixture.human_id,
            permset: input_set.id,
        },
    )
    .await
    .unwrap();
    let graph_action = ActionFixture::new(owner.pack_id, &owner.pack_ref, "wait_graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(owner.pack_id, &owner.pack_ref, "wait_leaf")
        .create(&db)
        .await
        .unwrap();
    ActionRepository::update(&db,graph_action.id,UpdateActionInput {param_schema:Some(json!({"target_execution":{"type":"integer"},"target_inquiry":{"type":"integer"},"target_queue":{"type":"integer"}})),..Default::default()}).await.unwrap();
    ActionRepository::update(
        &db,
        leaf.id,
        UpdateActionInput {
            param_schema: Some(
                if matches!(
                    &caller_case,
                    Some(native_wait_materialization::Case::EmptyControlExpiry(_))
                ) {
                    json!({"token":{"type":"string"},"probe":{"type":"string"}})
                } else {
                    json!({"token":{"type":"string"}})
                },
            ),
            out_schema: Some(
                if matches!(
                    &caller_case,
                    Some(native_wait_materialization::Case::EmptyControlExpiry(_))
                ) {
                    json!({"token":{"type":"string","secret":true},"probe":{"type":"string"}})
                } else {
                    json!({"token":{"type":"string","secret":true}})
                },
            ),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let set_ref = format!("{}.wait_reads", owner.pack_ref);
    let selector_key = if matches!(
        &caller_case,
        Some(native_wait_materialization::Case::EmptyControlExpiry(_))
    ) {
        // This key has not been read. Author its stable name and the human's
        // exact read grant before the graph, policy, and supplying authority.
        let key = helpers::KeyFixture::new_pack(
            owner.pack_id,
            &owner.pack_ref,
            "wait_empty_selector",
            "unread",
        )
        .create(&db)
        .await
        .unwrap();
        let role = PermissionSetRepository::create(&db, CreatePermissionSetInput {
            r#ref: "fixture.native_wait_empty_selector".into(), pack: None, pack_ref: None,
            label: None, description: None,
            grants: json!([{"resource":"keys","actions":["read"],"constraints":{"refs":[key.r#ref]}}]),
        }).await.unwrap();
        PermissionAssignmentRepository::create(
            &db,
            CreatePermissionAssignmentInput {
                identity: fixture.human_id,
                permset: role.id,
            },
        )
        .await
        .unwrap();
        Some(key)
    } else {
        None
    };
    let outcome_transitions = json!([
        {"when":"{{ succeeded() }}"}, {"when":"{{ failed() }}"},
        {"when":"{{ timed_out() }}"}, {"when":"{{ cancelled() }}"}
    ]);
    let mut definition = json!({"tasks":[
        {"name":"observed_execution","action":leaf.r#ref,"wait_for":{"execution":"{{ target_execution }}"},"permission_set_refs":[set_ref],"params":{"token":"{{ execution.observed_execution.result.token }}"},"next":outcome_transitions},
        {"name":"observed_inquiry","action":leaf.r#ref,"wait_for":{"inquiry":"{{ target_inquiry }}"},"permission_set_refs":[set_ref],"params":{"token":"{{ inquiry.observed_inquiry.response.token }}"},"next":outcome_transitions},
        {"name":"observed_queue","action":leaf.r#ref,"wait_for":{"work_queue_item":"{{ target_queue }}"},"permission_set_refs":[set_ref],"params":{"token":"{{ work_queue_item.observed_queue.result.token }}"},"next":outcome_transitions},
        {"name":"producer","action":leaf.r#ref,"permission_set_refs":[]},
        {"name":"source","action":leaf.r#ref,"permission_set_refs":[]},
        {"name":"native_recovery","action":leaf.r#ref,"permission_set_refs":[],"params":{"token":"recovered"}}
    ]});
    if matches!(
        &caller_case,
        Some(native_wait_materialization::Case::EmptyControlExpiry(_))
    ) {
        // Author empty inputs and real false/true outcome conditions before
        // graph, executable, declaration, policy, or input acceptance capture.
        definition["tasks"][0]["next"] = json!([
            {"when":"{{ succeeded() }}","do":["native_recovery"]},
            {"when":"{{ failed() }}","do":["native_recovery"]}
        ]);
        definition["tasks"][5]["params"] = json!({});
        let selector = selector_key
            .as_ref()
            .expect("new case owns its authored selector key");
        definition["tasks"][0]["wait_for"]["execution"] =
            json!(format!("{{{{ keystore['{}'] }}}}", selector.r#ref));
        definition["tasks"].as_array_mut().unwrap().push(json!({
            "name":"native_after_recovery", "action":leaf.r#ref,
            "permission_set_refs":[], "params":{"probe":"{{ task.native_recovery.probe }}"}
        }));
    }
    if terminal_expiry
        || matches!(
            &caller_case,
            Some(native_wait_materialization::Case::Terminal(..))
                | Some(native_wait_materialization::Case::Overwrite(_))
                | Some(native_wait_materialization::Case::Dependent(_))
                | Some(native_wait_materialization::Case::DependentChild(_))
                | Some(native_wait_materialization::Case::DependentExpiry(_))
                | Some(native_wait_materialization::Case::Cache(..))
                | Some(native_wait_materialization::Case::CacheExpiry(_))
        )
    {
        for task in definition["tasks"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .take(3)
        {
            task["next"] = if matches!(
                &caller_case,
                Some(
                    native_wait_materialization::Case::Dependent(_)
                        | native_wait_materialization::Case::DependentChild(_)
                        | native_wait_materialization::Case::DependentExpiry(_)
                        | native_wait_materialization::Case::Cache(..)
                        | native_wait_materialization::Case::CacheExpiry(_)
                )
            ) {
                native_wait_materialization::dependent_program()
            } else if matches!(
                &caller_case,
                Some(native_wait_materialization::Case::Overwrite(_))
            ) {
                native_wait_materialization::overwrite_program()
            } else {
                native_wait_materialization::recovery_program()
            };
        }
    }
    if matches!(
        &caller_case,
        Some(native_wait_materialization::Case::DependentChild(_))
    ) {
        definition["tasks"][5]["params"] = json!({"token":"{{ workflow.b }}"});
    }
    if matches!(
        &caller_case,
        Some(native_wait_materialization::Case::Initial(
            _,
            native_wait_materialization::InitialCase::Recovery(_)
                | native_wait_materialization::InitialCase::RecoveryExpiry
                | native_wait_materialization::InitialCase::RecoveryArray
        ))
    ) {
        let dependent = matches!(
            &caller_case,
            Some(native_wait_materialization::Case::Initial(
                _,
                native_wait_materialization::InitialCase::Recovery("failed")
                    | native_wait_materialization::InitialCase::RecoveryExpiry
                    | native_wait_materialization::InitialCase::RecoveryArray
            ))
        );
        for task in definition["tasks"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .take(3)
        {
            task["next"] = if matches!(
                &caller_case,
                Some(native_wait_materialization::Case::Initial(
                    _,
                    native_wait_materialization::InitialCase::RecoveryArray
                ))
            ) {
                native_wait_materialization::ordered_array_program()
            } else if dependent {
                native_wait_materialization::dependent_program()
            } else {
                native_wait_materialization::recovery_program()
            };
        }
        if dependent {
            definition["tasks"][5]["params"] = json!({"token":"{{ workflow.b }}"});
        }
    }
    if matches!(
        &caller_case,
        Some(
            native_wait_materialization::Case::Cache(..)
                | native_wait_materialization::Case::CacheExpiry(_)
        )
    ) {
        let empty = !matches!(
            &caller_case,
            Some(native_wait_materialization::Case::Cache(_, false))
        );
        definition["tasks"][5]["iterate_cache"] =
            json!({"namespace":"{{ workflow.b }}","page_size":1});
        definition["tasks"][5]["batch_size"] = json!(1);
        definition["tasks"][5]["permission_set_refs"] = json!(["standard"]);
        definition["tasks"][5]["params"] = json!({"token":"{{ item.value.token }}"});
        native_wait_materialization::seed_cache(&db, owner.pack_id, &owner.pack_ref, empty).await;
    }
    if matches!(
        &caller_case,
        Some(native_wait_materialization::Case::Initial(
            _,
            native_wait_materialization::InitialCase::Successor(_)
        ))
    ) {
        definition["tasks"][4]["next"] = if matches!(
            &caller_case,
            Some(native_wait_materialization::Case::Initial(
                _,
                native_wait_materialization::InitialCase::Successor(3)
            ))
        ) {
            json!([{"do":["observed_execution"]}])
        } else {
            json!([{"when":"{{ timed_out() }}", "do":["observed_execution"]}])
        };
        definition["tasks"][0]["next"] = json!([]);
    }
    let graph = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: graph_action.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "Wait observations".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            tags: vec![],
            definition: definition.clone(),
        },
    )
    .await
    .unwrap();
    ActionRepository::link_workflow_def(&db, graph_action.id, graph.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = activate_release(
        &mut tx,
        owner.pack_id,
        &owner.pack_ref,
        vec![graph_action.id, leaf.id],
        'd',
    )
    .await;
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let root = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(graph_action.id),
            action_ref: graph_action.r#ref.clone(),
            executor: Some(fixture.human_id),
            status: ExecutionStatus::Requested,
            config: None,
            permission_set_refs: vec![],
            ..Default::default()
        },
        &snapshots[&graph_action.r#ref],
    )
    .await
    .unwrap();
    let workflow = WorkflowExecutionRepository::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: root.id,
            workflow_def: graph.id,
            task_graph: definition,
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    let source = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(leaf.id),
            action_ref: leaf.r#ref.clone(),
            parent: Some(root.id),
            status: ExecutionStatus::Completed,
            config: Some(json!({})),
            result: Some(json!({"token":attune_common::secret_values::redaction_marker()})),
            workflow_task:Some(serde_json::from_value(json!({"workflow_execution":workflow.id,"task_name":"source","retry_count":0,"max_retries":0,"timed_out":false})).unwrap()),
            permission_set_refs: vec![],
            ..Default::default()
        },
        &snapshots[&leaf.r#ref],
    )
    .await
    .unwrap();
    if let Some(selector) = &selector_key {
        // The real source ID is now known. Set the unread key before its first
        // declared source read; no accepted input or captured proof is changed.
        attune_common::repositories::key::KeyRepository::update(
            &mut *tx,
            selector.id,
            attune_common::repositories::key::UpdateKeyInput {
                value: Some(json!(source.id)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let metadata=serde_json::from_value(json!({"workflow_execution":workflow.id,"task_name":"producer","retry_count":0,"max_retries":0,"timed_out":false})).unwrap();
    // Completion cases model the inquiry-producing action having returned.
    // Leaving this unrelated physical child Running must block root completion,
    // even when the selected Wait is terminal. Other lifecycle cases keep it live.
    let producer_finished = matches!(
        &caller_case,
        Some(native_wait_materialization::Case::Initial(
            _,
            native_wait_materialization::InitialCase::TerminalOutput
                | native_wait_materialization::InitialCase::Successor(_)
        ))
    );
    let producer = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(leaf.id),
            action_ref: leaf.r#ref.clone(),
            parent: Some(root.id),
            status: ExecutionStatus::Running,
            config: Some(json!({})),
            workflow_task: Some(metadata),
            permission_set_refs: vec![],
            ..Default::default()
        },
        &snapshots[&leaf.r#ref],
    )
    .await
    .unwrap();
    let inquiry = InquirySchemaRepository::create_workflow(
        &mut tx,
        CreateWorkflowInquiryInput {
            created_by_execution: producer.id,
            purpose: "native-observation".into(),
            prompt: "Owned response".into(),
            response_schema: Some(json!({"token":{"type":"string","secret":true}})),
            response_options: native_wait_response_options(),
            assigned_to: None,
            timeout_seconds: None,
        },
        key,
    )
    .await
    .unwrap();
    assert_eq!(inquiry.created_by_execution, producer.id);
    assert_eq!(inquiry.workflow_execution, Some(workflow.id));
    assert_eq!(inquiry.workflow_task_name.as_deref(), Some("producer"));
    assert_eq!(inquiry.action_attempt_family, Some(producer.id));
    let inquiry_contract = InquirySchemaRepository::find(&mut *tx, inquiry.id)
        .await
        .unwrap()
        .unwrap();
    let (response, fields) = inquiry_contract
        .binding
        .prepare_response(
            &json!({"token":"inquiry-original"}),
            &attune_common::secret_provenance::SecretProvenance::default(),
        )
        .unwrap();
    InquiryRepository::respond_pending(&mut *tx, inquiry.id, response, fixture.human_id, None)
        .await
        .unwrap()
        .unwrap();
    Secrets::upsert_many_with_conn(
        &mut tx,
        attune_common::secret_values::ENTITY_INQUIRY_RESPONSE,
        inquiry.id,
        &attune_common::secret_values::prepare_secret_values(fields, key).unwrap(),
    )
    .await
    .unwrap();
    if producer_finished {
        ExecutionRepository::update(
            &mut *tx,
            producer.id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let queue = owned_queue(&mut tx, owner, leaf.id, &leaf.r#ref).await;
    let item = WorkQueueItemRepository::create(
        &mut *tx,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: None,
            priority: 0,
            status: WorkQueueItemStatus::Completed,
            payload: json!({}),
            metadata: json!({}),
            trace_tag: None,
            enqueue_source: "native-wait-owned-fixture".into(),
            requested_by_identity: None,
            requested_by_execution: Some(root.id),
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 1,
            last_error: None,
            ack_summary: Some(json!({"token":"queue-original","status":"cancelled","succeeded":false,"failed":true,"timed_out":true})),
        },
    )
    .await
    .unwrap();
    // Runtime wait targets become known after the root and source tasks exist.
    // Publish actual workflow variables; root config stays immutable from creation.
    if selector_key.is_none() {
        sqlx::query("UPDATE workflow_execution SET variables=$2 WHERE id=$1")
            .bind(workflow.id)
            .bind(json!({"target_execution":source.id,"target_inquiry":inquiry.id,"target_queue":item.id}))
            .execute(&mut *tx).await.unwrap();
    }
    let grants = json!([
        {"resource":"executions","actions":["read","decrypt"],"constraints":{"ids":[source.id]}},
        {"resource":"inquiries","actions":["read","decrypt"],"constraints":{"ids":[inquiry.id]}},
        {"resource":"queue_items","actions":["read","decrypt"],"constraints":{"ids":[item.id]}}
    ]);
    let issuer_set = PermissionSetRepository::create(
        &mut *tx,
        CreatePermissionSetInput {
            r#ref: "fixture.native_wait_issuer".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: grants.clone(),
        },
    )
    .await
    .unwrap();
    PermissionAssignmentRepository::create(
        &mut *tx,
        CreatePermissionAssignmentInput {
            identity: fixture.human_id,
            permset: issuer_set.id,
        },
    )
    .await
    .unwrap();
    let original_consent_started = tokio::time::Instant::now();
    let approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        owner.pack_id,
        fixture.human_id,
        serde_json::from_value(grants).unwrap(),
        Some(
            Utc::now()
                + if matches!(
                    &caller_case,
                    Some(native_wait_materialization::Case::EmptyControlExpiry(_))
                ) {
                    Duration::seconds(120)
                } else if expire_while_blocked {
                    // Setup must not consume the approval before the fixture
                    // reaches its observed target-lock barrier. The original
                    // expiry remains fixed and must pass while that lock is held.
                    Duration::seconds(60)
                } else {
                    Duration::hours(1)
                },
        ),
    )
    .await
    .unwrap();
    let read_set = attune_common::runtime_authority::AdmittedPermissionSet::external(
        &set_ref,
        owner,
        &approval.grants,
        &[approval.approval().unwrap()],
    )
    .unwrap();
    let sets = BTreeMap::from([(set_ref.clone(), read_set)]);
    let root_binding = InvocationBinding::Action {
        action_ref: graph_action.r#ref.clone(),
    };
    let leaf_binding = InvocationBinding::Action {
        action_ref: leaf.r#ref.clone(),
    };
    let mut task_bindings = [
        "observed_execution",
        "observed_inquiry",
        "observed_queue",
        "producer",
        "source",
        "native_recovery",
    ]
    .into_iter()
    .map(|name| InvocationBinding::WorkflowTask {
        workflow_ref: graph_action.r#ref.clone(),
        task_path: format!("/tasks/{name}"),
        action_ref: leaf.r#ref.clone(),
    })
    .collect::<Vec<_>>();
    if selector_key.is_some() {
        task_bindings.push(InvocationBinding::WorkflowTask {
            workflow_ref: graph_action.r#ref.clone(),
            task_path: "/tasks/native_after_recovery".into(),
            action_ref: leaf.r#ref.clone(),
        });
    }
    let mut invocations = vec![
        InvocationPolicy::from_executable(
            root_binding.clone(),
            &snapshots[&graph_action.r#ref],
            Some(&[]),
            &sets,
        )
        .unwrap(),
        InvocationPolicy::from_executable(
            leaf_binding.clone(),
            &snapshots[&leaf.r#ref],
            Some(&if matches!(
                &caller_case,
                Some(native_wait_materialization::Case::Initial(
                    _,
                    native_wait_materialization::InitialCase::TerminalOutput
                        | native_wait_materialization::InitialCase::Successor(0 | 3)
                ))
            ) {
                vec!["standard".into()]
            } else {
                vec![]
            }),
            &sets,
        )
        .unwrap(),
    ];
    for (index, binding) in task_bindings.iter().enumerate() {
        let refs = if index == 5
            && matches!(
                &caller_case,
                Some(
                    native_wait_materialization::Case::Cache(..)
                        | native_wait_materialization::Case::CacheExpiry(_)
                )
            ) {
            vec!["standard".into()]
        } else if index < 3 {
            vec![set_ref.clone()]
        } else {
            vec![]
        };
        invocations.push(
            InvocationPolicy::from_executable(
                binding.clone(),
                &snapshots[&leaf.r#ref],
                Some(&refs),
                &sets,
            )
            .unwrap(),
        );
    }
    let policy = RuntimePolicy {
        owner: owner.clone(),
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: BTreeMap::new(),
        invocations,
    };
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let authority = policy
        .policy
        .select(
            policy.id,
            &root_binding,
            &[],
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    let root_admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(root.id),
        &authority,
    )
    .await
    .unwrap();
    assert!(!authority.has_api_access());
    for (execution, binding) in [
        (source.id, &task_bindings[4]),
        (producer.id, &task_bindings[3]),
    ] {
        let authority = policy
            .policy
            .select(
                policy.id,
                binding,
                &[],
                RequestAttribution::ParentExecution {
                    execution_id: root.id,
                },
            )
            .unwrap();
        RuntimeAuthorityRepository::record_admission(
            &mut tx,
            RuntimeOwner::Execution(execution),
            &authority,
        )
        .await
        .unwrap();
    }
    for (execution, snapshot) in [
        (root.id, &snapshots[&graph_action.r#ref]),
        (source.id, &snapshots[&leaf.r#ref]),
    ] {
        let binding = SchemaContractRepository::materialize_action_pair(&mut tx, snapshot)
            .await
            .unwrap();
        SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution, &binding)
            .await
            .unwrap();
    }
    assert!(matches!(&root_admission.authority.attribution,
        RequestAttribution::Human { actor } if actor.identity_id == fixture.human_id));
    if let Some(selector) = &selector_key {
        let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
            .await
            .unwrap();
        let expression = format!("{{{{ keystore['{}'] }}}}", selector.r#ref);
        let bindings = declarations
            .iter()
            .filter(|binding| {
                binding.source_path.ends_with("/wait_for/execution")
                    && binding.expression == expression
            })
            .collect::<Vec<_>>();
        assert_eq!(
            bindings.len(),
            1,
            "approve only the actual immutable selector declaration"
        );
        let binding = bindings[0];
        // Separate, one-time input-only consent for this exact key declaration.
        // It uses the already-issued selected-read deadline, never extends it.
        Inputs::approve_external_input(
            &mut tx,
            root.id,
            &binding.declaration_revision,
            &binding.source_path,
            &binding.expression,
            fixture.human_id,
            serde_json::from_value(json!([{"resource":"keys","actions":["read"],
                "constraints":{"refs":[selector.r#ref]}}]))
            .unwrap(),
            approval.expires_at,
        )
        .await
        .unwrap();
    }
    let original_input = if selector_key.is_some() {
        json!({})
    } else {
        json!({"target_execution":source.id,"target_inquiry":inquiry.id,"target_queue":item.id})
    };
    ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        root.id,
        Some(root_admission.id),
        &snapshots[&graph_action.r#ref],
        Some(original_input.clone()),
        &[],
    )
    .await
    .unwrap();
    attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(
        &mut tx, "execution", root.id, fixture.human_id,
    ).await.unwrap();
    attune_common::repositories::declared_input_materialization::DeclaredInputMaterializationRepository::seal_execution_inputs(
        &mut tx, root.id, key,
    ).await.unwrap();
    let accepted: (serde_json::Value, i64, bool) = sqlx::query_as(
        "SELECT body->'config',jsonb_array_length(body->'contributors')::BIGINT,e.transaction_id=pg_current_xact_id()::TEXT FROM runtime_accepted_input a JOIN runtime_accepted_input_epoch e ON e.admission=a.admission WHERE a.execution=$1",
    ).bind(root.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(accepted, (original_input, 1, true),
        "the wait parent must accept its genuine supplying human's input in its original creation transaction");
    ExecutionRepository::update(
        &mut *tx,
        root.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(&mut tx,"execution",source.id,fixture.human_id).await.unwrap();
    let original_fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/token".into(),
            value: json!("execution-original"),
            source_kind: "provenance".into(),
            source_ref: Some(
                serde_json::to_string(&attune_common::secret_provenance::SecretProvenance::key(
                    attune_common::key_access::key_origin(&original_key),
                ))
                .unwrap(),
            ),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(
        &mut tx,
        attune_common::secret_values::ENTITY_EXECUTION_RESULT,
        source.id,
        &original_fields,
    )
    .await
    .unwrap();
    let targets = [
        WorkflowTaskWaitTarget::Execution(source.id),
        WorkflowTaskWaitTarget::Inquiry(inquiry.id),
        WorkflowTaskWaitTarget::WorkQueueItem(item.id),
    ];
    let names = ["observed_execution", "observed_inquiry", "observed_queue"];
    let mut waits = Vec::new();
    for (name, target) in names.iter().zip(targets) {
        if matches!(
            &caller_case,
            Some(native_wait_materialization::Case::Initial(..))
        ) {
            continue;
        }
        waits.push(
            WorkflowTaskWaitRepository::create_or_get(
                &mut tx,
                CreateWorkflowTaskWaitInput {
                    workflow_execution: workflow.id,
                    task_name: (*name).into(),
                    target,
                },
            )
            .await
            .unwrap()
            .id,
        );
    }
    IdentityRepository::update(
        &mut *tx,
        fixture.human_id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut references = Vec::new();
    let mut outcome_references = Vec::new();
    if let Some(case) = caller_case {
        let index = case.index();
        if let native_wait_materialization::Case::EmptyControlExpiry(_) = case {
            // One absolute budget starts before the sole original issuance,
            // including the positive phase, real expiry, and owned teardown.
            let budget = original_consent_started + std::time::Duration::from_secs(130);
            let result = tokio::time::timeout_at(
                original_consent_started + std::time::Duration::from_secs(125),
                native_wait_materialization::empty_control_expiry(
                    &db,
                    workflow.id,
                    names[index],
                    targets[index],
                    key,
                    approval.expires_at.unwrap(),
                ),
            )
            .await;
            let cleanup = tokio::time::timeout_at(budget, db.cleanup()).await;
            cleanup
                .expect("cleanup within original consent budget")
                .expect("clean owned expiry database");
            result
                .expect("case completed within original consent budget")
                .expect("empty CONTROL cold reopening must retain original finite consent");
            return;
        }
        if let native_wait_materialization::Case::Cache(_, empty) = case {
            native_wait_materialization::check_cache(
                &db,
                workflow.id,
                root.id,
                names[index],
                key,
                empty,
            )
            .await;
            db.cleanup().await.unwrap();
            return;
        }
        if matches!(
            case,
            native_wait_materialization::Case::DependentExpiry(_)
                | native_wait_materialization::Case::CacheExpiry(_)
        ) {
            native_wait_materialization::expiry(
                &db,
                workflow.id,
                names[index],
                key,
                approval.expires_at.unwrap(),
                true,
            )
            .await;
            let state = WorkflowExecutionRepository::find_by_id(&db, workflow.id)
                .await
                .unwrap()
                .unwrap();
            assert!(state.variables.get("a").is_none() && state.variables.get("b").is_none());
            let leaked:i64=sqlx::query_scalar("SELECT count(*) FROM runtime_input_receipt_staged s JOIN runtime_input_receipt r ON r.id=s.receipt WHERE r.execution=$1 AND r.destination_path IN ('/a','/b')")
                .bind(root.id).fetch_one(&db).await.unwrap();
            assert_eq!(leaked, 0);
            if matches!(case, native_wait_materialization::Case::CacheExpiry(_)) {
                assert!(attune_common::repositories::workflow_cache_iteration::WorkflowCacheIterationRepository::find_by_workflow_task(&db,workflow.id,"native_recovery").await.unwrap().is_none());
            }
            db.cleanup().await.unwrap();
            return;
        }
        if let native_wait_materialization::Case::Initial(_, mode) = case {
            native_wait_materialization::initial_lifecycle(
                &db,
                workflow.id,
                root.id,
                names[index],
                targets[index],
                key,
                mode,
                approval.expires_at.unwrap(),
            )
            .await;
            db.cleanup().await.unwrap();
            return;
        }
        native_wait_materialization::lifecycle(
            &db,
            workflow.id,
            root.id,
            names[index],
            targets[index],
            key,
            case,
        )
        .await;
        db.cleanup().await.unwrap();
        return;
    }
    if expire_while_blocked {
        native_wait_materialization::expiry(
            &db,
            workflow.id,
            names[0],
            key,
            approval.expires_at.unwrap(),
            terminal_expiry,
        )
        .await;
        db.cleanup().await.unwrap();
        return;
    }
    for index in 0..3 {
        let mut tx = db.begin().await.unwrap();
        let task = Admissions::select(
            &mut tx,
            owner.pack_id,
            task_bindings[index].clone(),
            &snapshots[&leaf.r#ref],
            std::slice::from_ref(&set_ref),
            RequestAttribution::ParentExecution {
                execution_id: root.id,
            },
            RuntimeTarget::Component(ComponentTarget::Action, graph_action.id),
        )
        .await
        .unwrap();
        let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
            .await
            .unwrap();
        let task_path = format!("/workflow/tasks/{index}");
        let selector = declarations
            .iter()
            .find(|declaration| {
                declaration
                    .source_path
                    .starts_with(&format!("{task_path}/wait_for/"))
            })
            .unwrap();
        let parameter = ["/target_execution", "/target_inquiry", "/target_queue"][index];
        let mut control_session = Inputs::begin_render(
            &mut tx,
            root.id,
            &selector.declaration_revision,
            &selector.source_path,
            &selector.expression,
            InputSitePurpose::Control,
        )
        .await
        .unwrap();
        let target_site = InputSite {
            execution: root.id,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Value,
            path: parameter.into(),
        };
        Inputs::bind_public_field(&mut tx, &mut control_session, parameter, &target_site)
            .await
            .unwrap();
        let control_render = Inputs::evaluate(&mut tx, control_session).await.unwrap();
        let control_site = InputSite {
            execution: root.id,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Control,
            path: selector.source_path.clone(),
        };
        let control = Inputs::record_render(
            &mut tx,
            &control_render,
            &control_site,
            control_render.value(),
            control_render.selections(),
            key,
        )
        .await
        .unwrap();
        assert!(
            Snapshots::prepare_wait(&mut tx, root.id, &task_path, &task, waits[index], None, key)
                .await
                .is_err(),
            "a dynamic target needs the actual evaluated target control proof"
        );
        let status_cases: &[(&str, usize)] = match index {
            0 => &[
                ("failed", 1),
                ("abandoned", 1),
                ("timeout", 2),
                ("cancelled", 3),
            ],
            1 => &[("timeout", 2), ("cancelled", 3)],
            _ => &[("failed", 1), ("skipped", 1), ("cancelled", 3)],
        };
        let update_status=match index {
            0=>"UPDATE execution SET status=$2::TEXT::execution_status_enum WHERE id=$1",
            1=>"UPDATE inquiry SET status=$2::TEXT::inquiry_status_enum WHERE id=$1",
            _=>"UPDATE work_queue_item SET status=$2::TEXT::work_queue_item_status_enum WHERE id=$1",
        };
        for &(status, expected) in status_cases {
            sqlx::query("SAVEPOINT native_wait_outcome_case")
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query(update_status)
                .bind(targets[index].id())
                .bind(status)
                .execute(&mut *tx)
                .await
                .unwrap();
            if index == 0 {
                assert!(
                    Admissions::input_authorities(&mut tx, source.id, None)
                        .await
                        .is_err(),
                    "terminal producers remain forbidden as new input-delivery recipients"
                );
            }
            let case = Snapshots::prepare_wait(
                &mut tx,
                root.id,
                &task_path,
                &task,
                waits[index],
                Some(&control),
                key,
            )
            .await
            .unwrap();
            let mut renders = Vec::new();
            for function_index in 0..4 {
                let declaration = declarations
                    .iter()
                    .find(|declaration| {
                        declaration.source_path == format!("{task_path}/next/{function_index}/when")
                    })
                    .unwrap();
                let mut session = Inputs::begin_render(
                    &mut tx,
                    root.id,
                    &declaration.declaration_revision,
                    &declaration.source_path,
                    &declaration.expression,
                    InputSitePurpose::Condition,
                )
                .await
                .unwrap();
                Inputs::bind_prepared_wait_outcome(&mut session, &case).unwrap();
                let rendered = Inputs::evaluate_condition(&mut tx, session).await.unwrap();
                assert_eq!(
                    rendered.value(),
                    &json!(function_index == expected),
                    "native {status} classification"
                );
                let site = InputSite {
                    execution: root.id,
                    kind: InputEntityKind::WorkflowVariables,
                    purpose: InputSitePurpose::Condition,
                    path: declaration.source_path.clone(),
                };
                let destination = Inputs::prepare_destination(&mut tx, &rendered, &site, key)
                    .await
                    .unwrap();
                assert!(Inputs::record_prepared(
                    &mut tx,
                    &rendered,
                    &destination,
                    rendered.value(),
                    rendered.selections(),
                    key
                )
                .await
                .is_err());
                renders.push((rendered, destination));
            }
            Snapshots::release_wait(&mut tx, case).await.unwrap();
            let replay =
                Snapshots::prepare_released_wait(&mut tx, root.id, &task_path, waits[index], key)
                    .await
                    .unwrap();
            for (function_index, (rendered, destination)) in renders.iter().enumerate() {
                let receipt = Inputs::record_prepared(
                    &mut tx,
                    rendered,
                    destination,
                    rendered.value(),
                    rendered.selections(),
                    key,
                )
                .await
                .unwrap();
                assert_eq!(receipt.native_source_metadata().len(), 1);
                let declaration = declarations
                    .iter()
                    .find(|declaration| {
                        declaration.source_path == format!("{task_path}/next/{function_index}/when")
                    })
                    .unwrap();
                let mut session = Inputs::begin_render(
                    &mut tx,
                    root.id,
                    &declaration.declaration_revision,
                    &declaration.source_path,
                    &declaration.expression,
                    InputSitePurpose::Condition,
                )
                .await
                .unwrap();
                Inputs::bind_wait_outcome(&mut session, &replay).unwrap();
                assert_eq!(
                    Inputs::evaluate_condition(&mut tx, session)
                        .await
                        .unwrap()
                        .value(),
                    rendered.value()
                );
            }
            sqlx::query("ROLLBACK TO SAVEPOINT native_wait_outcome_case")
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query("RELEASE SAVEPOINT native_wait_outcome_case")
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        let prepared = Snapshots::prepare_wait(
            &mut tx,
            root.id,
            &task_path,
            &task,
            waits[index],
            Some(&control),
            key,
        )
        .await
        .unwrap();
        let mut outcome_renders = Vec::new();
        let foreign_path = format!("/workflow/tasks/{}/next/0/when", (index + 1) % 3);
        let foreign = declarations
            .iter()
            .find(|declaration| declaration.source_path == foreign_path)
            .unwrap();
        let mut foreign_session = Inputs::begin_render(
            &mut tx,
            root.id,
            &foreign.declaration_revision,
            &foreign.source_path,
            &foreign.expression,
            InputSitePurpose::Condition,
        )
        .await
        .unwrap();
        assert!(
            Inputs::bind_prepared_wait_outcome(&mut foreign_session, &prepared).is_err(),
            "a native wait outcome belongs only to its actual task's conditions"
        );
        for function_index in 0..4 {
            let declaration = declarations
                .iter()
                .find(|declaration| {
                    declaration.source_path == format!("{task_path}/next/{function_index}/when")
                })
                .unwrap();
            let mut session = Inputs::begin_render(
                &mut tx,
                root.id,
                &declaration.declaration_revision,
                &declaration.source_path,
                &declaration.expression,
                InputSitePurpose::Condition,
            )
            .await
            .unwrap();
            Inputs::bind_prepared_wait_outcome(&mut session, &prepared).unwrap();
            let rendered = Inputs::evaluate_condition(&mut tx, session).await.unwrap();
            assert_eq!(
                rendered.value(),
                &json!(function_index == 0),
                "native completion status wins over response/result booleans"
            );
            let site = InputSite {
                execution: root.id,
                kind: InputEntityKind::WorkflowVariables,
                purpose: InputSitePurpose::Condition,
                path: declaration.source_path.clone(),
            };
            let destination = Inputs::prepare_destination(&mut tx, &rendered, &site, key)
                .await
                .unwrap();
            assert!(
                Inputs::record_prepared(
                    &mut tx,
                    &rendered,
                    &destination,
                    rendered.value(),
                    rendered.selections(),
                    key
                )
                .await
                .is_err(),
                "native wait outcome publication is unavailable before actual release"
            );
            outcome_renders.push((rendered, destination));
        }
        let declaration = declarations
            .iter()
            .find(|declaration| declaration.source_path == format!("{task_path}/params/token"))
            .unwrap();
        let mut session = Inputs::begin_render(
            &mut tx,
            root.id,
            &declaration.declaration_revision,
            &declaration.source_path,
            &declaration.expression,
            InputSitePurpose::Value,
        )
        .await
        .unwrap();
        assert!(
            Inputs::bind_prepared_wait_outcome(&mut session, &prepared).is_err(),
            "native outcome hooks cannot bind a VALUE session"
        );
        Inputs::bind_prepared_wait_input(&mut session, &prepared).unwrap();
        let rendered = Inputs::evaluate(&mut tx, session).await.unwrap();
        assert_eq!(
            rendered.value(),
            &json!(["execution-original", "inquiry-original", "queue-original"][index])
        );
        let site = InputSite {
            execution: root.id,
            kind: InputEntityKind::ExecutionResult,
            purpose: InputSitePurpose::Value,
            path: format!("/{}", names[index]),
        };
        let destination = Inputs::prepare_destination(&mut tx, &rendered, &site, key)
            .await
            .unwrap();
        assert!(
            Inputs::record_prepared(
                &mut tx,
                &rendered,
                &destination,
                rendered.value(),
                rendered.selections(),
                key
            )
            .await
            .is_err(),
            "a prepared observation is not yet an actual native release"
        );
        let pending_snapshot: Id = sqlx::query_scalar(
            "SELECT id FROM runtime_native_input_snapshot ORDER BY id DESC LIMIT 1",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert!(
            Snapshots::storage_evidence(&mut tx, pending_snapshot)
                .await
                .is_err(),
            "a pending observation has no authenticated actual release association"
        );
        let released = Snapshots::release_wait(&mut tx, prepared).await.unwrap();
        assert_eq!(released.state(), "released");
        for (rendered, destination) in &outcome_renders {
            let outcome_receipt = Inputs::record_prepared(
                &mut tx,
                rendered,
                destination,
                rendered.value(),
                rendered.selections(),
                key,
            )
            .await
            .unwrap();
            assert_eq!(outcome_receipt.native_source_metadata().len(), 1);
            assert!(
                outcome_receipt.outcome_metadata().is_empty(),
                "a native wait snapshot must not manufacture a terminal child execution admission"
            );
            outcome_references.push((outcome_receipt.site().clone(), outcome_receipt.origin()));
        }
        let receipt = Inputs::record_prepared(
            &mut tx,
            &rendered,
            &destination,
            rendered.value(),
            rendered.selections(),
            key,
        )
        .await
        .unwrap();
        assert_eq!(receipt.native_source_metadata().len(), 1);
        let frozen = Snapshots::source_evidence(&mut tx, &receipt.native_source_metadata()[0], key)
            .await
            .unwrap();
        let snapshot = receipt.native_source_metadata()[0].snapshot();
        assert_eq!(snapshot, pending_snapshot);
        let storage = Snapshots::storage_evidence(&mut tx, snapshot)
            .await
            .unwrap();
        assert_eq!(storage.metadata(), &receipt.native_source_metadata()[0]);
        assert!(storage.cache().is_none());
        assert_eq!(storage.archive_owner(), ("native_input_snapshot", snapshot));
        assert_eq!(storage.archive_fields(), frozen.fields());
        assert!(storage.contributor_manifests() == frozen.contributor_manifests());
        assert_eq!(
            serde_json::to_value(storage.contract()).unwrap(),
            serde_json::to_value(frozen.contract()).unwrap()
        );
        assert_eq!(storage.receipt_edges().len(), 1);
        assert_eq!(storage.receipt_edges()[0].1, control_site);
        let original_source_execution = [source.id, producer.id, root.id][index];
        let original_admission:Id=sqlx::query_scalar("SELECT id FROM runtime_authority_admission WHERE owner_kind='execution' AND owner_id=$1").bind(original_source_execution).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(storage.admission_edges(), &[original_admission]);
        let release = storage.wait_release().unwrap();
        assert_eq!(
            (release.wait(), release.snapshot()),
            (waits[index], snapshot)
        );
        let release_transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(release.transaction(), release_transaction);
        let stored_body: serde_json::Value =
            sqlx::query_scalar("SELECT body FROM runtime_native_input_snapshot WHERE id=$1")
                .bind(snapshot)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        let storage_bytes=serde_json::to_vec(&stored_body).unwrap().len()+serde_json::to_vec(storage.encrypted_projection()).unwrap().len()+64+storage.encryption_key_hash().len()+serde_json::to_vec(&stored_body["source"]["parents"]).unwrap().len()+serde_json::to_vec(&stored_body["source"]["fields"]).unwrap().len()+serde_json::to_vec(&json!({"budget":storage.renderer_admission(),"admissions":[original_admission],"release":[waits[index],Id::MAX,release.transaction()]})).unwrap().len();
        assert_eq!(storage.retained_bytes(),storage_bytes,"original contract/manifests are charged in the body, archive ciphertext/sites again as stored copies, plus normalized admission/release/budget associations");
        let target_parent = match targets[index] {
            WorkflowTaskWaitTarget::Execution(execution) => {
                attune_common::repositories::native_input_snapshot::NativeStorageParent::Execution {
                    execution,
                }
            }
            WorkflowTaskWaitTarget::Inquiry(inquiry) => {
                attune_common::repositories::native_input_snapshot::NativeStorageParent::Inquiry {
                    inquiry,
                }
            }
            WorkflowTaskWaitTarget::WorkQueueItem(item) => {
                attune_common::repositories::native_input_snapshot::NativeStorageParent::QueueItem {
                    item,
                }
            }
        };
        assert!(storage.physical_parents().contains(&target_parent));
        assert!(storage.physical_parents().contains(&attune_common::repositories::native_input_snapshot::NativeStorageParent::WorkflowTaskWait {wait:waits[index]}));
        assert!(storage.physical_parents().contains(
            &attune_common::repositories::native_input_snapshot::NativeStorageParent::Admission {
                admission: original_admission
            }
        ));
        let archive_ciphertext_bytes = storage
            .archive_fields()
            .iter()
            .map(|field| serde_json::to_vec(&field.encrypted_value).unwrap().len())
            .sum::<usize>();
        assert!(
            storage.retained_bytes()
                >= archive_ciphertext_bytes
                    + serde_json::to_vec(storage.encrypted_projection())
                        .unwrap()
                        .len()
        );
        assert_eq!(
            frozen.receipts().len(),
            1,
            "the actual evaluated target control remains an independent parent"
        );
        if index == 0 {
            assert_eq!(frozen.contributor_manifests().len(), 1);
            assert_eq!(frozen.contributor_manifests()[0].kind(), "execution");
            assert_eq!(
                frozen.contributor_manifests()[0].body()["identity_id"],
                json!(fixture.human_id)
            );
            assert_eq!(
                frozen.fields()[0].encrypted_value,
                original_fields[0].encrypted_value,
                "snapshot archives preserve the actual original source ciphertext bytes"
            );
        }
        if index < 2 {
            assert_eq!(frozen.fields().len(), 1);
            assert!(frozen.contract().is_some());
        } else {
            assert!(frozen.fields().is_empty());
        }
        assert!(receipt.native_source_metadata()[0]
            .read_version()
            .get("ciphertext_digest")
            .is_some());
        assert_eq!(
            receipt.native_source_metadata()[0].target().id,
            targets[index].id()
        );
        released.ensure_fresh_on(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        released.ensure_fresh().unwrap();
        receipt.ensure_fresh().unwrap();
        references.push((site, receipt.origin()));
        let mut tx = db.begin().await.unwrap();
        let condition = declarations
            .iter()
            .find(|declaration| declaration.source_path == format!("{task_path}/next/0/when"))
            .unwrap();
        let mut stale_session = Inputs::begin_render(
            &mut tx,
            root.id,
            &condition.declaration_revision,
            &condition.source_path,
            &condition.expression,
            InputSitePurpose::Condition,
        )
        .await
        .unwrap();
        assert!(Inputs::bind_wait_outcome(&mut stale_session,&released).is_err(),"an in-memory wait outcome cannot cross transactions; replay authenticates the original snapshot again");
        let released =
            Snapshots::prepare_released_wait(&mut tx, root.id, &task_path, waits[index], key)
                .await
                .unwrap();
        let mut session = Inputs::begin_render(
            &mut tx,
            root.id,
            &declaration.declaration_revision,
            &declaration.source_path,
            &declaration.expression,
            InputSitePurpose::Value,
        )
        .await
        .unwrap();
        Inputs::bind_wait_input(&mut session, &released).unwrap();
        let rebound = Inputs::evaluate(&mut tx, session).await.unwrap();
        assert_eq!(rebound.value(), rendered.value());
        sqlx::query("UPDATE workflow_task_wait SET result=$2 WHERE id=$1")
            .bind(waits[index])
            .bind(json!({"response":"caller-replacement-must-not-be-a-read-proof"}))
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(
            Snapshots::prepare_released_wait(&mut tx, root.id, &task_path, waits[index], key)
                .await
                .is_err(),
            "a replacement native wait result cannot acquire the original read/release proof"
        );
        let original_storage = Snapshots::storage_evidence(&mut tx, snapshot)
            .await
            .unwrap();
        assert_eq!(original_storage.wait_release().unwrap().wait(),waits[index],"storage inventory remains anchored to the actual immutable release, not replacement native JSON");
        assert_eq!(original_storage.metadata(), storage.metadata());
        tx.rollback().await.unwrap();
    }
    let mut gc = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut gc)
        .await
        .unwrap();
    ExecutionRepository::delete(&mut *gc, source.id)
        .await
        .unwrap();
    gc.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let observed =
        Snapshots::prepare_released_wait(&mut tx, root.id, "/workflow/tasks/0", waits[0], key)
            .await
            .unwrap();
    assert_eq!(
        observed.value()["result"]["token"],
        json!("execution-original")
    );
    let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
        .await
        .unwrap();
    for function_index in 0..4 {
        let declaration = declarations
            .iter()
            .find(|declaration| {
                declaration.source_path == format!("/workflow/tasks/0/next/{function_index}/when")
            })
            .unwrap();
        let mut session = Inputs::begin_render(
            &mut tx,
            root.id,
            &declaration.declaration_revision,
            &declaration.source_path,
            &declaration.expression,
            InputSitePurpose::Condition,
        )
        .await
        .unwrap();
        Inputs::bind_wait_outcome(&mut session, &observed).unwrap();
        assert_eq!(
            Inputs::evaluate_condition(&mut tx, session)
                .await
                .unwrap()
                .value(),
            &json!(function_index == 0),
            "replayed native outcome survives raw execution/contributor GC"
        );
    }
    for (site, origin) in &outcome_references {
        assert!(Inputs::receipt_context(&mut tx, site, origin, key)
            .await
            .is_ok());
    }
    let retained_context =
        Inputs::receipt_context(&mut tx, &references[0].0, &references[0].1, key)
            .await
            .unwrap();
    let retained_evidence =
        Snapshots::source_evidence(&mut tx, &retained_context.native_source_metadata()[0], key)
            .await
            .unwrap();
    assert_eq!(retained_evidence.contributor_manifests().len(),1,"original independent manifests survive source deletion with the encrypted snapshot, not through live authority rows");
    assert_eq!(
        retained_evidence.fields()[0].source_ref,
        original_fields[0].source_ref
    );
    assert!(attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::load(&mut tx,"execution",source.id).await.unwrap().is_none(),"exercise raw contributor cleanup, not a retained live source");
    let retained_snapshot = retained_context.native_source_metadata()[0].snapshot();
    let retained_storage = Snapshots::storage_evidence(&mut tx, retained_snapshot)
        .await
        .unwrap();
    assert_eq!(
        retained_storage.archive_fields(),
        retained_evidence.fields()
    );
    assert!(retained_storage.contributor_manifests() == retained_evidence.contributor_manifests());
    assert_eq!(retained_storage.contributor_manifests()[0].expected(), 1);
    assert_eq!(retained_storage.contributor_manifests()[0].item(), 0);
    assert_eq!(retained_storage.contributor_manifests()[0].name(), "");
    assert_eq!(retained_storage.wait_release().unwrap().wait(), waits[0]);
    for (site, origin) in &references {
        assert!(Inputs::receipt_context(&mut tx, site, origin, key)
            .await
            .is_ok());
    }
    let source_admission = RuntimeAuthorityRepository::execution_admission(&mut tx, source.id)
        .await
        .unwrap()
        .unwrap();
    RuntimeAuthorityRepository::revoke_admission(&mut tx, source_admission.id)
        .await
        .unwrap();
    assert!(Snapshots::storage_evidence(&mut tx,retained_snapshot).await.is_ok(),"revocation denies reads/effects, not the keyless inventory needed for safe physical retention");
    assert!(Inputs::receipt_context(&mut tx,&references[0].0,&references[0].1,key).await.is_err(),"the original completed source admission remains an independent revocation dependency after raw GC");
    for (index, (site, origin)) in outcome_references.iter().enumerate() {
        assert_eq!(
            Inputs::receipt_context(&mut tx, site, origin, key)
                .await
                .is_err(),
            index < 4,
            "native outcomes retain the original independent source admission"
        );
    }
    for (site, origin) in &references[1..] {
        assert!(Inputs::receipt_context(&mut tx, site, origin, key)
            .await
            .is_ok());
    }
    tx.rollback().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    RuntimeAuthorityRepository::revoke_approval(&mut tx, approval.id)
        .await
        .unwrap();
    for (site, origin) in &outcome_references {
        assert!(
            Inputs::receipt_context(&mut tx, site, origin, key)
                .await
                .is_err(),
            "native outcomes retain original selected approval dependencies"
        );
    }
    for (site, origin) in &references {
        assert!(Inputs::receipt_context(&mut tx, site, origin, key)
            .await
            .is_err());
    }
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_wait_successor_timeout_uses_real_predecessor_and_settles_root() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::Initial(
            0,
            native_wait_materialization::InitialCase::Successor(0),
        )),
    )
    .await;
}

#[tokio::test]
async fn native_wait_successor_rejects_changed_original_workflow() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::Initial(
            0,
            native_wait_materialization::InitialCase::Successor(1),
        )),
    )
    .await;
}

#[tokio::test]
async fn native_wait_successor_rejects_foreign_source_transaction() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::Initial(
            0,
            native_wait_materialization::InitialCase::Successor(2),
        )),
    )
    .await;
}

#[tokio::test]
async fn native_wait_successor_unconditional_retains_real_outcome_control() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::Initial(
            0,
            native_wait_materialization::InitialCase::Successor(3),
        )),
    )
    .await;
}

#[tokio::test]
async fn native_wait_successor_expiry_after_actual_target_wait_rolls_back_handoff() {
    native_wait_snapshot_fixture(
        true,
        false,
        Some(native_wait_materialization::Case::Initial(
            0,
            native_wait_materialization::InitialCase::Successor(4),
        )),
    )
    .await;
}

#[tokio::test]
async fn native_wait_successor_does_not_resurrect_completed_target_or_rerender() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::Initial(
            0,
            native_wait_materialization::InitialCase::Successor(5),
        )),
    )
    .await;
}

#[tokio::test]
async fn human_deletion_preserves_recorded_approval_and_requester_attribution() {
    let (db, fixture) = fixture().await;
    assert!(IdentityRepository::delete(&db, fixture.human_id)
        .await
        .unwrap());
    let mut tx = db.begin().await.unwrap();
    let approval = RuntimeAuthorityRepository::find_approval(&mut tx, fixture.approval.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(approval.issuer_identity, fixture.human_id);
    assert!(!approval.issuer_login.is_empty());
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await
            .unwrap();
    assert_eq!(
        authority.authority().attribution,
        fixture.admission.authority.attribution
    );
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn explicit_approval_revocation_denies_an_existing_execution_credential() {
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    assert!(
        RuntimeAuthorityRepository::revoke_approval(&mut tx, fixture.approval.id)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    assert!(RuntimeAuthorityRepository::validate_execution_credential(
        &mut tx,
        &fixture.credential
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn policy_revocation_is_distinct_from_human_activity() {
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    assert!(
        RuntimeAuthorityRepository::revoke_policy(&mut tx, fixture.policy.id)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    assert!(RuntimeAuthorityRepository::validate_execution_credential(
        &mut tx,
        &fixture.credential
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn admission_revocation_and_terminal_status_each_stop_runtime_effects() {
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    RuntimeAuthorityRepository::revoke_admission(&mut tx, fixture.admission.id)
        .await
        .unwrap();
    assert!(RuntimeAuthorityRepository::validate_execution_credential(
        &mut tx,
        &fixture.credential
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    ExecutionRepository::update(
        &db,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    assert!(RuntimeAuthorityRepository::validate_execution_credential(
        &mut tx,
        &fixture.credential
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn wrong_action_subject_attempt_and_admission_are_denied() {
    let (db, fixture) = fixture().await;
    let mut wrong_action = fixture.credential.clone();
    wrong_action.action_ref = "shared.unadmitted".into();
    let mut wrong_subject = fixture.credential.clone();
    wrong_subject.sponsor_identity_id = fixture.human_id;
    let mut wrong_attempt = fixture.credential.clone();
    wrong_attempt.attempt = uuid::Uuid::new_v4();
    let mut wrong_admission = fixture.credential.clone();
    wrong_admission.admission_id = i64::MAX;
    let mut tx = db.begin().await.unwrap();
    for credential in [wrong_action, wrong_subject, wrong_attempt, wrong_admission] {
        assert!(
            RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &credential)
                .await
                .is_err()
        );
    }
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn worker_attempt_rotation_fences_old_tokens_and_cannot_replay_retired_attempts() {
    let (db, fixture) = fixture().await;
    let worker = WorkerRepository::create(
        &db,
        CreateWorkerInput {
            name: format!("runtime-authority-{}", uuid::Uuid::new_v4()),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: None,
            meta: None,
        },
    )
    .await
    .unwrap();
    ExecutionRepository::update(
        &db,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            worker: Some(worker.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let attempt = uuid::Uuid::new_v4();
    let credential = RuntimeAuthorityRepository::begin_execution_attempt(
        &mut tx,
        fixture.credential.execution_id,
        worker.id,
        attempt,
    )
    .await
    .unwrap();
    assert_eq!(credential.attempt, attempt);
    assert_eq!(
        RuntimeAuthorityRepository::begin_execution_attempt(
            &mut tx,
            fixture.credential.execution_id,
            worker.id,
            attempt,
        )
        .await
        .unwrap(),
        credential
    );
    assert!(RuntimeAuthorityRepository::begin_execution_attempt(
        &mut tx,
        fixture.credential.execution_id,
        worker.id,
        fixture.credential.attempt,
    )
    .await
    .is_err());
    assert!(RuntimeAuthorityRepository::begin_execution_attempt(
        &mut tx,
        fixture.credential.execution_id,
        i64::MAX,
        uuid::Uuid::new_v4(),
    )
    .await
    .is_err());
    assert!(RuntimeAuthorityRepository::validate_execution_credential(
        &mut tx,
        &fixture.credential
    )
    .await
    .is_err());
    let validated = RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &credential)
        .await
        .unwrap();
    assert_eq!(
        validated.authority().grants,
        fixture.admission.authority.grants
    );
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn system_sponsor_suspension_is_an_explicit_operational_control() {
    let (db, fixture) = fixture().await;
    IdentityRepository::update(
        &db,
        fixture.credential.sponsor_identity_id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    assert!(RuntimeAuthorityRepository::validate_execution_credential(
        &mut tx,
        &fixture.credential
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn policy_capture_and_execution_admission_are_idempotent_but_not_rebindable() {
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let repeated_policy =
        RuntimeAuthorityRepository::capture_policy(&mut tx, &fixture.policy.policy)
            .await
            .unwrap();
    assert_eq!(repeated_policy.id, fixture.policy.id);
    let mut repeated = fixture.admission.authority.0.clone();
    repeated.attempt = uuid::Uuid::new_v4();
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(fixture.credential.execution_id),
        &repeated,
    )
    .await
    .unwrap();
    assert_eq!(admission.id, fixture.admission.id);
    assert_eq!(admission.authority.attempt, fixture.credential.attempt);
    repeated.attribution = RequestAttribution::ParentExecution { execution_id: 123 };
    assert!(RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(fixture.credential.execution_id),
        &repeated
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn execution_retention_removes_admission_and_denies_old_credentials() {
    let (db, fixture) = fixture().await;
    assert!(
        ExecutionRepository::delete(&db, fixture.credential.execution_id)
            .await
            .unwrap()
    );
    let mut tx = db.begin().await.unwrap();
    assert!(RuntimeAuthorityRepository::validate_execution_credential(
        &mut tx,
        &fixture.credential
    )
    .await
    .is_err());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM runtime_authority_admission WHERE id = $1")
            .bind(fixture.admission.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(count, 0);
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn database_guards_keep_grants_immutable_and_revocation_irreversible() {
    let (db, fixture) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let error = sqlx::query("UPDATE runtime_external_approval SET grants = '[]' WHERE id = $1")
        .bind(fixture.approval.id)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("immutable"));
    tx.rollback().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    RuntimeAuthorityRepository::revoke_approval(&mut tx, fixture.approval.id)
        .await
        .unwrap();
    let error = sqlx::query("UPDATE runtime_external_approval SET revoked_at = NULL WHERE id = $1")
        .bind(fixture.approval.id)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cannot be reversed"));
    tx.rollback().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let error = sqlx::query(
        "UPDATE runtime_external_approval_target_binding SET binding = '{}' WHERE approval = $1",
    )
    .bind(fixture.approval.id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(error.to_string().contains("immutable"));
    tx.rollback().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let error=sqlx::query("UPDATE execution SET executable_snapshot = jsonb_set(executable_snapshot,'{executable,action,entrypoint}','\"forged.sh\"') WHERE id = $1")
        .bind(fixture.credential.execution_id).execute(&mut *tx).await.unwrap_err();
    assert!(error.to_string().contains("immutable"));
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_wait_no_recovery_execution_timeout_records_typed_output() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::Initial(
            0,
            native_wait_materialization::InitialCase::TerminalOutput,
        )),
    )
    .await;
}
#[tokio::test]
async fn native_wait_no_recovery_inquiry_cancel_records_typed_output() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::Initial(
            1,
            native_wait_materialization::InitialCase::TerminalOutput,
        )),
    )
    .await;
}
#[tokio::test]
async fn native_wait_no_recovery_queue_failure_records_typed_output() {
    native_wait_snapshot_fixture(
        false,
        false,
        Some(native_wait_materialization::Case::Initial(
            2,
            native_wait_materialization::InitialCase::TerminalOutput,
        )),
    )
    .await;
}
