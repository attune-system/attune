mod helpers;

use std::time::Duration;

use attune_common::{
    models::enums::ExecutionStatus,
    repositories::{
        execution::{CreateExecutionInput, ExecutionRepository},
        execution_log_stream_lease::ExecutionLogStreamLeaseRepository,
        Create,
    },
};
use axum::http::StatusCode;
use helpers::{create_test_action, create_test_pack, TestContext};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

async fn stream_fixture(ctx: &TestContext) -> Result<(i64, String)> {
    let pack = create_test_pack(
        &ctx.pool,
        &format!("stream_{}", uuid::Uuid::new_v4().simple()),
    )
    .await?;
    let action_ref = format!("{}.tail", pack.r#ref);
    let action = create_test_action(&ctx.pool, pack.id, &pack.r#ref, &action_ref).await?;
    helpers::activate_test_pack_release_with_projections(
        &ctx.pool,
        &pack,
        &attune_common::repositories::component_lifecycle::PackProjectionIds {
            actions: vec![action.id],
            ..Default::default()
        },
    )
    .await?;
    use attune_common::repositories::{
        executable_snapshot::ExecutableSnapshotRepository,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        schema_contract::{BindingOwner, SchemaContractRepository},
    };
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&ctx.pool, action.id).await?;
    let mut tx = ctx.pool.begin().await?;
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
    let identity = ctx.user.as_ref().expect("authenticated identity");
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: Some(identity.id),
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Running,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
        &snapshot,
    )
    .await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
        .await?;
    tx.commit().await?;
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("fixture.stream_read_{}", execution.id),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: serde_json::json!([{"resource":"executions","actions":["read"],
            "constraints":{"ids":[execution.id]}}]),
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
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    let token = ctx
        .token()
        .expect("authenticated identity token")
        .to_owned();
    Ok((execution.id, token))
}

enum LogAccess {
    Disabled,
    Named,
    Standard,
}

async fn bound_legacy_stream_fixture(
    ctx: &TestContext,
    access: LogAccess,
) -> Result<(i64, String)> {
    use attune_common::{
        auth::jwt::{
            generate_execution_token_with_permission_sets_and_standard_access,
            STANDARD_EXECUTION_ACCESS_REF,
        },
        repositories::{
            action::{ActionRepository, CreateActionInput},
            executable_snapshot::ExecutableSnapshotRepository,
            identity::{
                CreatePermissionAssignmentInput, CreatePermissionSetInput,
                PermissionAssignmentRepository, PermissionSetRepository,
            },
            schema_contract::{BindingOwner, SchemaContractRepository},
        },
    };
    let pack = create_test_pack(
        &ctx.pool,
        &format!("optin_{}", uuid::Uuid::new_v4().simple()),
    )
    .await?;
    let action = ActionRepository::create(
        &ctx.pool,
        CreateActionInput {
            r#ref: format!("{}.tail", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Ad-hoc log caller".into(),
            description: None,
            entrypoint: "main.py".into(),
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
    .await?;
    helpers::activate_test_pack_release(&ctx.pool, &pack).await?;
    let identity = ctx.user.as_ref().expect("authenticated identity");
    let reference = format!("fixture.log_optin_{}", uuid::Uuid::new_v4().simple());
    let selected = match access {
        LogAccess::Disabled => Vec::new(),
        LogAccess::Named => vec![reference.clone()],
        LogAccess::Standard => vec![STANDARD_EXECUTION_ACCESS_REF.to_owned()],
    };
    let mut tx = ctx.pool.begin().await?;
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id).await?;
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            executor: Some(identity.id),
            permission_set_refs: selected.clone(),
            status: ExecutionStatus::Running,
            ..Default::default()
        },
        &snapshot,
    )
    .await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
        .await?;
    // Both fixtures' real owner has this authority. Only opt-in selects it for the token.
    let set = PermissionSetRepository::create(
        &mut *tx,
        CreatePermissionSetInput {
            r#ref: reference,
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: serde_json::json!([{"resource":"executions","actions":["read"],
            "constraints":{"ids":[execution.id]}}]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &mut *tx,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: set.id,
        },
    )
    .await?;
    tx.commit().await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    let token = generate_execution_token_with_permission_sets_and_standard_access(
        identity.id,
        execution.id,
        &action.r#ref,
        &ctx.state.jwt_config,
        Some(300),
        &selected,
        std::slice::from_ref(&action.r#ref),
    )?;
    Ok((execution.id, token))
}

#[tokio::test]
async fn empty_ref_execution_token_cannot_stream_validly_bound_own_logs() -> Result<()> {
    let ctx = TestContext::new_with_stream_limits(2, 2)
        .await?
        .with_auth()
        .await?;
    let (id, token) = bound_legacy_stream_fixture(&ctx, LogAccess::Disabled).await?;
    for stream in ["stdout", "stderr"] {
        let response = ctx
            .get(
                &format!("/api/v1/executions/{id}/logs/{stream}/stream"),
                Some(&token),
            )
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "an original nonsensitive schema binding must not grant an empty-ref workload API access"
        );
        drop(response.into_response());
    }
    wait_for_no_leases(&ctx).await?;
    Ok(())
}

