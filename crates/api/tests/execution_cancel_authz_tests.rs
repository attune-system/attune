use attune_common::{
    auth::jwt::{
        generate_access_token, generate_execution_token_with_permission_sets,
        generate_worker_token, JwtConfig,
    },
    models::{enums::ExecutionStatus, Execution, Identity, PermissionSet},
    repositories::{
        execution::{CreateExecutionInput, ExecutionRepository},
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
        },
        Create, FindById,
    },
};
use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;

mod helpers;
use helpers::TestContext;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

fn jwt_config() -> JwtConfig {
    JwtConfig {
        secret: "test-secret-for-testing-only-not-secure".to_string(),
        access_token_expiration: 300,
        refresh_token_expiration: 3600,
    }
}

async fn create_identity(pool: &PgPool, label: &str) -> TestResult<Identity> {
    Ok(IdentityRepository::create(
        pool,
        CreateIdentityInput {
            login: format!("cancel_{label}_{}", uuid::Uuid::new_v4().simple()),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?)
}

async fn create_permission_set(
    pool: &PgPool,
    label: &str,
    actions: &[&str],
) -> TestResult<PermissionSet> {
    Ok(PermissionSetRepository::create(
        pool,
        CreatePermissionSetInput {
            r#ref: format!("test.cancel_{label}_{}", uuid::Uuid::new_v4().simple()),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{
                "resource": "executions",
                "actions": actions,
                "constraints": {"owner": "self"}
            }]),
        },
    )
    .await?)
}

async fn assign_permission_set(
    pool: &PgPool,
    identity: &Identity,
    permission_set: &PermissionSet,
) -> TestResult<()> {
    PermissionAssignmentRepository::create(
        pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: permission_set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    Ok(())
}

async fn create_requested_execution(pool: &PgPool, executor: i64) -> TestResult<Execution> {
    Ok(ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action: None,
            action_ref: "test.cancel_target".to_string(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: Some(executor),
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await?)
}

async fn execution_status(pool: &PgPool, id: i64) -> TestResult<ExecutionStatus> {
    Ok(ExecutionRepository::find_by_id(pool, id)
        .await?
        .expect("execution should exist")
        .status)
}

#[tokio::test]
async fn worker_token_cannot_cancel_execution() -> TestResult<()> {
    let ctx = TestContext::new().await?;
    let executor = create_identity(&ctx.pool, "worker_target").await?;
    let execution = create_requested_execution(&ctx.pool, executor.id).await?;
    let token = generate_worker_token(executor.id, "test-worker", &jwt_config(), Some(300))?;

    let denied = ctx
        .post(
            &format!("/api/v1/executions/{}/cancel", execution.id),
            json!({}),
            Some(&token),
        )
        .await?;

    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        execution_status(&ctx.pool, execution.id).await?,
        ExecutionStatus::Requested
    );

    Ok(())
}

#[tokio::test]
async fn access_token_requires_cancel_permission_and_respects_owner_scope() -> TestResult<()> {
    let ctx = TestContext::new().await?;
    let caller = create_identity(&ctx.pool, "access_caller").await?;
    let other = create_identity(&ctx.pool, "access_other").await?;
    let read = create_permission_set(&ctx.pool, "read", &["read"]).await?;
    assign_permission_set(&ctx.pool, &caller, &read).await?;
    let token = generate_access_token(caller.id, &caller.login, &jwt_config())?;
    let owned = create_requested_execution(&ctx.pool, caller.id).await?;

    let denied = ctx
        .post(
            &format!("/api/v1/executions/{}/cancel", owned.id),
            json!({}),
            Some(&token),
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        execution_status(&ctx.pool, owned.id).await?,
        ExecutionStatus::Requested
    );

    let cancel = create_permission_set(&ctx.pool, "access_cancel", &["cancel"]).await?;
    assign_permission_set(&ctx.pool, &caller, &cancel).await?;
    let foreign = create_requested_execution(&ctx.pool, other.id).await?;

    let foreign_denied = ctx
        .post(
            &format!("/api/v1/executions/{}/cancel", foreign.id),
            json!({}),
            Some(&token),
        )
        .await?;
    assert_eq!(foreign_denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        execution_status(&ctx.pool, foreign.id).await?,
        ExecutionStatus::Requested
    );

    let allowed = ctx
        .post(
            &format!("/api/v1/executions/{}/cancel", owned.id),
            json!({}),
            Some(&token),
        )
        .await?;
    assert_eq!(allowed.status(), StatusCode::OK);
    assert_eq!(
        execution_status(&ctx.pool, owned.id).await?,
        ExecutionStatus::Cancelled
    );

    Ok(())
}

#[tokio::test]
async fn execution_token_requires_embedded_cancel_permission_and_respects_owner_scope(
) -> TestResult<()> {
    let ctx = TestContext::new().await?;
    let caller = create_identity(&ctx.pool, "execution_caller").await?;
    let other = create_identity(&ctx.pool, "execution_other").await?;
    let owned = create_requested_execution(&ctx.pool, caller.id).await?;
    let foreign = create_requested_execution(&ctx.pool, other.id).await?;
    let cancel = create_permission_set(&ctx.pool, "execution_cancel", &["cancel"]).await?;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;

    let denied_token = generate_execution_token_with_permission_sets(
        caller.id,
        owned.id,
        "test.cancel_caller",
        &jwt_config(),
        Some(300),
        &[],
    )?;
    let denied = ctx
        .post(
            &format!("/api/v1/executions/{}/cancel", owned.id),
            json!({}),
            Some(&denied_token),
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        execution_status(&ctx.pool, owned.id).await?,
        ExecutionStatus::Requested
    );

    let permission_refs = vec![cancel.r#ref.clone()];
    let allowed_token = generate_execution_token_with_permission_sets(
        caller.id,
        owned.id,
        "test.cancel_caller",
        &jwt_config(),
        Some(300),
        &permission_refs,
    )?;
    let undelegable = ctx
        .post(
            &format!("/api/v1/executions/{}/cancel", owned.id),
            json!({}),
            Some(&allowed_token),
        )
        .await?;
    assert_eq!(undelegable.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        execution_status(&ctx.pool, owned.id).await?,
        ExecutionStatus::Requested
    );
    assign_permission_set(&ctx.pool, &caller, &cancel).await?;
    let foreign_denied = ctx
        .post(
            &format!("/api/v1/executions/{}/cancel", foreign.id),
            json!({}),
            Some(&allowed_token),
        )
        .await?;
    assert_eq!(foreign_denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        execution_status(&ctx.pool, foreign.id).await?,
        ExecutionStatus::Requested
    );

    let allowed = ctx
        .post(
            &format!("/api/v1/executions/{}/cancel", owned.id),
            json!({}),
            Some(&allowed_token),
        )
        .await?;
    assert_eq!(allowed.status(), StatusCode::OK);
    assert_eq!(
        execution_status(&ctx.pool, owned.id).await?,
        ExecutionStatus::Cancelled
    );

    ctx.cleanup().await?;
    Ok(())
}
