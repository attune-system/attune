mod helpers;

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
        Create, Delete, Update,
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

struct Fixture {
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
            content_path: format!("/packs/.releases/{digest}"),
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
    let executable = ExecutableSnapshotRepository::resolve_for_action_ref(
        &mut *tx,
        &fixture.credential.action_ref,
    )
    .await
    .unwrap();
    let policy = plan
        .compile(
            fixture.policy.policy.owner.clone(),
            &executable.release,
            &BTreeMap::from([(fixture.credential.action_ref.clone(), executable.clone())]),
            &[],
            &BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(policy.invocations.len(), 1);
    let set = &policy.invocations[0].permission_sets[&reference];
    assert!(
        matches!(set.grants[0], attune_common::runtime_authority::AdmittedGrant::PackOwned { pack_id, .. }
        if pack_id == fixture.policy.policy.owner.pack_id)
    );
    assert!(!policy.invocations[0].standard_access);
    let revision = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    assert_ne!(revision.id, fixture.policy.id);
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
    let (db, fixture) = fixture().await;
    let target_pack = PackFixture::new_unique("authority_target")
        .create(&db)
        .await
        .unwrap();
    let target_action = ActionFixture::new(target_pack.id, &target_pack.r#ref, "run")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let target = activate_release(
        &mut tx,
        target_pack.id,
        &target_pack.r#ref,
        vec![target_action.id],
        'b',
    )
    .await;
    let mut expanded = fixture.policy.policy.0.clone();
    expanded.invocations.push(InvocationPolicy {
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
    activate_release(
        &mut tx,
        target_pack.id,
        &target_pack.r#ref,
        vec![target_action.id],
        'c',
    )
    .await;
    let next = activate_release(
        &mut tx,
        expanded.owner.pack_id,
        &expanded.owner.pack_ref,
        vec![snapshot.executable.action.id],
        'd',
    )
    .await;
    let replacement = RuntimePolicy {
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
    let packs = tempfile::tempdir().unwrap();
    let cutoff = Utc::now() + Duration::hours(1);
    let retained = PackRetentionRepository::collect(&db, packs.path(), cutoff, 0, 20)
        .await
        .unwrap();
    assert_eq!(retained.deleted, 0);
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
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("runtime_authority")
        .create(&db)
        .await
        .unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "emit")
        .create(&db)
        .await
        .unwrap();
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
            content_path: "/packs/.releases/runtime-authority".into(),
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
        None,
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
        owner,
        release_id: release.id,
        release_digest: release.digest.clone(),
        invocations: vec![InvocationPolicy {
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
            &[set_ref.clone()],
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
    let mut context = AuthorizationContext::new(fixture.credential.sponsor_identity_id);
    context.target_ref = Some("shared.completed".into());
    assert!(validated.allows(None, Resource::Events, Action::Create, &context));
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
    db.cleanup().await.unwrap();
}