#[tokio::test]
async fn opted_in_execution_log_token_is_narrow_and_requires_active_owner() -> Result<()> {
    use attune_common::repositories::{
        identity::{IdentityRepository, UpdateIdentityInput},
        Update,
    };
    let ctx = TestContext::new_with_stream_limits(2, 2)
        .await?
        .with_auth()
        .await?;
    let (id, token) = bound_legacy_stream_fixture(&ctx, LogAccess::Named).await?;
    let (other, _) = bound_legacy_stream_fixture(&ctx, LogAccess::Named).await?;
    let response = ctx
        .get(
            &format!("/api/v1/executions/{id}/logs/stdout/stream"),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    drop(response.into_response());
    wait_for_no_leases(&ctx).await?;
    assert_eq!(
        ctx.get(
            &format!("/api/v1/executions/{other}/logs/stdout/stream"),
            Some(&token)
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    let identity = ctx.user.as_ref().unwrap().id;
    IdentityRepository::update(
        &ctx.pool,
        identity,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity).await;
    assert_eq!(
        ctx.get(
            &format!("/api/v1/executions/{id}/logs/stdout/stream"),
            Some(&token)
        )
        .await?
        .status(),
        StatusCode::UNAUTHORIZED
    );
    wait_for_no_leases(&ctx).await?;
    Ok(())
}

#[tokio::test]
async fn standard_execution_log_access_remains_scoped_to_own_execution() -> Result<()> {
    let ctx = TestContext::new_with_stream_limits(2, 2)
        .await?
        .with_auth()
        .await?;
    let (id, token) = bound_legacy_stream_fixture(&ctx, LogAccess::Standard).await?;
    let (other, _) = bound_legacy_stream_fixture(&ctx, LogAccess::Standard).await?;
    for stream in ["stdout", "stderr"] {
        let response = ctx
            .get(
                &format!("/api/v1/executions/{id}/logs/{stream}/stream"),
                Some(&token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        drop(response.into_response());
        wait_for_no_leases(&ctx).await?;
        assert_eq!(
            ctx.get(
                &format!("/api/v1/executions/{other}/logs/{stream}/stream"),
                Some(&token),
            )
            .await?
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    wait_for_no_leases(&ctx).await?;
    Ok(())
}

#[tokio::test]
async fn empty_ref_denial_precedes_log_stream_lease_admission() -> Result<()> {
    let ctx = TestContext::new_with_stream_limits(1, 1)
        .await?
        .with_auth()
        .await?;
    let (id, token) = bound_legacy_stream_fixture(&ctx, LogAccess::Disabled).await?;
    let (occupied, reader) = stream_fixture(&ctx).await?;
    let open = ctx
        .get(
            &format!("/api/v1/executions/{occupied}/logs/stdout/stream"),
            Some(&reader),
        )
        .await?;
    assert_eq!(open.status(), StatusCode::OK);
    assert_eq!(
        ExecutionLogStreamLeaseRepository::active_count(&ctx.pool).await?,
        1
    );
    assert_eq!(
        ctx.get(
            &format!("/api/v1/executions/{id}/logs/stdout/stream"),
            Some(&token),
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN,
        "missing opt-in must be denied before a saturated stream limit returns 429"
    );
    assert_eq!(
        ExecutionLogStreamLeaseRepository::active_count(&ctx.pool).await?,
        1
    );
    drop(open.into_response());
    wait_for_no_leases(&ctx).await?;
    Ok(())
}

async fn wait_for_no_leases(ctx: &TestContext) -> Result<()> {
    for _ in 0..50 {
        if ExecutionLogStreamLeaseRepository::active_count(&ctx.pool).await? == 0 {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Err("execution log stream lease was not released".into())
}

#[tokio::test]
async fn route_returns_429_and_dropping_sse_body_releases_lease() -> Result<()> {
    let ctx = TestContext::new_with_stream_limits(2, 1)
        .await?
        .with_auth()
        .await?;
    let (execution_id, token) = stream_fixture(&ctx).await?;
    let path = format!("/api/v1/executions/{execution_id}/logs/stdout/stream");

    let first = ctx.get(&path, Some(&token)).await?;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(
        ExecutionLogStreamLeaseRepository::active_count(&ctx.pool).await?,
        1
    );

    let rejected = ctx.get(&path, Some(&token)).await?;
    assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(rejected.headers()["retry-after"], "1");
    let body: serde_json::Value = rejected.json().await?;
    assert_eq!(body["code"], "TOO_MANY_REQUESTS");
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("close another stream"));

    drop(first.into_response());
    wait_for_no_leases(&ctx).await?;

    let replacement = ctx.get(&path, Some(&token)).await?;
    assert_eq!(replacement.status(), StatusCode::OK);
    drop(replacement.into_response());
    wait_for_no_leases(&ctx).await?;
    Ok(())
}

#[tokio::test]
async fn server_shutdown_waits_for_reconnect_and_lease_release() -> Result<()> {
    let ctx = TestContext::new_with_stream_limits(1, 1)
        .await?
        .with_auth()
        .await?;
    let (execution_id, token) = stream_fixture(&ctx).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = attune_api::server::Server::new(ctx.state.clone());
    let mut server_task = tokio::spawn(server.run_with_listener(listener));
    let response = reqwest::Client::new()
        .get(format!(
            "http://{address}/api/v1/executions/{execution_id}/logs/stdout/stream"
        ))
        .bearer_auth(token)
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let mut release_blocker = ctx.pool.begin().await?;
    sqlx::query("SELECT id FROM execution_log_stream_lease FOR UPDATE")
        .fetch_one(&mut *release_blocker)
        .await?;

    ctx.state.execution_log_streams.begin_shutdown();
    let body = tokio::time::timeout(Duration::from_secs(1), response.text()).await??;
    assert!(body.contains("event: error"));
    assert!(body.contains("server_shutting_down"));
    assert!(body.contains("reconnect using the last event ID"));

    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut server_task)
            .await
            .is_err()
    );
    release_blocker.rollback().await?;
    tokio::time::timeout(Duration::from_secs(1), &mut server_task).await???;
    assert_eq!(
        ExecutionLogStreamLeaseRepository::active_count(&ctx.pool).await?,
        0
    );
    Ok(())
}
