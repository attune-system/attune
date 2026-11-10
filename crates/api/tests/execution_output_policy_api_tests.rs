//! Production HTTP boundaries for schema-selected secrecy and explicit reveal.
mod helpers;

use attune_common::{
    models::{Execution, ExecutionStatus, OutputFormat},
    output_classification::{classify_action_output, ENTITY_EXECUTION_OUTPUT_CHANNEL},
    repositories::{
        action::{ActionRepository, UpdateActionInput},
        component_lifecycle::PackProjectionIds,
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        execution_secret_value::ExecutionSecretValueRepository,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        schema_contract::{BindingOwner, SchemaContractRepository},
        Create, Delete, FindByRef, Update,
    },
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{
        prepare_secret_values, redact_secret_parameters, redaction_marker, ENTITY_EXECUTION_CONFIG,
        ENTITY_EXECUTION_RESULT,
    },
};
use axum::http::StatusCode;
use helpers::TestContext;
use serde_json::{json, Value};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

async fn execution_grant(ctx: &TestContext, refs: &[String], actions: &[&str]) -> Result<i64> {
    let identity = ctx.user.as_ref().unwrap().id;
    let set=PermissionSetRepository::create(&ctx.pool,CreatePermissionSetInput {
        r#ref:format!("fixture.output_{}",uuid::Uuid::new_v4().simple()),
        pack:None,pack_ref:None,label:None,description:None,
        grants:json!([{"resource":"executions","actions":actions,"constraints":{"refs":refs}}]),
    }).await?;
    let assignment = PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity).await;
    Ok(assignment.id)
}

async fn completed_output(
    ctx: &TestContext,
    secret_output: bool,
    origins: SecretProvenance,
) -> Result<Execution> {
    let pack = helpers::create_test_pack(
        &ctx.pool,
        &format!("output_{}", uuid::Uuid::new_v4().simple()),
    )
    .await?;
    let reference = format!("{}.run", pack.r#ref);
    let action = helpers::create_test_action(&ctx.pool, pack.id, &pack.r#ref, &reference).await?;
    let action=ActionRepository::update(&ctx.pool,action.id,UpdateActionInput {
        param_schema:Some(json!({"credential":{"type":"string","secret":true}})),
        out_schema:Some(json!({"status":{"type":"string"},"token":{"type":"string","secret":secret_output}})),
        output_format:Some(OutputFormat::Json.to_string()),
        ..Default::default()
    }).await?;
    helpers::activate_test_pack_release_with_projections(
        &ctx.pool,
        &pack,
        &PackProjectionIds {
            actions: vec![action.id],
            ..Default::default()
        },
    )
    .await?;
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&ctx.pool, action.id).await?;
    let mut tx = ctx.pool.begin().await?;
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
    let (config, input_secrets) = redact_secret_parameters(
        json!({"credential":"private-input-fixture"}),
        action.param_schema.as_ref(),
    );
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: Some(config),
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: Some(ctx.user.as_ref().unwrap().id),
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
    let key = ctx.state.config.security.encryption_key.as_deref().unwrap();
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        ENTITY_EXECUTION_CONFIG,
        execution.id,
        &prepare_secret_values(input_secrets, key)?,
    )
    .await?;
    let data = if secret_output {
        json!({"status":"public-status","token":"private-output-fixture"})
    } else {
        json!({"status":"public-status","token":"public-token"})
    };
    let classified = classify_action_output(
        json!({"succeeded":true,"data":data,"stdout":serde_json::to_string(&data)?}),
        binding.output.as_ref(),
        binding.output_format,
        binding.output_policy,
        execution.id,
        &origins,
    )?;
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        ENTITY_EXECUTION_RESULT,
        execution.id,
        &prepare_secret_values(classified.structured_secrets, key)?,
    )
    .await?;
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        ENTITY_EXECUTION_OUTPUT_CHANNEL,
        execution.id,
        &prepare_secret_values(classified.channel_secrets, key)?,
    )
    .await?;
    let execution = ExecutionRepository::update(
        &mut *tx,
        execution.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            result: Some(classified.public_result),
            ..Default::default()
        },
    )
    .await?;
    tx.commit().await?;
    Ok(execution)
}

async fn detail(ctx: &TestContext, id: i64, reveal: bool) -> Result<(StatusCode, Value)> {
    let response = ctx
        .get(
            &format!("/api/v1/executions/{id}?include_secret_values={reveal}"),
            ctx.token(),
        )
        .await?;
    let status = response.status();
    if status.is_success() {
        assert_eq!(
            response.headers()[axum::http::header::CACHE_CONTROL],
            "private, no-store"
        );
    }
    Ok((status, response.json().await?))
}

