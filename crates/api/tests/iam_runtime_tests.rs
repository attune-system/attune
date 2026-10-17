mod helpers;

use attune_common::{
    auth::jwt::generate_access_token,
    repositories::{
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
        },
        Create,
    },
};
use axum::http::StatusCode;
use helpers::{create_test_pack, Result, TestContext};
use serde_json::{json, Value};

#[tokio::test]
async fn local_runtime_iam_authorizes_persisted_owners_and_filters_before_pagination() -> Result<()>
{
    let ctx = TestContext::new().await?;
    let local = create_test_pack(&ctx.pool, "runtime_local_iam").await?;
    let foreign = create_test_pack(&ctx.pool, "runtime_foreign_iam").await?;
    let identity = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "runtime-local-user".into(),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await?;
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("{}.runtimes", local.r#ref),
            pack: Some(local.id),
            pack_ref: Some(local.r#ref.clone()),
            label: None,
            description: None,
            grants: json!([{"resource":"runtimes","actions":["read","create","update","delete"]}]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: set.id,
        },
    )
    .await?;
    let token = generate_access_token(identity.id, &identity.login, &ctx.state.jwt_config)?;
    let reference = format!("{}.native", local.r#ref);
    let created=ctx.post("/api/v1/runtimes",json!({"ref":reference,"pack_ref":local.r#ref,"name":"Native","distributions":{},"execution_config":{}}),Some(&token)).await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(ctx.post("/api/v1/runtimes",json!({"ref":format!("{}.native",foreign.r#ref),"pack_ref":foreign.r#ref,"name":"Foreign","distributions":{},"execution_config":{}}),Some(&token)).await?.status(),StatusCode::FORBIDDEN);
    let list: Value = ctx
        .get("/api/v1/runtimes?page_size=1", Some(&token))
        .await?
        .json()
        .await?;
    assert_eq!(list["pagination"]["total_items"], 1);
    assert_eq!(list["items"][0]["ref"], reference);
    assert_eq!(
        ctx.get(&format!("/api/v1/runtimes/{reference}"), Some(&token))
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.put(
            &format!("/api/v1/runtimes/{reference}"),
            json!({"name":"Updated"}),
            Some(&token)
        )
        .await?
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.delete(&format!("/api/v1/runtimes/{reference}"), Some(&token))
            .await?
            .status(),
        StatusCode::OK
    );
    ctx.cleanup().await?;
    Ok(())
}
