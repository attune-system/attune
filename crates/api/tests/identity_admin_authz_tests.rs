use axum::http::StatusCode;
use helpers::*;
use serde_json::{json, Value};

use attune_api::authz::AuthorizationService;
use attune_common::repositories::{
    identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    },
    Create, FindById, IdentityRepository,
};

mod helpers;

async fn grant(ctx: &TestContext, identity: i64, grants: Value) {
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("test.g{}", uuid::Uuid::new_v4().simple()),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants,
        },
    )
    .await
    .unwrap();
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await
    .unwrap();
    AuthorizationService::invalidate_identity_authz_cache(identity).await;
}

#[tokio::test]
async fn profile_editor_cannot_take_over_admin_or_change_authorization() {
    let ctx = TestContext::new()
        .await
        .unwrap()
        .with_admin_auth()
        .await
        .unwrap();
    let target = ctx.user.as_ref().unwrap().id;
    let editor_token = ctx.create_test_user("profile_editor").await.unwrap();
    let editor = IdentityRepository::find_by_login(&ctx.pool, "profile_editor")
        .await
        .unwrap()
        .unwrap();
    grant(
        &ctx,
        editor.id,
        json!([{"resource":"identities","actions":["update"]}]),
    )
    .await;
    let path = format!("/api/v1/identities/{target}");
    let before = IdentityRepository::find_by_id(&ctx.pool, target)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ctx.put(
            &path,
            json!({"display_name":"Profile"}),
            Some(&editor_token)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    for body in [
        json!({"password":"AttackerPass123!"}),
        json!({"attributes":{"department":"admin","oidc":{"sub":"attacker"}}}),
        json!({"frozen":true}),
        json!({"display_name":"Partial write","password":"AttackerPass123!"}),
    ] {
        assert_eq!(
            ctx.put(&path, body, Some(&editor_token))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    for suffix in ["freeze", "unfreeze", "integration-tokens"] {
        assert_eq!(
            ctx.post(
                &format!("{path}/{suffix}"),
                json!({"label":"takeover"}),
                Some(&editor_token)
            )
            .await
            .unwrap()
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    // Self is no implicit credential-administration exemption.
    assert_eq!(
        ctx.put(
            &format!("/api/v1/identities/{}", editor.id),
            json!({"password":"AttackerPass123!"}),
            Some(&editor_token)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    let after = IdentityRepository::find_by_id(&ctx.pool, target)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.password_hash, before.password_hash);
    assert_eq!(after.attributes, before.attributes);
    assert_eq!(after.frozen, before.frozen);
    assert_eq!(after.display_name.as_deref(), Some("Profile"));

    // Explicit admin authority still supports recovery and token lifecycle.
    assert_eq!(
        ctx.put(
            &path,
            json!({"password":"RecoveredPass123!","attributes":{"department":"ops"}}),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    let after = IdentityRepository::find_by_id(&ctx.pool, target)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(after.password_hash, before.password_hash);
    let recovered = ctx
        .post(
            "/auth/login",
            json!({"login":after.login,"password":"RecoveredPass123!"}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(recovered.status(), StatusCode::OK);
    let recovered: Value = recovered.json().await.unwrap();
    let recovered_token = recovered["data"]["access_token"].as_str().unwrap();
    let response = ctx
        .post(
            &format!("{path}/integration-tokens"),
            json!({"label":"recovery"}),
            Some(recovered_token),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response.json().await.unwrap();
    let token_id = body["data"]["integration_token"]["id"].as_i64().unwrap();
    assert_eq!(
        ctx.post(
            "/auth/token-login",
            json!({"token":body["data"]["token"]}),
            None
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    let token_path = format!("{path}/integration-tokens/{token_id}");
    assert_eq!(
        ctx.post(
            &format!("{token_path}/revoke"),
            json!({}),
            Some(&editor_token)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ctx.delete(&token_path, Some(&editor_token))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ctx.post(
            &format!("{token_path}/revoke"),
            json!({}),
            Some(recovered_token)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.delete(&token_path, Some(recovered_token))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    ctx.cleanup().await.unwrap();
}

#[tokio::test]
async fn identity_scopes_use_actual_target_for_profile_and_administration() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();
    let caller = ctx.user.as_ref().unwrap();
    ctx.create_test_user("other_identity").await.unwrap();
    let other = IdentityRepository::find_by_login(&ctx.pool, "other_identity")
        .await
        .unwrap()
        .unwrap();
    grant(&ctx, caller.id, json!([
        {"resource":"identities","actions":["read","update","manage"],"constraints":{"owner":"self"}}
    ])).await;
    let own_path = format!("/api/v1/identities/{}", caller.id);
    let other_path = format!("/api/v1/identities/{}", other.id);
    assert_eq!(
        ctx.put(&own_path, json!({"display_name":"Self"}), ctx.token())
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.get(&own_path, ctx.token()).await.unwrap().status(),
        StatusCode::OK
    );
    for body in [
        json!({"display_name":"Other"}),
        json!({"password":"OtherPassword123!"}),
    ] {
        assert_eq!(
            ctx.put(&other_path, body, ctx.token())
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        ctx.post(
            &format!("{other_path}/integration-tokens"),
            json!({"label":"other"}),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    // Explicit login scopes grant other-target recovery, not global access.
    grant(
        &ctx,
        caller.id,
        json!([
            {"resource":"identities","actions":["manage"],"constraints":{"refs":[other.login]}}
        ]),
    )
    .await;
    assert_eq!(
        ctx.put(
            &other_path,
            json!({"password":"OtherPassword123!"}),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.post(&format!("{other_path}/freeze"), json!({}), ctx.token())
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.post(&format!("{other_path}/unfreeze"), json!({}), ctx.token())
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.put(
            &other_path,
            json!({"display_name":"Still denied"}),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ctx.get(&other_path, ctx.token()).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ctx.get("/api/v1/identities/-1", ctx.token())
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ctx.put(
            "/api/v1/identities/-1",
            json!({"display_name":"Absent"}),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ctx.put(
            &own_path,
            json!({"password":"SelfPassword123!"}),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    ctx.cleanup().await.unwrap();
}

#[tokio::test]
async fn identity_administration_scopes_are_authorable_via_api() {
    let ctx = TestContext::new()
        .await
        .unwrap()
        .with_admin_auth()
        .await
        .unwrap();
    for (index, constraints) in [
        json!({"owner":"self"}),
        json!({"refs":[ctx.user.as_ref().unwrap().login]}),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(ctx.post("/api/v1/permissions/sets",
            json!({"ref":format!("test.identity{index}"),
                "grants":[{"resource":"identities","actions":["read","update","manage"],"constraints":constraints}]}),
            ctx.token()).await.unwrap().status(), StatusCode::CREATED);
    }
    assert_eq!(
        ctx.get("/api/v1/identities/-1", ctx.token())
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        ctx.put(
            "/api/v1/identities/-1",
            json!({"password":"AbsentPassword123!"}),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::NOT_FOUND
    );
    ctx.cleanup().await.unwrap();
}
#[tokio::test]
async fn installed_revision_three_admin_receives_explicit_administration() {
    use attune_common::{
        models::ManagementOrigin,
        platform_catalog::{ManagedComponentKind, CATALOG_REVISION, COMPATIBILITY_EPOCH},
        repositories::{
            identity::{
                CreateIdentityRoleAssignmentInput, CreatePermissionSetRoleAssignmentInput,
                IdentityRoleAssignmentRepository, PermissionSetRoleAssignmentRepository,
            },
            platform_catalog::PlatformCatalogRepository as Catalog,
            FindByRef,
        },
    };

    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();
    let actor = ctx.user.as_ref().unwrap().id;
    ctx.create_test_user("catalog_recovery_admin")
        .await
        .unwrap();
    let target = IdentityRepository::find_by_login(&ctx.pool, "catalog_recovery_admin")
        .await
        .unwrap()
        .unwrap();
    let profile_token = ctx
        .create_test_user("catalog_profile_editor")
        .await
        .unwrap();
    let profile = IdentityRepository::find_by_login(&ctx.pool, "catalog_profile_editor")
        .await
        .unwrap()
        .unwrap();

    // Frozen catalog bytes from be05d042, not current YAML with an old counter.
    // Fixture SQL only: seed an installed platform row without violating the
    // production prohibition on downgrading existing platform-owned rows.
    let old: Value =
        serde_yaml_ng::from_str(include_str!("fixtures/catalog-revision-3-admin.yaml")).unwrap();
    let old_identity = old["grants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["resource"] == "identities")
        .unwrap();
    assert!(!old_identity["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a == "manage"));
    let mut tx = ctx.pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('attune.catalog_write', 'on', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO permission_set (ref, label, description, grants, catalog_revision) VALUES ($1->>'ref', $1->>'label', $1->>'description', $1->'grants', 3)")
        .bind(&old).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE platform_catalog_state SET revision = 3, bootstrap_definitions = $1 WHERE singleton")
        .bind(json!({"permission_set":{"core.admin":old}})).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    let admin = PermissionSetRepository::find_by_ref(&ctx.pool, "core.admin")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(admin.grants, old["grants"]);
    assert_eq!(
        Catalog::origin(&ctx.pool, ManagedComponentKind::PermissionSet, "core.admin")
            .await
            .unwrap(),
        Some(ManagementOrigin::Platform {
            catalog_revision: 3
        })
    );
    // Recovery actor gets admin only through its role; target admin has a
    // direct assignment. Reconciliation must preserve both forms of linkage.
    let direct = PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: target.id,
            permset: admin.id,
        },
    )
    .await
    .unwrap();
    let membership = IdentityRoleAssignmentRepository::create(
        &ctx.pool,
        CreateIdentityRoleAssignmentInput {
            identity: actor,
            role: "security_admin".into(),
            source: "manual".into(),
            managed: false,
        },
    )
    .await
    .unwrap();
    let role = PermissionSetRoleAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionSetRoleAssignmentInput {
            permset: admin.id,
            role: "security_admin".into(),
        },
    )
    .await
    .unwrap();
    let links_before = json!([&direct, &membership, &role]);
    grant(
        &ctx,
        profile.id,
        json!([{"resource":"identities","actions":["update"]}]),
    )
    .await;
    AuthorizationService::invalidate_identity_authz_cache(actor).await;

    let path = format!("/api/v1/identities/{}", target.id);
    assert_eq!(
        ctx.put(
            &path,
            json!({"password":"CatalogRecovery123!"}),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ctx.put(
            &path,
            json!({"password":"CatalogRecovery123!"}),
            Some(&profile_token)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );

    Catalog::reconcile(&ctx.pool).await.unwrap();
    // Startup reconciliation precedes request authorization. Discard snapshots
    // from the pre-upgrade probes to model the restarted API's empty caches.
    AuthorizationService::invalidate_identity_authz_cache(actor).await;
    AuthorizationService::invalidate_permission_set_caches().await;
    let upgraded = PermissionSetRepository::find_by_ref(&ctx.pool, "core.admin")
        .await
        .unwrap()
        .unwrap();
    let expected: Value = serde_yaml_ng::from_str(include_str!(
        "../../common/src/platform_catalog/permission_sets/admin.yaml"
    ))
    .unwrap();
    assert_eq!(upgraded.id, admin.id);
    assert_eq!(
        Catalog::origin(&ctx.pool, ManagedComponentKind::PermissionSet, "core.admin")
            .await
            .unwrap(),
        Some(ManagementOrigin::Platform {
            catalog_revision: CATALOG_REVISION
        })
    );
    assert_eq!(upgraded.grants, expected["grants"]);
    assert_ne!(upgraded.grants, admin.grants);
    assert_eq!(
        json!([
            PermissionAssignmentRepository::find_by_id(&ctx.pool, direct.id)
                .await
                .unwrap()
                .unwrap(),
            IdentityRoleAssignmentRepository::find_by_id(&ctx.pool, membership.id)
                .await
                .unwrap()
                .unwrap(),
            PermissionSetRoleAssignmentRepository::find_by_id(&ctx.pool, role.id)
                .await
                .unwrap()
                .unwrap()
        ]),
        links_before
    );
    let state: (i32, i32, Value) = sqlx::query_as("SELECT compatibility_epoch, revision, bootstrap_definitions FROM platform_catalog_state WHERE singleton")
        .fetch_one(&ctx.pool).await.unwrap();
    assert_eq!((state.0, state.1), (COMPATIBILITY_EPOCH, CATALOG_REVISION));
    assert_eq!(
        state.2["permission_set"]["core.admin"]["grants"],
        upgraded.grants
    );

    // Current-revision reconciliation preserves content, timestamps and snapshot.
    Catalog::reconcile(&ctx.pool).await.unwrap();
    let unchanged = PermissionSetRepository::find_by_ref(&ctx.pool, "core.admin")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&unchanged).unwrap(),
        serde_json::to_value(&upgraded).unwrap()
    );
    let snapshot: Value = sqlx::query_scalar(
        "SELECT bootstrap_definitions FROM platform_catalog_state WHERE singleton",
    )
    .fetch_one(&ctx.pool)
    .await
    .unwrap();
    assert_eq!(snapshot, state.2);

    assert_eq!(
        ctx.put(
            &path,
            json!({"password":"CatalogRecovery123!"}),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    let recovered = ctx
        .post(
            "/auth/login",
            json!({"login":target.login,"password":"CatalogRecovery123!"}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(recovered.status(), StatusCode::OK);
    assert_eq!(
        ctx.post(
            &format!("{path}/integration-tokens"),
            json!({"label":"reconciled-recovery"}),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::CREATED
    );
    for body in [
        json!({"password":"DeniedCatalog123!"}),
        json!({"attributes":{"department":"admin"}}),
        json!({"frozen":true}),
    ] {
        assert_eq!(
            ctx.put(&path, body, Some(&profile_token))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        ctx.put(
            &path,
            json!({"display_name":"Profile after upgrade"}),
            Some(&profile_token)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    ctx.cleanup().await.unwrap();
}
