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

async fn grant(ctx: &TestContext, identity: i64, grants: Value) -> String {
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
    set.r#ref
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
    // Exercise a real persisted session across denied profile-editor attempts
    // and an authorized administrative reset, not just the access-token fixture.
    let prior_login = ctx
        .post(
            "/auth/login",
            json!({"login":ctx.user.as_ref().unwrap().login,"password":"TestPassword123!"}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(prior_login.status(), StatusCode::OK);
    let prior_login: Value = prior_login.json().await.unwrap();
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

    // Rejected credential writes and a permitted display-name change must
    // leave the existing session usable.
    let rotated = ctx
        .post(
            "/auth/refresh",
            json!({"refresh_token":prior_login["data"]["refresh_token"]}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(rotated.status(), StatusCode::OK);
    let rotated: Value = rotated.json().await.unwrap();

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
    // The same admin endpoint's hash write must atomically revoke the family.
    assert_eq!(
        ctx.post(
            "/auth/refresh",
            json!({"refresh_token":rotated["data"]["refresh_token"]}),
            None,
        )
        .await
        .unwrap()
        .status(),
        StatusCode::UNAUTHORIZED
    );
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
    assert_eq!(
        ctx.post(
            "/auth/refresh",
            json!({"refresh_token":recovered["data"]["refresh_token"]}),
            None,
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
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
    // Permission administration is necessary but never widens identity scopes.
    grant(
        &ctx,
        caller.id,
        json!([{"resource":"permissions","actions":["manage"]}]),
    )
    .await;
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

#[tokio::test]
async fn identity_manager_cannot_issue_admin_credentials_or_satisfy_rbac_attributes() {
    use attune_common::repositories::{
        identity::{
            CreateIdentityRoleAssignmentInput, CreatePermissionSetRoleAssignmentInput,
            IdentityRoleAssignmentRepository, PermissionSetRoleAssignmentRepository,
        },
        IntegrationTokenRepository,
    };
    let ctx = TestContext::new()
        .await
        .unwrap()
        .with_admin_auth()
        .await
        .unwrap();
    let admin = ctx.user.as_ref().unwrap();
    let manager_token = ctx.create_test_user("delegated_manager").await.unwrap();
    let manager = IdentityRepository::find_by_login(&ctx.pool, "delegated_manager")
        .await
        .unwrap()
        .unwrap();
    let refs = grant(
        &ctx,
        manager.id,
        json!([
            {"resource":"identities","actions":["create","update","manage"]}
        ]),
    )
    .await;
    // A real role grants read authority only after its attribute predicate matches.
    let conditional = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "test.attribute_gate".into(), pack:None, pack_ref:None, label:None, description:None,
        grants:json!([{"resource":"permissions","actions":["read"],"constraints":{"attributes":{"department":"security"}}}]),
    }).await.unwrap();
    IdentityRoleAssignmentRepository::create(
        &ctx.pool,
        CreateIdentityRoleAssignmentInput {
            identity: manager.id,
            role: "conditional_reader".into(),
            source: "local".into(),
            managed: false,
        },
    )
    .await
    .unwrap();
    PermissionSetRoleAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionSetRoleAssignmentInput {
            role: "conditional_reader".into(),
            permset: conditional.id,
        },
    )
    .await
    .unwrap();
    AuthorizationService::invalidate_identity_authz_cache(manager.id).await;
    let admin_path = format!("/api/v1/identities/{}", admin.id);
    let self_path = format!("/api/v1/identities/{}", manager.id);
    let before = IdentityRepository::find_by_id(&ctx.pool, admin.id)
        .await
        .unwrap()
        .unwrap();
    let session = ctx
        .post(
            "/auth/login",
            json!({"login":admin.login,"password":"TestPassword123!"}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(session.status(), StatusCode::OK);
    let session: Value = session.json().await.unwrap();
    let tokens_before = IntegrationTokenRepository::list_by_identity(&ctx.pool, admin.id)
        .await
        .unwrap();
    assert_eq!(
        ctx.get("/api/v1/permissions/sets", Some(&manager_token))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    for (path, body) in [
        (&admin_path, json!({"password":"AttackerPass123!"})),
        (
            &admin_path,
            json!({"display_name":"Partial","password":"AttackerPass123!"}),
        ),
        (&self_path, json!({"attributes":{"department":"security"}})),
        (&self_path, json!({"password":"AttackerPass123!"})),
    ] {
        let response = ctx.put(path, body, Some(&manager_token)).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!response.text().await.unwrap().contains("AttackerPass"));
    }
    let denied = ctx
        .post(
            &format!("{admin_path}/integration-tokens"),
            json!({"label":"takeover"}),
            Some(&manager_token),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let body: Value = denied.json().await.unwrap();
    assert!(body.get("data").is_none_or(Value::is_null));
    let after = IdentityRepository::find_by_id(&ctx.pool, admin.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
    assert_eq!(
        tokens_before.len(),
        IntegrationTokenRepository::list_by_identity(&ctx.pool, admin.id)
            .await
            .unwrap()
            .len()
    );
    assert_eq!(
        IdentityRepository::find_by_id(&ctx.pool, manager.id)
            .await
            .unwrap()
            .unwrap()
            .attributes,
        manager.attributes
    );
    assert_eq!(
        ctx.post(
            "/api/v1/identities",
            json!({"login":"injected_identity","attributes":{"department":"security"}}),
            Some(&manager_token)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    assert!(
        IdentityRepository::find_by_login(&ctx.pool, "injected_identity")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        ctx.put(
            &admin_path,
            json!({"display_name":"Safe profile"}),
            Some(&manager_token)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.post(
            "/auth/refresh",
            json!({"refresh_token":session["data"]["refresh_token"]}),
            None
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    // Attribute omission, null and an empty object do not supply an RBAC
    // predicate or provider binding when creating an ordinary local identity.
    for (login, attrs) in [
        ("plain_omitted", None),
        ("plain_null", Some(Value::Null)),
        ("plain_empty", Some(json!({}))),
    ] {
        let mut body = json!({"login":login,"password":"LocalPass123!"});
        if let Some(attrs) = attrs {
            body["attributes"] = attrs;
        }
        assert_eq!(
            ctx.post("/api/v1/identities", body, Some(&manager_token))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            ctx.post(
                "/auth/login",
                json!({"login":login,"password":"LocalPass123!"}),
                None
            )
            .await
            .unwrap()
            .status(),
            StatusCode::OK
        );
    }
    // An authorized attribute change really does satisfy the role predicate.
    assert_eq!(
        ctx.put(
            &self_path,
            json!({"attributes":{"department":"security"}}),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.get("/api/v1/permissions/sets", Some(&manager_token))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    // Adding permission administration to the owner does not widen its existing
    // execution credential: only the selected identity-management set is admitted.
    grant(
        &ctx,
        manager.id,
        json!([{"resource":"permissions","actions":["manage"]}]),
    )
    .await;
    let narrow = helpers::delegated_source::token(&ctx, manager.id, &[refs])
        .await
        .unwrap();
    assert_eq!(
        ctx.put(
            &admin_path,
            json!({"password":"AttackerPass123!"}),
            Some(&narrow)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ctx.post(
            &format!("{admin_path}/integration-tokens"),
            json!({"label":"narrow"}),
            Some(&narrow)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    let before_narrow = IdentityRepository::find_by_id(&ctx.pool, manager.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ctx.put(
            &self_path,
            json!({"display_name":"Partial execution write","attributes":{"department":"changed"}}),
            Some(&narrow)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        serde_json::to_value(&before_narrow).unwrap(),
        serde_json::to_value(
            IdentityRepository::find_by_id(&ctx.pool, manager.id)
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        ctx.post(
            "/api/v1/identities",
            json!({"login":"execution_injected","attributes":{"department":"security"}}),
            Some(&narrow)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    assert!(
        IdentityRepository::find_by_login(&ctx.pool, "execution_injected")
            .await
            .unwrap()
            .is_none()
    );
    // Human credential with both explicit capabilities can recover the account.
    assert_eq!(
        ctx.put(
            &admin_path,
            json!({"password":"RecoveredPass123!"}),
            Some(&manager_token)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.post(
            "/auth/login",
            json!({"login":admin.login,"password":"RecoveredPass123!"}),
            None
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    let issued = ctx
        .post(
            &format!("{admin_path}/integration-tokens"),
            json!({"label":"authorized-recovery"}),
            Some(&manager_token),
        )
        .await
        .unwrap();
    assert_eq!(issued.status(), StatusCode::CREATED);
    let issued: Value = issued.json().await.unwrap();
    assert_eq!(
        ctx.post(
            "/auth/token-login",
            json!({"token":issued["data"]["token"]}),
            None
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    ctx.cleanup().await.unwrap();
}

#[tokio::test]
async fn external_identity_password_reset_is_rejected_without_partial_writes() {
    let ctx = TestContext::new()
        .await
        .unwrap()
        .with_admin_auth()
        .await
        .unwrap();
    ctx.create_test_user("external_identity").await.unwrap();
    let target = IdentityRepository::find_by_login(&ctx.pool, "external_identity")
        .await
        .unwrap()
        .unwrap();
    let path = format!("/api/v1/identities/{}", target.id);
    for provider in ["oidc", "ldap"] {
        // Neither creation nor adding a provider marker to a local identity may
        // create a local password at the same time, even for an administrator.
        assert_eq!(ctx.post("/api/v1/identities", json!({"login":format!("new_{provider}"),"password":"DeniedPass123!","attributes":{provider:{"sub":"external"}}}), ctx.token()).await.unwrap().status(), StatusCode::BAD_REQUEST);
        assert!(
            IdentityRepository::find_by_login(&ctx.pool, &format!("new_{provider}"))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            ctx.put(&path, json!({"attributes":{}}), ctx.token())
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let local = IdentityRepository::find_by_id(&ctx.pool, target.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ctx.put(&path, json!({"password":"DeniedPass123!","display_name":"partial","attributes":{provider:{"sub":"external"}}}), ctx.token()).await.unwrap().status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            serde_json::to_value(&local).unwrap(),
            serde_json::to_value(
                IdentityRepository::find_by_id(&ctx.pool, target.id)
                    .await
                    .unwrap()
                    .unwrap()
            )
            .unwrap()
        );

        assert_eq!(
            ctx.put(
                &path,
                json!({"attributes":{provider:{"sub":"external"}}}),
                ctx.token()
            )
            .await
            .unwrap()
            .status(),
            StatusCode::OK
        );
        let before = IdentityRepository::find_by_id(&ctx.pool, target.id)
            .await
            .unwrap()
            .unwrap();
        for body in [
            json!({"password":"DeniedPass123!","display_name":"partial"}),
            json!({"password":"DeniedPass123!","attributes":{}}),
        ] {
            assert_eq!(
                ctx.put(&path, body, ctx.token()).await.unwrap().status(),
                StatusCode::BAD_REQUEST
            );
            assert_eq!(
                serde_json::to_value(&before).unwrap(),
                serde_json::to_value(
                    IdentityRepository::find_by_id(&ctx.pool, target.id)
                        .await
                        .unwrap()
                        .unwrap()
                )
                .unwrap()
            );
        }
        assert_eq!(
            ctx.put(
                &path,
                json!({"display_name":"External profile"}),
                ctx.token()
            )
            .await
            .unwrap()
            .status(),
            StatusCode::OK
        );
    }
    ctx.cleanup().await.unwrap();
}
