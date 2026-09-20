use axum::http::StatusCode;
use serde_json::json;

use attune_api::authz::AuthorizationService;
use attune_common::repositories::{
    identity::{
        CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
        IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
    },
    Create,
};

use helpers::*;

mod helpers;

async fn create_identity(ctx: &TestContext, prefix: &str) -> i64 {
    IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: format!("{prefix}_{}", uuid::Uuid::new_v4().simple()),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await
    .expect("create identity")
    .id
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn admin_can_manage_external_identity_mappings() {
    let ctx = TestContext::new()
        .await
        .expect("create test context")
        .with_admin_auth()
        .await
        .expect("authenticate admin");
    let integration_identity = create_identity(&ctx, "mapping_integration").await;
    let other_integration_identity = create_identity(&ctx, "mapping_other_integration").await;
    let mapped_identity = create_identity(&ctx, "mapping_target").await;
    let replacement_identity = create_identity(&ctx, "mapping_replacement").await;

    let path = format!("/api/v1/identities/{integration_identity}/external-identity-mappings");
    let request = json!({
        "mapped_identity": mapped_identity,
        "provider": " GitHub ",
        "tenant": " Acme ",
        "external_subject": " User-42 "
    });
    let created = ctx
        .post(&path, request.clone(), ctx.token())
        .await
        .expect("create mapping");
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_body: serde_json::Value = created.json().await.expect("created mapping response");
    let mapping = &created_body["data"];
    let mapping_id = mapping["id"].as_i64().expect("mapping id");
    assert_eq!(mapping["integration_identity"], integration_identity);
    assert_eq!(mapping["provider"], "github");
    assert_eq!(mapping["tenant"], "Acme");
    assert_eq!(mapping["external_subject"], "User-42");
    assert!(mapping["created_by"].as_i64().is_some_and(|id| id > 0));

    let duplicate = ctx
        .post(&path, request, ctx.token())
        .await
        .expect("create duplicate mapping");
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    let wrong_parent = ctx
        .get(
            &format!(
                "/api/v1/identities/{other_integration_identity}/external-identity-mappings/{mapping_id}"
            ),
            ctx.token(),
        )
        .await
        .expect("get mapping under wrong parent");
    assert_eq!(wrong_parent.status(), StatusCode::NOT_FOUND);

    let item_path = format!("{path}/{mapping_id}");
    let updated = ctx
        .put(
            &item_path,
            json!({
                "mapped_identity": replacement_identity,
                "provider": "OIDC",
                "tenant": "Tenant-A",
                "external_subject": "Subject-A"
            }),
            ctx.token(),
        )
        .await
        .expect("update mapping");
    assert_eq!(updated.status(), StatusCode::OK);
    let updated_body: serde_json::Value = updated.json().await.expect("updated mapping response");
    assert_eq!(
        updated_body["data"]["mapped_identity"],
        replacement_identity
    );
    assert_eq!(updated_body["data"]["provider"], "oidc");

    let listed = ctx.get(&path, ctx.token()).await.expect("list mappings");
    assert_eq!(listed.status(), StatusCode::OK);
    let listed_body: serde_json::Value = listed.json().await.expect("mapping list response");
    assert_eq!(
        listed_body["items"].as_array().expect("mapping list").len(),
        1
    );

    let deleted = ctx
        .delete(&item_path, ctx.token())
        .await
        .expect("delete mapping");
    assert_eq!(deleted.status(), StatusCode::OK);
    let missing = ctx
        .get(&item_path, ctx.token())
        .await
        .expect("get deleted mapping");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn scoped_admin_cannot_map_an_identity_outside_their_scope() {
    let ctx = TestContext::new()
        .await
        .expect("create test context")
        .with_auth()
        .await
        .expect("authenticate user");
    let caller = ctx.user.as_ref().expect("caller identity");
    let integration_identity = create_identity(&ctx, "scoped_integration").await;
    let mapped_identity = create_identity(&ctx, "out_of_scope_target").await;
    let permission_set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("test.mapping_scope_{}", uuid::Uuid::new_v4().simple()),
            pack: None,
            pack_ref: None,
            label: Some("Scoped mapping administration".to_string()),
            description: None,
            grants: json!([{
                "resource": "identities",
                "actions": ["update"],
                "constraints": {"ids": [integration_identity]}
            }]),
        },
    )
    .await
    .expect("create scoped permission set");
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: caller.id,
            permset: permission_set.id,
        },
    )
    .await
    .expect("assign scoped permission");
    AuthorizationService::invalidate_identity_authz_cache(caller.id).await;
    AuthorizationService::invalidate_permission_set_caches().await;

    let response = ctx
        .post(
            &format!("/api/v1/identities/{integration_identity}/external-identity-mappings"),
            json!({
                "mapped_identity": mapped_identity,
                "provider": "slack",
                "tenant": "team-1",
                "external_subject": "user-1"
            }),
            ctx.token(),
        )
        .await
        .expect("create mapping request");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn mapping_api_rejects_non_admins_and_credential_fields() {
    let ctx = TestContext::new()
        .await
        .expect("create test context")
        .with_auth()
        .await
        .expect("authenticate user");
    let integration_identity = ctx.user.as_ref().expect("user identity").id;
    let path = format!("/api/v1/identities/{integration_identity}/external-identity-mappings");

    let forbidden = ctx
        .get(&path, ctx.token())
        .await
        .expect("list mappings without admin grant");
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let admin_ctx = TestContext::new()
        .await
        .expect("create admin test context")
        .with_admin_auth()
        .await
        .expect("authenticate admin");
    let admin_integration_identity = create_identity(&admin_ctx, "mapping_integration").await;
    let admin_mapped_identity = create_identity(&admin_ctx, "mapping_target").await;
    let rejected = admin_ctx
        .post(
            &format!("/api/v1/identities/{admin_integration_identity}/external-identity-mappings"),
            json!({
                "mapped_identity": admin_mapped_identity,
                "provider": "github",
                "tenant": "acme",
                "external_subject": "user-42",
                "token": "must-not-be-accepted"
            }),
            admin_ctx.token(),
        )
        .await
        .expect("reject credential field");
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
