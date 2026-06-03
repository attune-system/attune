//! End-to-end authorization checks for execution-scoped API tokens.
//!
//! These tests exercise the API middleware, JWT metadata parsing, RBAC grant
//! loading, delegation checks, and execution creation path together.

use attune_common::{
    auth::jwt::{
        generate_execution_token_with_permission_sets,
        generate_execution_token_with_permission_sets_and_standard_access, JwtConfig,
        STANDARD_EXECUTION_ACCESS_REF,
    },
    models::{enums::ExecutionStatus, *},
    repositories::{
        action::{ActionRepository, CreateActionInput},
        artifact::{ArtifactRepository, CreateArtifactInput},
        execution::{CreateExecutionInput, ExecutionRepository},
        identity::{
            CreateIdentityInput, CreatePermissionSetInput, IdentityRepository,
            PermissionSetRepository,
        },
        key::{CreateKeyInput, KeyRepository},
        pack::{CreatePackInput, PackRepository},
        workflow::{CreateWorkflowDefinitionInput, WorkflowDefinitionRepository},
        Create,
    },
};
use axum::http::StatusCode;
use serde_json::{json, Value};
use sqlx::PgPool;

mod helpers;
use helpers::TestContext;

type TResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const TEST_JWT_SECRET: &str = "test-secret-for-testing-only-not-secure";

fn jwt_config() -> JwtConfig {
    JwtConfig {
        secret: TEST_JWT_SECRET.to_string(),
        access_token_expiration: 3600,
        refresh_token_expiration: 604800,
    }
}

