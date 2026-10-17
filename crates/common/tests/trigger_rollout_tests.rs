mod helpers;

use attune_common::{
    models::ExecutionStatus,
    repositories::{
        component_lifecycle::PackProjectionIds,
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        rule_authority::RuleAuthorityRepository,
        runtime_authority::{RuntimeAuthorityRepository, RuntimeOwner},
        trigger_rollout::TriggerRolloutRepository,
        Update,
    },
    runtime_authority::{
        HumanAttribution, InvocationBinding, InvocationPolicy, PermissionSetDefinition,
        RequestAttribution, RuntimePolicy,
    },
};
use helpers::{create_test_pool, ActionFixture, IdentityFixture, PackFixture, TriggerFixture};
use serde_json::json;
use std::collections::BTreeMap;

#[tokio::test]
async fn live_admitted_emitter_requires_explicit_acknowledgement_until_drained() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("rollout")
        .create(&db)
        .await
        .unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "emit")
        .create(&db)
        .await
        .unwrap();
    let trigger = TriggerFixture::new(Some(pack.id), Some(pack.r#ref.clone()), "tick")
        .create(&db)
        .await
        .unwrap();
    let human = IdentityFixture::new_unique("rollout_operator")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: "1.0.0".into(),
            digest: "a".repeat(64),
            object_key: "packs/blobs/rollout.tar.gz".into(),
            provider_version: "e:rollout".into(),
            content_path: "/packs/.releases/rollout".into(),
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
            triggers: vec![trigger.id],
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
    let definition: PermissionSetDefinition = serde_json::from_value(json!({
        "ref":format!("{}.emit_events", pack.r#ref),
        "grants":[{"resource":"events","actions":["create"],"constraints":{"refs":[trigger.r#ref]}}]
    }))
    .unwrap();
    let set = definition.compile(&owner, &[]).unwrap();
    let set_ref = set.reference.clone();
    let binding = InvocationBinding::Action {
        action_ref: action.r#ref.clone(),
    };
    let policy = RuntimeAuthorityRepository::capture_policy(
        &mut tx,
        &RuntimePolicy {
            concrete_targets: BTreeMap::new(),
            owner,
            release_id: release.id,
            release_digest: release.digest.clone(),
            invocations: vec![InvocationPolicy {
                source_id: None,
                metadata_revision: None,
                foreign_workflow_policy: None,
                dispatch_template_digest: None,
                binding: binding.clone(),
                target_release_id: release.id,
                target_release_digest: release.digest,
                permission_sets: BTreeMap::from([(set_ref.clone(), set)]),
                standard_access: false,
            }],
        },
    )
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
            action_ref: action.r#ref,
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
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(execution.id),
        &selected,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    TriggerRolloutRepository::lock_activation(&mut tx)
        .await
        .unwrap();
    let unchanged = TriggerRolloutRepository::preflight(&mut tx, trigger.id, None, None)
        .await
        .unwrap();
    assert!(!unchanged.requires_acknowledgement());
    // A named dependency can change while the authored trigger field map stays
    // byte-for-byte identical. The candidate planner must use the resolved hook.
    use attune_common::value_schema::{
        compile, Limits, RegistryEntry, SchemaContext, SourceIdentity,
    };
    let type_ref = format!("{}.payload", pack.r#ref);
    let authored = json!({"value":{"type":type_ref}});
    let source = SourceIdentity::Authoring {
        component_ref: trigger.r#ref.clone(),
    };
    let old = compile(
        source.clone(),
        SchemaContext::FlatDocument,
        &authored,
        &BTreeMap::from([(
            type_ref.clone(),
            RegistryEntry::Candidate(json!({"type":"string"})),
        )]),
        Limits::default(),
    )
    .unwrap();
    let new = compile(
        source,
        SchemaContext::FlatDocument,
        &authored,
        &BTreeMap::from([(
            type_ref,
            RegistryEntry::Candidate(json!({"type":"integer"})),
        )]),
        Limits::default(),
    )
    .unwrap();
    assert_ne!(
        old.semantic_digest().unwrap(),
        new.semantic_digest().unwrap()
    );
    let named_change = TriggerRolloutRepository::preflight_changed_contract(&mut tx, trigger.id)
        .await
        .unwrap();
    assert_eq!(named_change.execution_ids, [execution.id]);
    assert!(named_change.require_drained_or_acknowledged(false).is_err());
    assert!(named_change.require_drained_or_acknowledged(true).is_ok());
    let changed = json!({"value":{"type":"integer","required":true}});
    let impact = TriggerRolloutRepository::preflight(&mut tx, trigger.id, Some(&changed), None)
        .await
        .unwrap();
    assert_eq!(impact.execution_ids, [execution.id]);
    assert!(impact.require_drained_or_acknowledged(false).is_err());
    assert!(impact.require_drained_or_acknowledged(true).is_ok());
    // The activation fence excludes new admissions under shared graph selection.
    let mut concurrent = db.begin().await.unwrap();
    let admitted: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock_shared(hashtextextended(current_schema() || ':named-type-graph', 0))")
        .fetch_one(&mut *concurrent).await.unwrap();
    assert!(!admitted);
    concurrent.rollback().await.unwrap();
    tx.rollback().await.unwrap();

    ExecutionRepository::update(
        &db,
        execution.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    TriggerRolloutRepository::lock_activation(&mut tx)
        .await
        .unwrap();
    let drained = TriggerRolloutRepository::preflight(&mut tx, trigger.id, Some(&changed), None)
        .await
        .unwrap();
    assert!(!drained.requires_acknowledgement());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}
