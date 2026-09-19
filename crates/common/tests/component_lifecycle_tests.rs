mod helpers;

use attune_common::config::CacheAdmissionConfig;
use attune_common::models::AbsentMetadataPolicy;
use attune_common::pack_registry::PackComponentLoader;
use attune_common::platform_catalog::ManagedComponentKind;
use attune_common::repositories::{
    action::{ActionRepository, UpdateActionInput},
    component_lifecycle::{ComponentLifecycleRepository, PackProjectionIds},
    identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    },
    platform_catalog::PlatformCatalogRepository,
    Create, FindById, FindByRef, Update,
};
use helpers::{create_test_pool, ActionFixture, IdentityFixture, PackFixture};
use serde_json::json;

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn absence_policies_preserve_overrides_and_gate_omitted_components() {
    let pool = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("absence-policy")
        .create(&pool)
        .await
        .unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "omitted")
        .create(&pool)
        .await
        .unwrap();
    let permission_set = PermissionSetRepository::create(
        &pool,
        CreatePermissionSetInput {
            r#ref: format!("{}.retained_grant", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: None,
            description: None,
            grants: json!([]),
        },
    )
    .await
    .unwrap();
    let mut connection = pool.acquire().await.unwrap();
    ActionRepository::update(
        &mut *connection,
        action.id,
        UpdateActionInput {
            enabled: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    ComponentLifecycleRepository::reconcile_omissions_with_policy(
        &mut connection,
        pack.id,
        &PackProjectionIds::default(),
        AbsentMetadataPolicy::Disable,
    )
    .await
    .unwrap();

    let action_state: (bool, Option<bool>, bool, bool) = sqlx::query_as(
        "SELECT omission_disabled, enabled_override, effective_enabled, retired_at IS NULL FROM action WHERE id = $1",
    )
    .bind(action.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(action_state, (true, Some(true), false, true));
    let permission_retired: bool =
        sqlx::query_scalar("SELECT retired_at IS NOT NULL FROM permission_set WHERE id = $1")
            .bind(permission_set.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!permission_retired);

    ComponentLifecycleRepository::reconcile_omissions_with_policy(
        &mut connection,
        pack.id,
        &PackProjectionIds::default(),
        AbsentMetadataPolicy::Retain,
    )
    .await
    .unwrap();
    let still_disabled: bool =
        sqlx::query_scalar("SELECT omission_disabled FROM action WHERE id = $1")
            .bind(action.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(still_disabled);

    ComponentLifecycleRepository::reconcile_omissions_with_policy(
        &mut connection,
        pack.id,
        &PackProjectionIds {
            actions: vec![action.id],
            permission_sets: vec![permission_set.id],
            ..Default::default()
        },
        AbsentMetadataPolicy::Retain,
    )
    .await
    .unwrap();
    let restored: (bool, Option<bool>, bool) = sqlx::query_as(
        "SELECT omission_disabled, enabled_override, effective_enabled FROM action WHERE id = $1",
    )
    .bind(action.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(restored, (false, Some(true), true));
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn retire_and_reactivate_preserves_ids_assignments_and_refs() {
    let pool = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("lifecycle")
        .create(&pool)
        .await
        .unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "retained_action")
        .create(&pool)
        .await
        .unwrap();
    let permission_set = PermissionSetRepository::create(
        &pool,
        CreatePermissionSetInput {
            r#ref: format!("{}.retained_grant", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: Some("Retained grant".to_string()),
            description: None,
            grants: json!([{"resource": "actions", "actions": ["execute"]}]),
        },
    )
    .await
    .unwrap();
    let identity = IdentityFixture::new("lifecycle-user")
        .create(&pool)
        .await
        .unwrap();
    let assignment = PermissionAssignmentRepository::create(
        &pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: permission_set.id,
        },
    )
    .await
    .unwrap();

    let mut connection = pool.acquire().await.unwrap();
    let overridden = ActionRepository::update(
        &mut *connection,
        action.id,
        UpdateActionInput {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!overridden.enabled);
    ComponentLifecycleRepository::set_declared_enabled(
        &mut connection,
        ManagedComponentKind::Action,
        action.id,
        true,
    )
    .await
    .unwrap();
    let enabled_state: (bool, Option<bool>, bool) = sqlx::query_as(
        "SELECT enabled, enabled_override, effective_enabled FROM action WHERE id = $1",
    )
    .bind(action.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(enabled_state, (true, Some(false), false));

    ComponentLifecycleRepository::reconcile_omissions(
        &mut connection,
        pack.id,
        &PackProjectionIds::default(),
    )
    .await
    .unwrap();

    let retired_action: (i64, bool) =
        sqlx::query_as("SELECT id, retired_at IS NOT NULL FROM action WHERE ref = $1")
            .bind(&action.r#ref)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(retired_action, (action.id, true));
    let other_pack = PackFixture::new_unique("other-owner")
        .create(&pool)
        .await
        .unwrap();
    assert!(PlatformCatalogRepository::ensure_pack_owner(
        &mut connection,
        ManagedComponentKind::Action,
        &action.r#ref,
        other_pack.id,
    )
    .await
    .is_err());
    assert!(ActionRepository::find_by_ref(&pool, &action.r#ref)
        .await
        .unwrap()
        .is_none());
    assert!(ActionRepository::find_by_id(&pool, action.id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        ActionRepository::find_by_id_including_retired(&pool, action.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        action.id
    );
    assert_eq!(
        ActionRepository::find_by_ref_including_retired(&pool, &action.r#ref)
            .await
            .unwrap()
            .unwrap()
            .id,
        action.id
    );
    assert!(
        PermissionSetRepository::find_by_ref(&pool, &permission_set.r#ref)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        PermissionSetRepository::find_by_ref_including_retired(&pool, &permission_set.r#ref)
            .await
            .unwrap()
            .unwrap()
            .id,
        permission_set.id
    );
    assert!(
        PermissionSetRepository::find_by_identity(&pool, identity.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        PermissionAssignmentRepository::find_by_id(&pool, assignment.id)
            .await
            .unwrap()
            .unwrap()
            .permset,
        permission_set.id
    );

    ComponentLifecycleRepository::reconcile_omissions(
        &mut connection,
        pack.id,
        &PackProjectionIds {
            actions: vec![action.id],
            permission_sets: vec![permission_set.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let restored_action: (i64, bool) =
        sqlx::query_as("SELECT id, retired_at IS NULL FROM action WHERE ref = $1")
            .bind(&action.r#ref)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(restored_action, (action.id, true));
    assert_eq!(
        ActionRepository::find_by_ref(&pool, &action.r#ref)
            .await
            .unwrap()
            .unwrap()
            .id,
        action.id
    );
    assert_eq!(
        PermissionSetRepository::find_by_identity(&pool, identity.id)
            .await
            .unwrap()
            .into_iter()
            .map(|permission_set| permission_set.id)
            .collect::<Vec<_>>(),
        vec![permission_set.id]
    );
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn lists_only_retired_components_owned_by_the_pack() {
    let pool = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("retired-list")
        .create(&pool)
        .await
        .unwrap();
    let other_pack = PackFixture::new_unique("retired-list-other")
        .create(&pool)
        .await
        .unwrap();
    let retired = ActionFixture::new_unique(pack.id, &pack.r#ref, "retired")
        .create(&pool)
        .await
        .unwrap();
    let active = ActionFixture::new_unique(pack.id, &pack.r#ref, "active")
        .create(&pool)
        .await
        .unwrap();
    let other = ActionFixture::new_unique(other_pack.id, &other_pack.r#ref, "retired")
        .create(&pool)
        .await
        .unwrap();

    sqlx::query("UPDATE action SET management_origin = 'pack' WHERE id = ANY($1::BIGINT[])")
        .bind(vec![retired.id, active.id, other.id])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE action SET retired_at = NOW() WHERE id = ANY($1::BIGINT[])")
        .bind(vec![retired.id, other.id])
        .execute(&pool)
        .await
        .unwrap();

    let components = ComponentLifecycleRepository::list_retired_by_pack(&pool, pack.id)
        .await
        .unwrap();
    assert_eq!(components.len(), 1);
    assert_eq!(components[0].kind, "action");
    assert_eq!(components[0].id, retired.id);
    assert_eq!(
        components[0].component_ref.as_deref(),
        Some(retired.r#ref.as_str())
    );
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn loader_retires_omitted_versions_and_rejects_a_retired_runtime_dependency() {
    let pool = create_test_pool().await.unwrap();
    PlatformCatalogRepository::reconcile(&pool).await.unwrap();
    let pack = PackFixture::new_unique("dependency-lifecycle")
        .create(&pool)
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("runtimes")).unwrap();
    std::fs::create_dir(root.path().join("actions")).unwrap();
    let runtime_ref = format!("{}.custom", pack.r#ref);
    let runtime_path = root.path().join("runtimes/custom.yaml");
    std::fs::write(
        &runtime_path,
        format!(
            "ref: {runtime_ref}\npack_ref: {}\nname: Custom\nexecution_config: {{}}\nversions:\n  - version: '1.0.0'\n",
            pack.r#ref
        ),
    )
    .unwrap();
    std::fs::write(
        root.path().join("actions/run.yaml"),
        format!(
            "ref: {}.run\nlabel: Run\nrunner_type: {runtime_ref}\nentry_point: run.sh\n",
            pack.r#ref
        ),
    )
    .unwrap();

    let loader = PackComponentLoader::new(
        &pool,
        pack.id,
        &pack.r#ref,
        &CacheAdmissionConfig::default(),
    );
    loader.load_all(root.path()).await.unwrap();
    let runtime_id: i64 = sqlx::query_scalar("SELECT id FROM runtime WHERE ref = $1")
        .bind(&runtime_ref)
        .fetch_one(&pool)
        .await
        .unwrap();
    let version_id: i64 = sqlx::query_scalar("SELECT id FROM runtime_version WHERE runtime = $1")
        .bind(runtime_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    std::fs::write(
        &runtime_path,
        format!(
            "ref: {runtime_ref}\npack_ref: {}\nname: Custom\nexecution_config: {{}}\n",
            pack.r#ref
        ),
    )
    .unwrap();
    loader.load_all(root.path()).await.unwrap();
    let version_retired: bool =
        sqlx::query_scalar("SELECT retired_at IS NOT NULL FROM runtime_version WHERE id = $1")
            .bind(version_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(version_retired);

    std::fs::remove_file(runtime_path).unwrap();
    let error = loader.load_all(root.path()).await.unwrap_err();
    assert!(error.to_string().contains("references retired runtime"));
    let runtime_still_active: bool =
        sqlx::query_scalar("SELECT retired_at IS NULL FROM runtime WHERE id = $1")
            .bind(runtime_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(runtime_still_active);
}
