mod helpers;

use attune_common::{
    auth::jwt::{generate_access_token, JwtConfig},
    repositories::{
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
        },
        pack::{CreatePackInput, PackRepository},
        Create, FindByRef,
    },
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::json;

fn jwt_config() -> JwtConfig {
    JwtConfig {
        secret: "test-secret-for-testing-only-not-secure".to_string(),
        access_token_expiration: 300,
        refresh_token_expiration: 3600,
    }
}

#[tokio::test]
async fn updating_standard_pack_requires_configure_permission() -> Result<()> {
    let ctx = TestContext::new().await?;
    let pack = PackRepository::create(
        &ctx.pool,
        CreatePackInput {
            r#ref: "standard_update_authz".to_string(),
            label: "Original label".to_string(),
            description: None,
            version: "1.0.0".to_string(),
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            tags: vec![],
            runtime_deps: vec![],
            dependencies: vec![],
            is_standard: true,
            installers: json!({}),
        },
    )
    .await?;

    let denied_identity = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "standard_pack_denied".to_string(),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;
    let denied_token =
        generate_access_token(denied_identity.id, &denied_identity.login, &jwt_config())?;

    let denied = ctx
        .put(
            &format!("/api/v1/packs/{}", pack.r#ref),
            json!({ "label": "Denied update" }),
            Some(&denied_token),
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        PackRepository::find_by_ref(&ctx.pool, &pack.r#ref)
            .await?
            .expect("standard pack should exist")
            .label,
        "Original label"
    );

    let allowed_identity = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "standard_pack_allowed".to_string(),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;
    let permission_set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "test.standard_pack_configure".to_string(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{ "resource": "packs", "actions": ["configure"] }]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: allowed_identity.id,
            permset: permission_set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(allowed_identity.id)
        .await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let allowed_token =
        generate_access_token(allowed_identity.id, &allowed_identity.login, &jwt_config())?;

    let allowed = ctx
        .put(
            &format!("/api/v1/packs/{}", pack.r#ref),
            json!({ "label": "Authorized update" }),
            Some(&allowed_token),
        )
        .await?;
    assert_eq!(allowed.status(), StatusCode::OK);
    assert_eq!(
        PackRepository::find_by_ref(&ctx.pool, &pack.r#ref)
            .await?
            .expect("standard pack should exist")
            .label,
        "Authorized update"
    );

    Ok(())
}