#[tokio::test]
async fn secret_inputs_do_not_hide_public_output_from_read_only_http_views() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let execution = completed_output(
        &ctx,
        false,
        SecretProvenance {
            origins: vec![SecretOrigin::Entity {
                entity_type: ENTITY_EXECUTION_CONFIG.into(),
                entity_id: 1,
                path: "/credential".into(),
            }],
            templates: Vec::new(),
        },
    )
    .await?;
    execution_grant(&ctx, std::slice::from_ref(&execution.action_ref), &["read"]).await?;
    let (status, body) = detail(&ctx, execution.id, false).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["config"]["credential"], redaction_marker());
    assert_eq!(body["data"]["result"]["data"]["status"], "public-status");
    assert_eq!(body["data"]["result"]["data"]["token"], "public-token");
    assert!(body["data"]["result"]["stdout"]
        .as_str()
        .unwrap()
        .contains("public-token"));
    assert_eq!(
        detail(&ctx, execution.id, true).await?.0,
        StatusCode::FORBIDDEN
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn authorized_http_reveal_restores_schema_secret_and_separate_original_channel() -> Result<()>
{
    let ctx = TestContext::new().await?.with_auth().await?;
    let execution = completed_output(&ctx, true, SecretProvenance::default()).await?;
    execution_grant(
        &ctx,
        std::slice::from_ref(&execution.action_ref),
        &["read", "decrypt"],
    )
    .await?;
    let (status, public) = detail(&ctx, execution.id, false).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        public["data"]["result"]["data"]["token"],
        redaction_marker()
    );
    assert_eq!(public["data"]["result"]["data"]["status"], "public-status");
    let sanitized = public["data"]["result"]["stdout"].clone();
    assert!(!sanitized
        .as_str()
        .unwrap()
        .contains("private-output-fixture"));
    assert!(public["data"]
        .get("revealed_output_channels")
        .is_none_or(Value::is_null));
    let (status, revealed) = detail(&ctx, execution.id, true).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        revealed["data"]["result"]["data"]["token"],
        "private-output-fixture"
    );
    assert_eq!(
        revealed["data"]["result"]["stdout"], sanitized,
        "revealing an original must not replace its public sanitized projection"
    );
    assert!(revealed["data"]["revealed_output_channels"]["stdout"]
        .as_str()
        .unwrap()
        .contains("private-output-fixture"));
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn admin_http_reveal_opens_input_derived_output_and_original_channels() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    attune_common::repositories::platform_catalog::PlatformCatalogRepository::reconcile(&ctx.pool)
        .await?;
    let administrator = PermissionSetRepository::find_by_ref(&ctx.pool, "core.admin")
        .await?
        .unwrap();
    let identity = ctx.user.as_ref().unwrap().id;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: administrator.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity).await;
    let source = completed_output(&ctx, true, SecretProvenance::default()).await?;
    let execution = completed_output(
        &ctx,
        true,
        SecretProvenance {
            origins: vec![SecretOrigin::Entity {
                entity_type: ENTITY_EXECUTION_CONFIG.into(),
                entity_id: source.id,
                path: "/credential".into(),
            }],
            templates: Vec::new(),
        },
    )
    .await?;
    let (status, public) = detail(&ctx, execution.id, false).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        public["data"]["result"]["data"]["token"],
        redaction_marker()
    );
    let (status, revealed) = detail(&ctx, execution.id, true).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        revealed["data"]["config"]["credential"],
        "private-input-fixture"
    );
    assert_eq!(
        revealed["data"]["result"]["data"]["token"],
        "private-output-fixture"
    );
    assert_eq!(
        revealed["data"]["result"]["stdout"],
        public["data"]["result"]["stdout"]
    );
    assert!(revealed["data"]["revealed_output_channels"]["stdout"]
        .as_str()
        .unwrap()
        .contains("private-output-fixture"));
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn source_grant_revocation_redacts_only_protected_fields_at_http_boundary() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let source = completed_output(&ctx, true, SecretProvenance::default()).await?;
    let execution = completed_output(
        &ctx,
        true,
        SecretProvenance {
            origins: vec![SecretOrigin::Entity {
                entity_type: ENTITY_EXECUTION_RESULT.into(),
                entity_id: source.id,
                path: "/data/token".into(),
            }],
            templates: Vec::new(),
        },
    )
    .await?;
    execution_grant(
        &ctx,
        std::slice::from_ref(&execution.action_ref),
        &["read", "decrypt"],
    )
    .await?;
    let (status, denied) = detail(&ctx, execution.id, true).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        denied["data"]["result"]["data"]["token"],
        redaction_marker()
    );
    assert_eq!(denied["data"]["result"]["data"]["status"], "public-status");
    let assignment = execution_grant(
        &ctx,
        std::slice::from_ref(&source.action_ref),
        &["read", "decrypt"],
    )
    .await?;
    let (status, allowed) = detail(&ctx, execution.id, true).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        allowed["data"]["result"]["data"]["token"],
        "private-output-fixture"
    );
    assert!(PermissionAssignmentRepository::delete(&ctx.pool, assignment).await?);
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(
        ctx.user.as_ref().unwrap().id,
    )
    .await;
    let (status, revoked) = detail(&ctx, execution.id, true).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        revoked["data"]["result"]["data"]["token"],
        redaction_marker()
    );
    assert_eq!(revoked["data"]["result"]["data"]["status"], "public-status");
    assert!(!revoked["data"]["revealed_output_channels"]
        .to_string()
        .contains("private-output-fixture"));
    ctx.cleanup().await?;
    Ok(())
}
