mod helpers;

use attune_common::{
    models::OwnerType,
    repositories::{
        action::{ActionRepository, CreateActionInput},
        runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
    },
};
use chrono::{Duration, Utc};
use helpers::{
    create_test_pool, ActionFixture, EventFixture, KeyFixture, PackFixture, TriggerFixture,
};

#[tokio::test]
async fn prospective_key_creation_resolves_each_real_owner_without_writing_a_key() {
    use attune_common::repositories::{
        action::UpdateActionInput,
        key::{KeyRepository, KeySearchFilters},
        runtime_target::{ProspectiveDataOwner, ProspectiveRuntimeTarget},
    };
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("new_key_owner")
        .create(&db)
        .await
        .unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "owner")
        .create(&db)
        .await
        .unwrap();
    let runtime = helpers::RuntimeFixture::new(Some(pack.id), Some(pack.r#ref.clone()), "native")
        .create(&db)
        .await
        .unwrap();
    let sensor = helpers::SensorFixture::new(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref,
        "owner",
    )
    .create(&db)
    .await
    .unwrap();
    let human = helpers::IdentityFixture::new_unique("new_key_human")
        .create(&db)
        .await
        .unwrap();
    let mut transaction = db.begin().await.unwrap();
    for (owner, kind, id, reference, expected_pack) in [
        (
            ProspectiveDataOwner::System,
            OwnerType::System,
            None,
            None,
            None,
        ),
        (
            ProspectiveDataOwner::Identity(human.id),
            OwnerType::Identity,
            Some(human.id),
            Some(human.login.clone()),
            None,
        ),
        (
            ProspectiveDataOwner::Pack(pack.id),
            OwnerType::Pack,
            Some(pack.id),
            Some(pack.r#ref.clone()),
            Some(pack.id),
        ),
        (
            ProspectiveDataOwner::Action(action.id),
            OwnerType::Action,
            Some(action.id),
            Some(action.r#ref.clone()),
            Some(pack.id),
        ),
        (
            ProspectiveDataOwner::Sensor(sensor.id),
            OwnerType::Sensor,
            Some(sensor.id),
            Some(sensor.r#ref.clone()),
            Some(pack.id),
        ),
    ] {
        let target = RuntimeTargetRepository::resolve_creation_target(
            &mut transaction,
            owner,
            ProspectiveRuntimeTarget::Key {
                local_ref: "future",
                encrypted: true,
            },
            human.id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(target.owner_id(), id);
        assert_eq!(target.pack_id(), expected_pack);
        assert_eq!(target.context().target_id, None);
        assert_eq!(target.context().owner_type, Some(kind));
        assert_eq!(target.context().encrypted, Some(true));
        assert_eq!(
            target.context().target_ref.as_deref(),
            Some(
                attune_common::key_ref::canonical_key_ref(kind, reference.as_deref(), "future")
                    .unwrap()
                    .as_str()
            )
        );
        assert!(target.accepts_new_effects());
        let cache = RuntimeTargetRepository::resolve_creation_target(
            &mut transaction,
            owner,
            ProspectiveRuntimeTarget::CacheNamespace("future"),
            human.id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(cache.owner_id(), id);
        assert_eq!(cache.pack_id(), expected_pack);
        assert_eq!(cache.context().target_ref.as_deref(), Some("future"));
        assert_eq!(cache.context().encrypted, None);
        assert_eq!(cache.resource(), attune_common::rbac::Resource::Caches);
    }
    for owner in [
        ProspectiveDataOwner::Pack(i64::MAX),
        ProspectiveDataOwner::Action(i64::MAX),
        ProspectiveDataOwner::Sensor(i64::MAX),
        ProspectiveDataOwner::Identity(i64::MAX),
    ] {
        assert!(RuntimeTargetRepository::resolve_creation_target(
            &mut transaction,
            owner,
            ProspectiveRuntimeTarget::Key {
                local_ref: "future",
                encrypted: false
            },
            human.id
        )
        .await
        .unwrap()
        .is_none());
    }
    transaction.commit().await.unwrap();
    ActionRepository::update(
        &db,
        action.id,
        UpdateActionInput {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut transaction = db.begin().await.unwrap();
    let target = RuntimeTargetRepository::resolve_creation_target(
        &mut transaction,
        ProspectiveDataOwner::Action(action.id),
        ProspectiveRuntimeTarget::Key {
            local_ref: "future",
            encrypted: false,
        },
        human.id,
    )
    .await
    .unwrap()
    .unwrap();
    // Ordinary invocation disablement does not revoke an admitted run's
    // authority over data owned by its retained action identity.
    assert!(target.accepts_new_effects());
    transaction.commit().await.unwrap();
    assert_eq!(
        KeyRepository::search(
            &*db,
            &KeySearchFilters {
                owner_type: None,
                owner: None,
                limit: 10,
                offset: 0,
                visibility: None
            }
        )
        .await
        .unwrap()
        .total,
        0
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn target_resolution_uses_structural_ownership_not_an_action_ref_prefix() {
    let db = create_test_pool().await.unwrap();
    let namespace = PackFixture::new_unique("namespace")
        .create(&db)
        .await
        .unwrap();
    let owner = PackFixture::new_unique("actual_owner")
        .create(&db)
        .await
        .unwrap();
    // New schema admission rejects forged namespace/owner combinations before
    // persistence. Resolution still takes its ownership from the actual row.
    assert!(
        ActionFixture::new(owner.id, &namespace.r#ref, "different_owner")
            .create(&db)
            .await
            .is_err()
    );
    let action = ActionFixture::new(owner.id, &owner.r#ref, "actual_owner")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let resolved = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Component(ComponentTarget::Action, action.id),
        1,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(resolved.pack_id(), Some(owner.id));
    assert_ne!(resolved.pack_id(), Some(namespace.id));
    assert_eq!(
        resolved.context().pack_ref.as_deref(),
        Some(owner.r#ref.as_str())
    );
    assert_eq!(
        resolved.context().target_ref.as_deref(),
        Some(action.r#ref.as_str())
    );
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn keys_resolve_canonical_owners_and_never_give_system_or_human_keys_a_local_pack() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("key_owner")
        .create(&db)
        .await
        .unwrap();
    let human = helpers::IdentityFixture::new_unique("key_human")
        .create(&db)
        .await
        .unwrap();
    let owned = KeyFixture::new_pack(pack.id, &pack.r#ref, "owned", "value")
        .create(&db)
        .await
        .unwrap();
    let system = KeyFixture::new_system("system_key", "value")
        .create(&db)
        .await
        .unwrap();
    let private = KeyFixture::new_identity(human.id, "private", "value")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let resolved = RuntimeTargetRepository::resolve(&mut tx, RuntimeTarget::Key(owned.id), 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.pack_id(), Some(pack.id));
    assert_eq!(resolved.context().owner_type, Some(OwnerType::Pack));
    assert_eq!(resolved.context().encrypted, Some(false));
    for key in [system, private] {
        let resolved = RuntimeTargetRepository::resolve(&mut tx, RuntimeTarget::Key(key.id), 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.pack_id(), None);
        assert_eq!(resolved.context().owner_type, Some(key.owner_type));
        assert_eq!(resolved.context().owner_identity_id, key.owner_identity);
    }
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn disabled_targets_preserve_ownership_but_reject_new_effects() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("disabled_target")
        .create(&db)
        .await
        .unwrap();
    let trigger = TriggerFixture::new(Some(pack.id), Some(pack.r#ref.clone()), "disabled")
        .with_enabled(false)
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let resolved =
        RuntimeTargetRepository::resolve(&mut tx, RuntimeTarget::EventTrigger(trigger.id), 1)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(resolved.pack_id(), Some(pack.id));
    assert!(!resolved.accepts_new_effects());
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn event_resolution_requires_the_exact_partitioned_row() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("event_owner")
        .create(&db)
        .await
        .unwrap();
    let trigger = TriggerFixture::new(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&db)
        .await
        .unwrap();
    let event = EventFixture::new(Some(trigger.id), &trigger.r#ref)
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let resolved = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Event {
            id: event.id,
            created: event.created,
        },
        1,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(resolved.pack_id(), Some(pack.id));
    assert!(RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Event {
            id: event.id,
            created: event.created + Duration::microseconds(1)
        },
        1
    )
    .await
    .unwrap()
    .is_none());
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn ad_hoc_components_do_not_inherit_pack_authority_from_namespace_association() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("adhoc_namespace")
        .create(&db)
        .await
        .unwrap();
    let action = ActionRepository::create(
        &db,
        CreateActionInput {
            r#ref: format!("{}.adhoc", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Ad hoc".into(),
            description: None,
            entrypoint: "run.sh".into(),
            runtime: None,
            enabled: true,
            runtime_version_constraint: None,
            required_worker_runtimes: serde_json::json!({}),
            worker_selector: serde_json::json!({}),
            worker_tolerations: serde_json::json!([]),
            worker_affinity: serde_json::json!({}),
            param_schema: None,
            out_schema: None,
            is_adhoc: true,
            accesses_mcp: false,
            default_execution_permission_set_refs: Vec::new(),
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let resolved = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Component(ComponentTarget::Action, action.id),
        1,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(resolved.pack_id(), None);
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn missing_targets_are_not_local_and_every_resolver_query_runs() {
    let db = create_test_pool().await.unwrap();
    let id = i64::MAX;
    let mut targets = vec![
        RuntimeTarget::Pack(id),
        RuntimeTarget::Execution(id),
        RuntimeTarget::Enforcement(id),
        RuntimeTarget::QueueItem(id),
        RuntimeTarget::Inquiry(id),
        RuntimeTarget::Key(id),
        RuntimeTarget::Artifact(id),
        RuntimeTarget::CacheNamespace(id),
        RuntimeTarget::RuntimeVersion(id),
        RuntimeTarget::Event {
            id,
            created: Utc::now(),
        },
        RuntimeTarget::EventTrigger(id),
    ];
    targets.extend(
        [
            ComponentTarget::Action,
            ComponentTarget::Policy,
            ComponentTarget::Queue,
            ComponentTarget::Rule,
            ComponentTarget::Trigger,
            ComponentTarget::Sensor,
            ComponentTarget::Runtime,
            ComponentTarget::Dashboard,
        ]
        .map(|kind| RuntimeTarget::Component(kind, id)),
    );
    let mut tx = db.begin().await.unwrap();
    for target in targets {
        assert!(
            RuntimeTargetRepository::resolve(&mut tx, target, 1)
                .await
                .unwrap()
                .is_none(),
            "{target:?}"
        );
    }
    assert!(
        RuntimeTargetRepository::resolve(&mut tx, RuntimeTarget::Pack(0), 1)
            .await
            .is_err()
    );
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}
