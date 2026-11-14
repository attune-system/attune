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
            "/api/v1/auth/login",
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
            "/api/v1/auth/token-login",
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