async fn create_execution_identity(pool: &PgPool, suffix: &str) -> TResult<Identity> {
    Ok(IdentityRepository::create(
        pool,
        CreateIdentityInput {
            login: format!("exec_token_user_{}", suffix),
            display_name: Some("Execution Token User".to_string()),
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?)
}

async fn create_permission_set(
    pool: &PgPool,
    ref_name: &str,
    grants: Value,
) -> TResult<PermissionSet> {
    Ok(PermissionSetRepository::create(
        pool,
        CreatePermissionSetInput {
            r#ref: ref_name.to_string(),
            pack: None,
            pack_ref: None,
            label: Some(ref_name.to_string()),
            description: None,
            grants,
        },
    )
    .await?)
}

async fn setup_executable_action(pool: &PgPool, suffix: &str) -> TResult<(Pack, Action)> {
    let pack = PackRepository::create(
        pool,
        CreatePackInput {
            r#ref: format!("exec_token_pack_{}", suffix),
            label: "Execution Token Pack".to_string(),
            description: None,
            version: "1.0.0".to_string(),
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            tags: vec![],
            runtime_deps: vec![],
            dependencies: vec![],
            is_standard: false,
            installers: json!({}),
        },
    )
    .await?;

    let action = ActionRepository::create(
        pool,
        CreateActionInput {
            r#ref: format!("{}.call_api", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Call API".to_string(),
            description: None,
            entrypoint: "echo ok".to_string(),
            runtime: None,
            enabled: true,
            runtime_version_constraint: None,
            required_worker_runtimes: json!({}),
            worker_selector: json!({}),
            worker_tolerations: json!([]),
            worker_affinity: json!({}),
            param_schema: None,
            out_schema: None,
            is_adhoc: true,
            accesses_mcp: true,
            default_execution_permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
        },
    )
    .await?;

    Ok((pack, action))
}

async fn create_pack(pool: &PgPool, ref_name: &str, label: &str) -> TResult<Pack> {
    Ok(PackRepository::create(
        pool,
        CreatePackInput {
            r#ref: ref_name.to_string(),
            label: label.to_string(),
            description: None,
            version: "1.0.0".to_string(),
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            tags: vec![],
            runtime_deps: vec![],
            dependencies: vec![],
            is_standard: false,
            installers: json!({}),
        },
    )
    .await?)
}

async fn create_action_with_default_access(
    pool: &PgPool,
    pack: &Pack,
    name: &str,
    default_refs: Vec<String>,
) -> TResult<Action> {
    Ok(ActionRepository::create(
        pool,
        CreateActionInput {
            r#ref: format!("{}.{}", pack.r#ref, name),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: name.to_string(),
            description: None,
            entrypoint: "echo ok".to_string(),
            runtime: None,
            enabled: true,
            runtime_version_constraint: None,
            required_worker_runtimes: json!({}),
            worker_selector: json!({}),
            worker_tolerations: json!([]),
            worker_affinity: json!({}),
            param_schema: None,
            out_schema: None,
            is_adhoc: true,
            accesses_mcp: true,
            default_execution_permission_set_refs: default_refs,
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
        },
    )
    .await?)
}

async fn create_workflow_action_with_standard_access(
    pool: &PgPool,
    pack: &Pack,
    name: &str,
) -> TResult<Action> {
    let action = create_action_with_default_access(
        pool,
        pack,
        name,
        vec![STANDARD_EXECUTION_ACCESS_REF.to_string()],
    )
    .await?;
    let workflow_def = WorkflowDefinitionRepository::create(
        pool,
        CreateWorkflowDefinitionInput {
            r#ref: action.r#ref.clone(),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: action.label.clone(),
            description: None,
            version: "1.0".to_string(),
            param_schema: None,
            out_schema: None,
            definition: json!({
                "version": "1.0",
                "tasks": {}
            }),
            tags: vec![],
        },
    )
    .await?;

    Ok(ActionRepository::link_workflow_def(pool, action.id, workflow_def.id).await?)
}

async fn create_plain_key_for_pack(pool: &PgPool, pack: &Pack, key_ref: &str) -> TResult<Key> {
    Ok(KeyRepository::create(
        pool,
        CreateKeyInput {
            r#ref: key_ref.to_string(),
            owner_type: OwnerType::Pack,
            owner: None,
            owner_identity: None,
            owner_pack: Some(pack.id),
            owner_pack_ref: Some(pack.r#ref.clone()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: key_ref.to_string(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!({"token": key_ref}),
        },
    )
    .await?)
}

async fn create_plain_key_for_action(
    pool: &PgPool,
    action: &Action,
    key_ref: &str,
) -> TResult<Key> {
    Ok(KeyRepository::create(
        pool,
        CreateKeyInput {
            r#ref: key_ref.to_string(),
            owner_type: OwnerType::Action,
            owner: None,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: Some(action.id),
            owner_action_ref: Some(action.r#ref.clone()),
            owner_sensor: None,
            owner_sensor_ref: None,
            name: key_ref.to_string(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!({"token": key_ref}),
        },
    )
    .await?)
}

async fn create_artifact_for_pack(
    pool: &PgPool,
    pack: &Pack,
    artifact_ref: &str,
) -> TResult<artifact::Artifact> {
    Ok(ArtifactRepository::create(
        pool,
        CreateArtifactInput {
            r#ref: artifact_ref.to_string(),
            scope: OwnerType::Pack,
            owner: pack.r#ref.clone(),
            r#type: ArtifactType::Progress,
            visibility: ArtifactVisibility::Private,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 5,
            name: Some(artifact_ref.to_string()),
            description: None,
            content_type: Some("application/json".to_string()),
            data: Some(json!([])),
        },
    )
    .await?)
}

async fn create_artifact_for_action(
    pool: &PgPool,
    action: &Action,
    artifact_ref: &str,
) -> TResult<artifact::Artifact> {
    Ok(ArtifactRepository::create(
        pool,
        CreateArtifactInput {
            r#ref: artifact_ref.to_string(),
            scope: OwnerType::Action,
            owner: action.r#ref.clone(),
            r#type: ArtifactType::Progress,
            visibility: ArtifactVisibility::Private,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 5,
            name: Some(artifact_ref.to_string()),
            description: None,
            content_type: Some("application/json".to_string()),
            data: Some(json!([])),
        },
    )
    .await?)
}

async fn create_parent_execution(
    pool: &PgPool,
    action: &Action,
    identity: &Identity,
) -> TResult<Execution> {
    Ok(ExecutionRepository::create(
        pool,
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
            result: None,
            workflow_task: None,
        },
    )
    .await?)
}

fn execution_token(
    identity: &Identity,
    parent_execution: &Execution,
    action: &Action,
    refs: &[String],
) -> TResult<String> {
    Ok(generate_execution_token_with_permission_sets(
        identity.id,
        parent_execution.id,
        &action.r#ref,
        &jwt_config(),
        Some(600),
        refs,
    )?)
}

fn standard_execution_token(
    identity: &Identity,
    execution: &Execution,
    action_ref: &str,
    standard_action_refs: &[String],
) -> TResult<String> {
    Ok(
        generate_execution_token_with_permission_sets_and_standard_access(
            identity.id,
            execution.id,
            action_ref,
            &jwt_config(),
            Some(600),
            &[STANDARD_EXECUTION_ACCESS_REF.to_string()],
            standard_action_refs,
        )?,
    )
}

fn execute_body(action: &Action, permission_set_refs: Option<Vec<String>>) -> Value {
    let mut body = json!({
        "action_ref": action.r#ref,
        "parameters": {
            "message": "hello"
        }
    });

    if let Some(refs) = permission_set_refs {
        body["permission_set_refs"] = json!(refs);
    }

    body
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn execution_tokens_are_limited_to_embedded_permission_sets() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let suffix = uuid::Uuid::new_v4().to_string().replace('-', "");
    let suffix = &suffix[..8];
    let identity = create_execution_identity(&ctx.pool, suffix).await?;
    let (_pack, action) = setup_executable_action(&ctx.pool, suffix).await?;
    let parent_execution = create_parent_execution(&ctx.pool, &action, &identity).await?;

    let runtime_read_ref = format!("test.exec_token_runtime_read_{}", suffix);
    let worker_read_ref = format!("test.exec_token_worker_read_{}", suffix);
    let action_read_ref = format!("test.exec_token_action_read_{}", suffix);
    let action_execute_ref = format!("test.exec_token_action_execute_{}", suffix);

    create_permission_set(
        &ctx.pool,
        &runtime_read_ref,
        json!([
            {"resource": "runtimes", "actions": ["read"]}
        ]),
    )
    .await?;
    create_permission_set(
        &ctx.pool,
        &worker_read_ref,
        json!([
            {"resource": "workers", "actions": ["read"]}
        ]),
    )
    .await?;
    create_permission_set(
        &ctx.pool,
        &action_read_ref,
        json!([
            {
                "resource": "actions",
                "actions": ["read"],
                "constraints": {"refs": [action.r#ref.clone()]}
            }
        ]),
    )
    .await?;
    create_permission_set(
        &ctx.pool,
        &action_execute_ref,
        json!([
            {
                "resource": "actions",
                "actions": ["execute"],
                "constraints": {"refs": [action.r#ref.clone()]}
            }
        ]),
    )
    .await?;

    let missing_token = ctx.get("/api/v1/runtimes", None).await?;
    assert_eq!(missing_token.status(), StatusCode::UNAUTHORIZED);

    let empty_token = execution_token(&identity, &parent_execution, &action, &[])?;
    let empty_read = ctx.get("/api/v1/runtimes", Some(&empty_token)).await?;
    assert_eq!(empty_read.status(), StatusCode::FORBIDDEN);

    let empty_execute = ctx
        .post(
            "/api/v1/executions/execute",
            execute_body(&action, None),
            Some(&empty_token),
        )
        .await?;
    assert_eq!(empty_execute.status(), StatusCode::FORBIDDEN);

    let read_only_refs = vec![
        runtime_read_ref.clone(),
        worker_read_ref.clone(),
        action_read_ref.clone(),
    ];
    let read_only_token = execution_token(&identity, &parent_execution, &action, &read_only_refs)?;

    let runtime_read = ctx.get("/api/v1/runtimes", Some(&read_only_token)).await?;
    assert_eq!(runtime_read.status(), StatusCode::OK);

    let worker_read = ctx.get("/api/v1/workers", Some(&read_only_token)).await?;
    assert_eq!(worker_read.status(), StatusCode::OK);

    let read_only_create = ctx
        .post(
            "/api/v1/runtimes",
            json!({
                "ref": format!("test.exec_token_denied_runtime_{}", suffix),
                "name": "Denied Runtime",
                "distributions": {},
                "execution_config": {}
            }),
            Some(&read_only_token),
        )
        .await?;
    assert_eq!(read_only_create.status(), StatusCode::FORBIDDEN);

    let read_only_execute = ctx
        .post(
            "/api/v1/executions/execute",
            execute_body(&action, None),
            Some(&read_only_token),
        )
        .await?;
    assert_eq!(read_only_execute.status(), StatusCode::FORBIDDEN);

    let execute_only_refs = vec![action_execute_ref.clone()];
    let execute_only_token =
        execution_token(&identity, &parent_execution, &action, &execute_only_refs)?;

    let execute_only_runtime_read = ctx
        .get("/api/v1/runtimes", Some(&execute_only_token))
        .await?;
    assert_eq!(execute_only_runtime_read.status(), StatusCode::FORBIDDEN);

    let execute_only_delegation = ctx
        .post(
            "/api/v1/executions/execute",
            execute_body(&action, Some(vec![runtime_read_ref.clone()])),
            Some(&execute_only_token),
        )
        .await?;
    assert_eq!(execute_only_delegation.status(), StatusCode::FORBIDDEN);

    let execute_only_success = ctx
        .post(
            "/api/v1/executions/execute",
            execute_body(&action, Some(Vec::new())),
            Some(&execute_only_token),
        )
        .await?;
    assert_eq!(execute_only_success.status(), StatusCode::CREATED);
    let no_permission_body: Value = execute_only_success.json().await?;
    assert_eq!(
        no_permission_body["data"]["parent"].as_i64(),
        Some(parent_execution.id)
    );
    assert!(
        no_permission_body["data"]["permission_set_refs"].is_null(),
        "empty execution permission refs should be persisted as an empty set and omitted from the response"
    );

    let combined_refs = vec![action_execute_ref.clone(), runtime_read_ref.clone()];
    let combined_token = execution_token(&identity, &parent_execution, &action, &combined_refs)?;

    let combined_runtime_read = ctx.get("/api/v1/runtimes", Some(&combined_token)).await?;
    assert_eq!(combined_runtime_read.status(), StatusCode::OK);

    let delegated_execution = ctx
        .post(
            "/api/v1/executions/execute",
            execute_body(&action, Some(vec![runtime_read_ref.clone()])),
            Some(&combined_token),
        )
        .await?;
    assert_eq!(delegated_execution.status(), StatusCode::CREATED);
    let delegated_body: Value = delegated_execution.json().await?;
    assert_eq!(
        delegated_body["data"]["parent"].as_i64(),
        Some(parent_execution.id)
    );
    assert_eq!(
        delegated_body["data"]["executor"].as_i64(),
        Some(identity.id)
    );
    assert_eq!(
        delegated_body["data"]["permission_set_refs"],
        json!([runtime_read_ref])
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn standard_execution_access_covers_action_and_pack_scoped_resources() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let suffix = uuid::Uuid::new_v4().to_string().replace('-', "");
    let suffix = &suffix[..8];
    let identity = create_execution_identity(&ctx.pool, suffix).await?;

    let salesforce_pack =
        create_pack(&ctx.pool, &format!("sf_std_{}", suffix), "Salesforce").await?;
    let action = create_action_with_default_access(
        &ctx.pool,
        &salesforce_pack,
        "read_sobject",
        vec![STANDARD_EXECUTION_ACCESS_REF.to_string()],
    )
    .await?;
    let unrelated_pack = create_pack(&ctx.pool, &format!("other_std_{}", suffix), "Other").await?;
    let unrelated_action =
        create_action_with_default_access(&ctx.pool, &unrelated_pack, "noop", Vec::new()).await?;

    let pack_key_ref = format!("{}.pack_key", salesforce_pack.r#ref);
    let action_key_ref = format!("{}.action_key", action.r#ref);
    let unrelated_key_ref = format!("{}.pack_key", unrelated_pack.r#ref);
    create_plain_key_for_pack(&ctx.pool, &salesforce_pack, &pack_key_ref).await?;
    create_plain_key_for_action(&ctx.pool, &action, &action_key_ref).await?;
    create_plain_key_for_pack(&ctx.pool, &unrelated_pack, &unrelated_key_ref).await?;

    let pack_artifact_ref = format!("{}.pack_artifact", salesforce_pack.r#ref);
    let action_artifact_ref = format!("{}.action_artifact", action.r#ref);
    let unrelated_artifact_ref = format!("{}.pack_artifact", unrelated_pack.r#ref);
    create_artifact_for_pack(&ctx.pool, &salesforce_pack, &pack_artifact_ref).await?;
    create_artifact_for_action(&ctx.pool, &action, &action_artifact_ref).await?;
    create_artifact_for_pack(&ctx.pool, &unrelated_pack, &unrelated_artifact_ref).await?;

    let execution = create_parent_execution(&ctx.pool, &action, &identity).await?;
    let token = standard_execution_token(
        &identity,
        &execution,
        &action.r#ref,
        std::slice::from_ref(&action.r#ref),
    )?;

    assert_eq!(
        ctx.get(&format!("/api/v1/keys/{}", pack_key_ref), Some(&token))
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.get(&format!("/api/v1/keys/{}", action_key_ref), Some(&token))
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.get(&format!("/api/v1/keys/{}", unrelated_key_ref), Some(&token))
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        ctx.get(
            &format!("/api/v1/artifacts/ref/{}", pack_artifact_ref),
            Some(&token)
        )
        .await?
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.get(
            &format!("/api/v1/artifacts/ref/{}", action_artifact_ref),
            Some(&token)
        )
        .await?
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.get(
            &format!("/api/v1/artifacts/ref/{}", unrelated_artifact_ref),
            Some(&token)
        )
        .await?
        .status(),
        StatusCode::NOT_FOUND
    );

    let create_allowed = ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref": format!("{}.created_by_standard", action.r#ref),
                "scope": "action",
                "owner": action.r#ref,
                "type": "progress",
                "visibility": "private",
                "name": "Created by standard access",
                "data": []
            }),
            Some(&token),
        )
        .await?;
    assert_eq!(create_allowed.status(), StatusCode::CREATED);

    let create_denied = ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref": format!("{}.created_by_standard", unrelated_action.r#ref),
                "scope": "action",
                "owner": unrelated_action.r#ref,
                "type": "progress",
                "visibility": "private",
                "name": "Denied standard access",
                "data": []
            }),
            Some(&token),
        )
        .await?;
    assert_eq!(create_denied.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn workflow_task_standard_access_includes_workflow_action_scope() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let suffix = uuid::Uuid::new_v4().to_string().replace('-', "");
    let suffix = &suffix[..8];
    let identity = create_execution_identity(&ctx.pool, suffix).await?;

    let workflow_pack = create_pack(&ctx.pool, &format!("wf_std_{}", suffix), "Workflow").await?;
    let workflow_action =
        create_workflow_action_with_standard_access(&ctx.pool, &workflow_pack, "sync_records")
            .await?;
    let sql_pack = create_pack(&ctx.pool, &format!("sql_std_{}", suffix), "SQL").await?;
    let sql_action = create_action_with_default_access(
        &ctx.pool,
        &sql_pack,
        "query",
        vec![STANDARD_EXECUTION_ACCESS_REF.to_string()],
    )
    .await?;
    let unrelated_pack =
        create_pack(&ctx.pool, &format!("unrelated_std_{}", suffix), "Unrelated").await?;

    let workflow_pack_key_ref = format!("{}.db_credentials", workflow_pack.r#ref);
    let workflow_action_key_ref = format!("{}.runtime_override", workflow_action.r#ref);
    let sql_action_key_ref = format!("{}.driver_config", sql_action.r#ref);
    let unrelated_key_ref = format!("{}.secret", unrelated_pack.r#ref);
    create_plain_key_for_pack(&ctx.pool, &workflow_pack, &workflow_pack_key_ref).await?;
    create_plain_key_for_action(&ctx.pool, &workflow_action, &workflow_action_key_ref).await?;
    create_plain_key_for_action(&ctx.pool, &sql_action, &sql_action_key_ref).await?;
    create_plain_key_for_pack(&ctx.pool, &unrelated_pack, &unrelated_key_ref).await?;

    let workflow_artifact_ref = format!("{}.state", workflow_pack.r#ref);
    let sql_artifact_ref = format!("{}.rows", sql_action.r#ref);
    let unrelated_artifact_ref = format!("{}.state", unrelated_pack.r#ref);
    create_artifact_for_pack(&ctx.pool, &workflow_pack, &workflow_artifact_ref).await?;
    create_artifact_for_action(&ctx.pool, &sql_action, &sql_artifact_ref).await?;
    create_artifact_for_pack(&ctx.pool, &unrelated_pack, &unrelated_artifact_ref).await?;

    let workflow_execution =
        create_parent_execution(&ctx.pool, &workflow_action, &identity).await?;
    let token = standard_execution_token(
        &identity,
        &workflow_execution,
        &sql_action.r#ref,
        &[sql_action.r#ref.clone(), workflow_action.r#ref.clone()],
    )?;

    for key_ref in [
        &workflow_pack_key_ref,
        &workflow_action_key_ref,
        &sql_action_key_ref,
    ] {
        assert_eq!(
            ctx.get(&format!("/api/v1/keys/{}", key_ref), Some(&token))
                .await?
                .status(),
            StatusCode::OK,
            "expected workflow task standard token to read key {}",
            key_ref
        );
    }

    assert_eq!(
        ctx.get(&format!("/api/v1/keys/{}", unrelated_key_ref), Some(&token))
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );

    for artifact_ref in [&workflow_artifact_ref, &sql_artifact_ref] {
        assert_eq!(
            ctx.get(
                &format!("/api/v1/artifacts/ref/{}", artifact_ref),
                Some(&token)
            )
            .await?
            .status(),
            StatusCode::OK,
            "expected workflow task standard token to read artifact {}",
            artifact_ref
        );
    }
    assert_eq!(
        ctx.get(
            &format!("/api/v1/artifacts/ref/{}", unrelated_artifact_ref),
            Some(&token)
        )
        .await?
        .status(),
        StatusCode::NOT_FOUND
    );

    let workflow_pack_create = ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref": format!("{}.created_by_child", workflow_pack.r#ref),
                "scope": "pack",
                "owner": workflow_pack.r#ref,
                "type": "progress",
                "visibility": "private",
                "name": "Created by workflow child",
                "data": []
            }),
            Some(&token),
        )
        .await?;
    assert_eq!(workflow_pack_create.status(), StatusCode::CREATED);

    let unrelated_create = ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref": format!("{}.created_by_child", unrelated_pack.r#ref),
                "scope": "pack",
                "owner": unrelated_pack.r#ref,
                "type": "progress",
                "visibility": "private",
                "name": "Denied workflow child",
                "data": []
            }),
            Some(&token),
        )
        .await?;
    assert_eq!(unrelated_create.status(), StatusCode::FORBIDDEN);
    Ok(())
}
