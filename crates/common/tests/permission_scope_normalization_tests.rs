mod helpers;

use attune_common::{
    rbac::{Action, AuthorizationContext, Grant, Resource},
    repositories::{
        identity::{
            CreateIdentityRoleAssignmentInput, CreatePermissionAssignmentInput,
            CreatePermissionSetInput, CreatePermissionSetRoleAssignmentInput, IdentityRepository,
            IdentityRoleAssignmentRepository, PermissionAssignmentRepository,
            PermissionSetRepository, PermissionSetRoleAssignmentRepository,
        },
        Create, FindByRef,
    },
    runtime_authority::normalize_iam_grants,
};
use helpers::{create_test_pool, IdentityFixture, PackFixture};
use serde_json::json;

#[tokio::test]
async fn direct_role_and_repository_readers_share_structural_normalization_and_preserve_authoring()
{
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("local_policy")
        .create(&db)
        .await
        .unwrap();
    let direct = IdentityFixture::new_unique("direct_policy")
        .create(&db)
        .await
        .unwrap();
    let inherited = IdentityFixture::new_unique("role_policy")
        .create(&db)
        .await
        .unwrap();
    let authored = json!([
        {"resource":"actions","actions":["execute"]},
        {"resource":"events","actions":["create"],"scope":"external","constraints":{"refs":["shared.completed"]}}
    ]);
    let expected = normalize_iam_grants(authored.clone(), Some(pack.id)).unwrap();
    let set = PermissionSetRepository::create(
        &db,
        CreatePermissionSetInput {
            r#ref: format!("{}.runtime", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: None,
            description: None,
            grants: authored.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(set.grants, expected);
    PermissionAssignmentRepository::create(
        &db,
        CreatePermissionAssignmentInput {
            identity: direct.id,
            permset: set.id,
        },
    )
    .await
    .unwrap();
    IdentityRoleAssignmentRepository::create(
        &db,
        CreateIdentityRoleAssignmentInput {
            identity: inherited.id,
            role: "local-operators".into(),
            source: "local".into(),
            managed: false,
        },
    )
    .await
    .unwrap();
    PermissionSetRoleAssignmentRepository::create(
        &db,
        CreatePermissionSetRoleAssignmentInput {
            permset: set.id,
            role: "local-operators".into(),
        },
    )
    .await
    .unwrap();
    let direct_state = IdentityRepository::authorization_state(&db, direct.id)
        .await
        .unwrap()
        .unwrap();
    let inherited_state = IdentityRepository::authorization_state(&db, inherited.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&direct_state.grants.0).unwrap(),
        expected
    );
    assert_eq!(direct_state.grants.0, inherited_state.grants.0);
    let owned_action = helpers::ActionFixture::new(pack.id, &pack.r#ref, "owned")
        .create(&db)
        .await
        .unwrap();
    let owners =
        attune_common::repositories::iam_ownership::IamOwnershipRepository::component_packs(
            &*db,
            attune_common::repositories::runtime_target::ComponentTarget::Action,
            &[owned_action.id],
        )
        .await
        .unwrap();
    for (identity, grants) in [
        (direct.id, &direct_state.grants.0),
        (inherited.id, &inherited_state.grants.0),
    ] {
        let mut context = AuthorizationContext::new(identity);
        context.target_id = Some(owned_action.id);
        context.target_ref = Some(owned_action.r#ref.clone());
        context.managing_pack_id = owners.get(&owned_action.id).copied();
        assert!(grants.iter().any(|grant| grant.allows(
            Resource::Actions,
            Action::Execute,
            &context
        )));
        // A structural local grant is never a global-administration grant.
        assert!(!grants.iter().any(|grant| grant.allows(
            Resource::Workers,
            Action::Manage,
            &context
        )));
    }
    let read = PermissionSetRepository::find_by_ref(&db, &set.r#ref)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.grants, expected);
    let mut context = AuthorizationContext::new(direct.id);
    context.pack_ref = Some(pack.r#ref.clone());
    context.target_ref = Some(format!("{}.owned", pack.r#ref));
    let grants: Vec<Grant> = serde_json::from_value(read.grants).unwrap();
    assert!(!grants[0].allows(Resource::Actions, Action::Execute, &context));
    context.managing_pack_id = Some(pack.id);
    assert!(grants[0].allows(Resource::Actions, Action::Execute, &context));
    context.managing_pack_id = Some(pack.id + 1000);
    assert!(!grants[0].allows(Resource::Actions, Action::Execute, &context));
    let mut tx = db.begin().await.unwrap();
    let definition = PermissionSetRepository::authored_definition(&mut tx, &set.r#ref)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_value(&definition.grants).unwrap(), authored);
    let admin = PermissionSetRepository::list_admin(&db, None, Some(&set.r#ref), false)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(admin.authored_grants, authored);
    assert_eq!(admin.permission_set.grants, expected);
    tx.rollback().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let mut canonical = definition;
    canonical.scope = attune_common::runtime_authority::PermissionScope::External;
    let projected = canonical.authored_grants().unwrap();
    sqlx::query("UPDATE permission_set SET grants = $1 WHERE id = $2")
        .bind(&projected)
        .bind(set.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    PermissionSetRepository::store_authored_definition(&mut tx, set.id, &canonical)
        .await
        .unwrap();
    let retained = PermissionSetRepository::authored_definition(&mut tx, &set.r#ref)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.scope, canonical.scope);
    assert_eq!(retained.authored_grants().unwrap(), projected);
    assert_eq!(retained.grants[0].scope, None);
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn unmanaged_platform_grants_keep_their_scope_and_cannot_inject_an_ownership_constraint() {
    let db = create_test_pool().await.unwrap();
    let raw = json!([{"resource":"workers","actions":["manage"]}]);
    let set = PermissionSetRepository::create(
        &db,
        CreatePermissionSetInput {
            r#ref: "platform.worker_admin".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: raw.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(set.grants, raw);
    let context = AuthorizationContext::new(1);
    let grants: Vec<Grant> = serde_json::from_value(set.grants).unwrap();
    assert!(grants[0].allows(Resource::Workers, Action::Manage, &context));
    db.cleanup().await.unwrap();
}
