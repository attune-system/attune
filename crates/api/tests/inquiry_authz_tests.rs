//! Authorization tests for the inquiry response endpoint.
//!
//! Verifies the security guarantees added in `inquiry-assignee-edge-cases`:
//!
//! - `assigned_to` is an *enforced* lock (only the assignee may respond).
//! - Tokens without a resolvable identity are rejected with 403.
//! - Execution-scoped tokens whose `execution_id` matches
//!   `inquiry.created_by_execution`
//!   are blocked (privilege-loop guard) — an execution cannot answer an
//!   inquiry it created.
//! - Execution-scoped tokens for a *different* execution may respond when
//!   they belong to the assignee.
//! - When `assigned_to` is unset, any authenticated caller may respond
//!   (existing behavior).

use attune_common::{
    auth::jwt::{
        generate_access_token, generate_execution_token,
        generate_execution_token_with_permission_sets, generate_sensor_token_with_workload_fence,
        JwtConfig,
    },
    crypto::encrypt_json,
    inquiry_response_handle::issue_inquiry_response_handle,
    models::{enums::ExecutionStatus, *},
    repositories::{
        action::{ActionRepository, CreateActionInput},
        execution::{CreateExecutionInput, ExecutionRepository},
        external_identity_mapping::{
            CreateExternalIdentityMappingInput, ExternalIdentityMappingRepository,
        },
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
        },
        inquiry::{CreateInquiryInput, CreateWorkflowInquiryInput, InquiryRepository},
        inquiry_callback_delivery::{
            CreateInquiryCallbackDelivery, InquiryCallbackDeliveryRepository,
            InsertInquiryCallbackDelivery,
        },
        pack::{CreatePackInput, PackRepository},
        runtime::{CreateRuntimeInput, CreateWorkerInput, RuntimeRepository, WorkerRepository},
        sensor_admission::SensorAdmissionRepository,
        sensor_workload::{
            AcquireSensorWorkloadInput, AcquireSensorWorkloadOutcome, SensorWorkloadRepository,
        },
        trigger::{CreateSensorInput, SensorRepository, UpdateSensorInput},
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            WorkflowDefinitionRepository, WorkflowExecutionRepository,
        },
        Create, Delete, FindById, Update,
    },
};
use axum::http::StatusCode;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::time::Duration;

mod helpers;
use helpers::{activate_test_pack_release_with_projections, TestContext};

type TResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const TEST_JWT_SECRET: &str = "test-secret-for-testing-only-not-secure";

fn jwt_config() -> JwtConfig {
    JwtConfig {
        secret: TEST_JWT_SECRET.to_string(),
        access_token_expiration: 3600,
        refresh_token_expiration: 604800,
    }
}

async fn create_identity(pool: &PgPool, login: &str) -> TResult<Identity> {
    Ok(IdentityRepository::create(
        pool,
        CreateIdentityInput {
            login: login.to_string(),
            display_name: Some(login.to_string()),
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await?)
}

async fn setup_pack_action(pool: &PgPool, suffix: &str) -> TResult<(Pack, Action)> {
    let pack = PackRepository::create(
        pool,
        CreatePackInput {
            r#ref: format!("inq_test_{}", suffix),
            label: format!("Inquiry Test Pack {}", suffix),
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
            r#ref: format!("{}.ask", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Ask".to_string(),
            description: None,
            entrypoint: "ask.sh".to_string(),
            runtime: None,
            enabled: true,
            runtime_version_constraint: None,
            required_worker_runtimes: json!({}),
            worker_selector: json!({}),
            worker_tolerations: json!([]),
            worker_affinity: json!({}),
            param_schema: None,
            out_schema: None,
            is_adhoc: false,
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

    Ok((pack, action))
}

async fn create_execution(pool: &PgPool, action: &Action) -> TResult<Execution> {
    Ok(ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
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
    )
    .await?)
}

async fn create_child_execution(
    pool: &PgPool,
    action: &Action,
    parent_id: i64,
) -> TResult<Execution> {
    Ok(ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: Some(parent_id),
            enforcement: None,
            executor: None,
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
    )
    .await?)
}

async fn create_workflow_child_execution(
    pool: &PgPool,
    action: &Action,
    identity_id: i64,
    permission_set_ref: &str,
) -> TResult<Execution> {
    let parent = create_execution(pool, action).await?;
    let definition = WorkflowDefinitionRepository::create(
        pool,
        CreateWorkflowDefinitionInput {
            r#ref: format!("{}.workflow", action.pack_ref),
            pack: action.pack,
            pack_ref: action.pack_ref.clone(),
            label: "Inquiry creation test workflow".to_string(),
            description: None,
            version: "1.0.0".to_string(),
            param_schema: None,
            out_schema: None,
            definition: json!({"version": "1.0", "tasks": {}}),
            tags: Vec::new(),
        },
    )
    .await?;
    let workflow = WorkflowExecutionRepository::create(
        pool,
        CreateWorkflowExecutionInput {
            execution: parent.id,
            workflow_def: definition.id,
            task_graph: json!({"tasks": {}}),
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await?;

    Ok(ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            parent: Some(parent.id),
            executor: Some(identity_id),
            permission_set_refs: vec![permission_set_ref.to_string()],
            status: ExecutionStatus::Running,
            workflow_task: Some(WorkflowTaskMetadata {
                workflow_execution: workflow.id,
                task_name: "request_approval".to_string(),
                triggered_by: None,
                task_index: None,
                task_batch: None,
                retry_count: 0,
                max_retries: 0,
                next_retry_at: None,
                timeout_seconds: None,
                timed_out: false,
                duration_ms: None,
                started_at: Some(chrono::Utc::now()),
                completed_at: None,
            }),
            ..Default::default()
        },
    )
    .await?)
}

async fn create_inquiry(
    pool: &PgPool,
    execution_id: i64,
    assigned_to: Option<i64>,
) -> TResult<Inquiry> {
    Ok(InquiryRepository::create(
        pool,
        CreateInquiryInput {
            created_by_execution: execution_id,
            prompt: "Approve?".to_string(),
            response_schema: None,
            response_options: vec![InquiryResponseOption {
                r#ref: "approve".to_string(),
                label: "Approve".to_string(),
                style: InquiryResponseOptionStyle::Positive,
                response: json!({"approved": true}),
            }],
            assigned_to,
            status: attune_common::models::enums::InquiryStatus::Pending,
            response: None,
            timeout_at: None,
        },
    )
    .await?)
}

fn respond_body() -> serde_json::Value {
    json!({ "response": { "approved": true } })
}

#[tokio::test]
async fn inquiry_reads_enrich_visible_context_and_protect_workflow_filters() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let creator = create_identity(&ctx.pool, "inquiry_context_creator").await?;
    let assignee = create_identity(&ctx.pool, "inquiry_context_assignee").await?;
    let viewer = create_identity(&ctx.pool, "inquiry_context_viewer").await?;
    let viewer_permissions = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "test.inquiry_context_viewer".to_string(),
            pack: None,
            pack_ref: None,
            label: Some("Inquiry context viewer".to_string()),
            description: None,
            grants: json!([
                {"resource": "inquiries", "actions": ["read"]},
                {"resource": "executions", "actions": ["read"]}
            ]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: viewer.id,
            permset: viewer_permissions.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(viewer.id).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let viewer_token = generate_access_token(viewer.id, &viewer.login, &jwt_config())?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "read_context").await?;
    let child = create_workflow_child_execution(&ctx.pool, &action, creator.id, "unused").await?;
    let workflow_execution = child
        .workflow_task
        .as_ref()
        .expect("workflow child metadata")
        .workflow_execution;
    let workflow_root_execution = child.parent.expect("workflow root execution");

    let mut conn = ctx.pool.acquire().await?;
    let inquiry = InquiryRepository::create_workflow_inquiry_idempotent(
        &mut conn,
        CreateWorkflowInquiryInput {
            created_by_execution: child.id,
            purpose: "read-context".to_string(),
            prompt: "Approve enriched context?".to_string(),
            response_schema: None,
            response_options: vec![InquiryResponseOption {
                r#ref: "approve".to_string(),
                label: "Approve".to_string(),
                style: InquiryResponseOptionStyle::Positive,
                response: json!({"approved": true}),
            }],
            assigned_to: Some(assignee.id),
            timeout_seconds: None,
        },
    )
    .await?;
    drop(conn);

    let detail = ctx
        .get(
            &format!("/api/v1/inquiries/{}", inquiry.id),
            Some(&viewer_token),
        )
        .await?;
    assert_eq!(detail.status(), StatusCode::OK);
    let body: serde_json::Value = detail.json().await?;
    assert_eq!(body["data"]["created_by_action_ref"], action.r#ref);
    assert_eq!(body["data"]["workflow_execution"], workflow_execution);
    assert_eq!(
        body["data"]["workflow_root_execution"],
        workflow_root_execution
    );
    assert_eq!(body["data"]["workflow_action_ref"], action.r#ref);
    assert_eq!(body["data"]["assigned_to_login"], assignee.login);
    assert_eq!(
        body["data"]["assigned_to_display_name"],
        assignee.display_name.unwrap()
    );

    let filtered = ctx
        .get(
            &format!("/api/v1/inquiries?workflow_action_ref={}", action.r#ref),
            Some(&viewer_token),
        )
        .await?;
    assert_eq!(filtered.status(), StatusCode::OK);
    let body: serde_json::Value = filtered.json().await?;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], inquiry.id);
    assert_eq!(
        body["items"][0]["workflow_root_execution"],
        workflow_root_execution
    );

    let assignee_token = generate_access_token(assignee.id, &assignee.login, &jwt_config())?;
    let detail = ctx
        .get(
            &format!("/api/v1/inquiries/{}", inquiry.id),
            Some(&assignee_token),
        )
        .await?;
    assert_eq!(detail.status(), StatusCode::OK);
    let body: serde_json::Value = detail.json().await?;
    assert_eq!(body["data"]["created_by_execution"], 0);
    assert!(body["data"]["created_by_action_ref"].is_null());
    assert!(body["data"]["workflow_execution"].is_null());
    assert!(body["data"]["workflow_root_execution"].is_null());
    assert!(body["data"]["workflow_action_ref"].is_null());
    assert_eq!(body["data"]["assigned_to_login"], assignee.login);

    let filtered = ctx
        .get(
            &format!("/api/v1/inquiries?workflow_action_ref={}", action.r#ref),
            Some(&assignee_token),
        )
        .await?;
    assert_eq!(filtered.status(), StatusCode::FORBIDDEN);

    Ok(())
}

#[tokio::test]
async fn create_inquiry_returns_option_bound_handles_and_rejects_invalid_options() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let identity = create_identity(&ctx.pool, "inquiry_creator").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "creation").await?;
    let permission_set_ref = "test.inquiry_creator";
    let permission_set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: permission_set_ref.to_string(),
            pack: None,
            pack_ref: None,
            label: Some("Inquiry creator".to_string()),
            description: None,
            grants: json!([{"resource": "inquiries", "actions": ["create"]}]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: permission_set.id,
        },
    )
    .await?;
    let execution =
        create_workflow_child_execution(&ctx.pool, &action, identity.id, permission_set_ref)
            .await?;
    let token = generate_execution_token_with_permission_sets(
        identity.id,
        execution.id,
        &action.r#ref,
        &jwt_config(),
        Some(600),
        &[permission_set_ref.to_string()],
    )?;

    let response = ctx
        .post(
            "/api/v1/inquiries",
            json!({
                "purpose": "approval",
                "prompt": "Approve deployment?",
                "response_schema": {
                    "approved": {"type": "boolean", "required": true}
                },
                "response_options": [
                    {
                        "ref": "approve",
                        "label": "Approve",
                        "style": "positive",
                        "response": {"approved": true}
                    },
                    {
                        "ref": "reject",
                        "label": "Reject",
                        "style": "destructive",
                        "response": {"approved": false}
                    }
                ],
                "assigned_to": identity.id,
                "timeout_seconds": 300
            }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response.headers()[axum::http::header::CACHE_CONTROL],
        "no-store"
    );
    let body: serde_json::Value = response.json().await?;
    let options = body["data"]["response_options"].as_array().unwrap();
    assert_eq!(options[0]["ref"], "approve");
    assert_eq!(options[1]["ref"], "reject");
    let approve_handle = options[0]["response_handle"].as_str().unwrap();
    let reject_handle = options[1]["response_handle"].as_str().unwrap();
    assert_ne!(approve_handle, reject_handle);
    assert!(approve_handle.len() <= 96);
    assert!(reject_handle.len() <= 96);

    for (purpose, options) in [
        (
            "duplicate-options",
            json!([
                {"ref": "approve", "label": "Approve", "style": "positive", "response": {"approved": true}},
                {"ref": "approve", "label": "Again", "style": "default", "response": {"approved": false}}
            ]),
        ),
        (
            "schema-invalid-option",
            json!([
                {"ref": "approve", "label": "Approve", "style": "positive", "response": {"approved": "yes"}}
            ]),
        ),
    ] {
        let invalid = ctx
            .post(
                "/api/v1/inquiries",
                json!({
                    "purpose": purpose,
                    "prompt": "Approve deployment?",
                    "response_schema": {
                        "approved": {"type": "boolean", "required": true}
                    },
                    "response_options": options
                }),
                Some(&token),
            )
            .await?;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    }

    let raw_schema = ctx
        .post(
            "/api/v1/inquiries",
            json!({
                "purpose": "raw-json-schema",
                "prompt": "Approve deployment?",
                "response_schema": {
                    "type": "object",
                    "properties": {"approved": {"type": "boolean"}}
                },
                "response_options": [{
                    "ref": "approve",
                    "label": "Approve",
                    "style": "positive",
                    "response": {"approved": true}
                }]
            }),
            Some(&token),
        )
        .await?;
    assert_eq!(raw_schema.status(), StatusCode::BAD_REQUEST);

    ctx.cleanup().await?;
    Ok(())
}

struct CallbackResponseFixture {
    integration_identity_id: i64,
    mapped_identity_id: i64,
    permission_assignment_id: i64,
    inquiry_id: i64,
    delivery_id: i64,
    sensor_id: i64,
    workload_fence: SensorWorkloadFence,
    sensor_ref: String,
    response_handle: String,
}

const CALLBACK_ADAPTER_REF: &str = "slack.socket_mode";

fn callback_sensor_config(enabled: bool) -> serde_json::Value {
    json!({
        "inquiry_callback_adapters": {
            "slack.socket_mode": {
                "enabled": enabled,
                "provider": "slack",
                "subject_kind": "user",
                "request": {
                    "delivery_id_pointer": "/envelope_id",
                    "tenant_pointer": "/payload/team/id",
                    "external_subject_pointer": "/payload/user/id",
                    "response_handle_pointer": "/payload/actions/0/value",
                    "required_values": {
                        "/type": "interactive",
                        "/payload/type": "block_actions"
                    },
                    "allowed_values": {
                        "/payload/actions/0/action_id": [
                            "attune.inquiry.response.v1.approve",
                            "attune.inquiry.response.v1.reject"
                        ]
                    },
                    "required_array_lengths": {"/payload/actions": 1}
                }
            }
        }
    })
}

async fn setup_callback_response_fixture(
    ctx: &TestContext,
    suffix: &str,
) -> TResult<CallbackResponseFixture> {
    let mapped = create_identity(&ctx.pool, &format!("callback_mapped_{suffix}")).await?;
    let (pack, action) = setup_pack_action(&ctx.pool, &format!("callback_{suffix}")).await?;
    let runtime = RuntimeRepository::create(
        &ctx.pool,
        CreateRuntimeInput {
            r#ref: format!("{}.callback_runtime", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            description: None,
            name: "Callback runtime".to_string(),
            aliases: Vec::new(),
            distributions: json!({}),
            installation: None,
            execution_config: json!({}),
            auto_detected: false,
            detection_config: json!({}),
        },
    )
    .await?;
    let sensor_ref = format!("{}.slack", pack.r#ref);
    let sensor = SensorRepository::create(
        &ctx.pool,
        CreateSensorInput {
            r#ref: sensor_ref.clone(),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: "Slack callback".to_string(),
            description: None,
            entrypoint: "slack.py".to_string(),
            runtime: runtime.id,
            runtime_ref: runtime.r#ref.clone(),
            runtime_version_constraint: None,
            enabled: true,
            param_schema: None,
            config: Some(callback_sensor_config(true)),
            worker_selector: json!({}),
            worker_tolerations: json!({}),
            worker_affinity: json!({}),
            log_retention_policy: None,
            log_retention_limit: None,
            artifact_retention_policy: None,
            artifact_retention_limit: None,
        },
    )
    .await?;
    activate_test_pack_release_with_projections(
        &ctx.pool,
        &pack,
        &attune_common::repositories::component_lifecycle::PackProjectionIds {
            runtimes: vec![runtime.id],
            sensors: vec![sensor.id],
            ..Default::default()
        },
    )
    .await?;
    let integration = create_identity(&ctx.pool, &format!("sensor:{sensor_ref}")).await?;
    let execution = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, execution.id, Some(mapped.id)).await?;
    let permission_set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("test.callback_response_{suffix}"),
            pack: None,
            pack_ref: None,
            label: Some("Callback inquiry response".to_string()),
            description: None,
            grants: json!([{"resource": "inquiries", "actions": ["respond"]}]),
        },
    )
    .await?;
    let permission_assignment = PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: integration.id,
            permset: permission_set.id,
        },
    )
    .await?;
    ExternalIdentityMappingRepository::create(
        &ctx.pool,
        integration.id,
        CreateExternalIdentityMappingInput {
            mapped_identity: mapped.id,
            provider: "slack".to_string(),
            tenant: "T123".to_string(),
            subject_kind: "user".to_string(),
            external_subject: "U456".to_string(),
            created_by: Some(integration.id),
        },
    )
    .await?;
    let response_handle = issue_inquiry_response_handle(
        inquiry.id,
        0,
        ctx.state
            .config
            .security
            .encryption_key
            .as_deref()
            .expect("test encryption key"),
    )?;
    let worker = WorkerRepository::create(
        &ctx.pool,
        CreateWorkerInput {
            name: format!("callback-worker-{suffix}"),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: Some(json!({})),
            meta: None,
        },
    )
    .await?;
    let worker_instance = uuid::Uuid::new_v4();
    let lease = match SensorWorkloadRepository::acquire_or_renew(
        &ctx.pool,
        AcquireSensorWorkloadInput {
            sensor_id: sensor.id,
            worker_id: worker.id,
            worker_instance,
            lease_seconds: 300,
        },
    )
    .await?
    {
        AcquireSensorWorkloadOutcome::Acquired(lease) => lease,
        AcquireSensorWorkloadOutcome::HeldByOther(_) => unreachable!(),
    };
    let owned = SensorWorkloadRepository::begin_process(&ctx.pool, lease)
        .await?
        .expect("owned callback workload");
    let encrypted_payload = encrypt_json(
        &json!({
            "provider": "slack",
            "subject_kind": "user",
            "response_handle": response_handle,
            "tenant": "T123",
            "external_subject": "U456"
        }),
        ctx.state.config.security.encryption_key.as_deref().unwrap(),
    )?;
    let mut tx = ctx.pool.begin().await?;
    let delivery = InquiryCallbackDeliveryRepository::insert_or_load(
        &mut tx,
        CreateInquiryCallbackDelivery {
            sensor: sensor.id,
            integration_identity: integration.id,
            workload: owned.workload_id,
            assignment_generation: owned.generation,
            pack_release: owned.pack_release.unwrap(),
            pack_release_digest: owned.pack_release_digest.as_deref().unwrap(),
            adapter_ref: CALLBACK_ADAPTER_REF,
            provider_delivery_id: &format!("env-{suffix}"),
            request_digest: &format!("sha256:{}", "a".repeat(64)),
            encrypted_payload: &encrypted_payload,
        },
    )
    .await?;
    tx.commit().await?;
    let delivery_id = match delivery {
        InsertInquiryCallbackDelivery::Inserted(delivery) => delivery.id,
        _ => unreachable!(),
    };
    Ok(CallbackResponseFixture {
        integration_identity_id: integration.id,
        mapped_identity_id: mapped.id,
        permission_assignment_id: permission_assignment.id,
        inquiry_id: inquiry.id,
        delivery_id,
        sensor_id: sensor.id,
        workload_fence: owned.fence(),
        sensor_ref,
        response_handle,
    })
}

fn callback_submission(
    fixture: &CallbackResponseFixture,
) -> attune_api::inquiry_response::InquiryResponseSubmission {
    attune_api::inquiry_response::InquiryResponseSubmission::CallbackAdapter {
        delivery_id: fixture.delivery_id,
    }
}

fn callback_sensor_token(fixture: &CallbackResponseFixture) -> TResult<String> {
    Ok(generate_sensor_token_with_workload_fence(
        fixture.integration_identity_id,
        &fixture.sensor_ref,
        Vec::new(),
        fixture.workload_fence,
        &jwt_config(),
        Some(600),
    )?)
}

#[tokio::test]
async fn pending_callback_delivery_is_owned_by_api_after_durable_ingress() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let fixture = setup_callback_response_fixture(&ctx, "socket_restart").await?;

    assert!(SensorWorkloadRepository::release(&ctx.pool, fixture.workload_fence).await?);
    let lease = match SensorWorkloadRepository::acquire_or_renew(
        &ctx.pool,
        AcquireSensorWorkloadInput {
            sensor_id: fixture.sensor_id,
            worker_id: fixture.workload_fence.worker_id,
            worker_instance: fixture.workload_fence.worker_instance,
            lease_seconds: 300,
        },
    )
    .await?
    {
        AcquireSensorWorkloadOutcome::Acquired(lease) => lease,
        AcquireSensorWorkloadOutcome::HeldByOther(_) => unreachable!(),
    };
    let restarted = SensorWorkloadRepository::begin_process(&ctx.pool, lease)
        .await?
        .expect("restarted callback workload");
    assert!(restarted.generation > fixture.workload_fence.generation);
    assert!(SensorWorkloadRepository::release(&ctx.pool, restarted.fence()).await?);
    attune_api::inquiry_response::process_inquiry_callback_delivery(
        &ctx.state,
        fixture.delivery_id,
    )
    .await?;
    let updated = InquiryRepository::find_by_id(&ctx.pool, fixture.inquiry_id)
        .await?
        .unwrap();

    assert_eq!(updated.id, fixture.inquiry_id);
    assert_eq!(updated.response, Some(json!({"approved": true})));
    Ok(())
}

fn slack_socket_body(fixture: &CallbackResponseFixture, envelope_id: &str) -> serde_json::Value {
    json!({
        "envelope_id": envelope_id,
        "type": "interactive",
        "accepts_response_payload": true,
        "payload": {
            "type": "block_actions",
            "team": {"id": "T123"},
            "user": {"id": "U456"},
            "actions": [{
                "action_id": "attune.inquiry.response.v1.approve",
                "value": fixture.response_handle
            }]
        }
    })
}

#[tokio::test]
async fn metadata_callback_ingress_rejects_non_sensor_stale_fence_and_invalid_shape() -> TResult<()>
{
    let ctx = TestContext::new().await?;
    let fixture = setup_callback_response_fixture(&ctx, "socket_rejections").await?;
    let body = slack_socket_body(&fixture, "env-rejections");

    let access_token = generate_access_token(
        fixture.integration_identity_id,
        &format!("sensor:{}", fixture.sensor_ref),
        &jwt_config(),
    )?;
    let response = ctx
        .post(
            "/api/v1/internal/inquiry-callbacks/slack.socket_mode",
            body.clone(),
            Some(&access_token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let mut stale_fence = fixture.workload_fence;
    stale_fence.generation += 1;
    let stale_token = generate_sensor_token_with_workload_fence(
        fixture.integration_identity_id,
        &fixture.sensor_ref,
        Vec::new(),
        stale_fence,
        &jwt_config(),
        Some(600),
    )?;
    let response = ctx
        .post(
            "/api/v1/internal/inquiry-callbacks/slack.socket_mode",
            body.clone(),
            Some(&stale_token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let mut invalid = body;
    invalid["payload"]["actions"][0]["action_id"] = json!("untrusted.action");
    let response = ctx
        .post(
            "/api/v1/internal/inquiry-callbacks/slack.socket_mode",
            invalid,
            Some(&callback_sensor_token(&fixture)?),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn metadata_callback_ingress_ack_does_not_wait_for_response_processing() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let fixture = setup_callback_response_fixture(&ctx, "socket_ack_deadline").await?;
    let body = slack_socket_body(&fixture, "env-ack-deadline");
    let workload =
        SensorWorkloadRepository::find_by_id(&ctx.pool, fixture.workload_fence.workload_id)
            .await?
            .unwrap();
    let encrypted_payload = encrypt_json(
        &json!({
            "provider": "slack",
            "subject_kind": "user",
            "response_handle": fixture.response_handle,
            "tenant": "T123",
            "external_subject": "U456"
        }),
        ctx.state.config.security.encryption_key.as_deref().unwrap(),
    )?;
    let digest = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&body)?))
    );
    let mut tx = ctx.pool.begin().await?;
    let delivery = InquiryCallbackDeliveryRepository::insert_or_load(
        &mut tx,
        CreateInquiryCallbackDelivery {
            sensor: fixture.sensor_id,
            integration_identity: fixture.integration_identity_id,
            workload: workload.id,
            assignment_generation: fixture.workload_fence.generation,
            pack_release: workload.pack_release.unwrap(),
            pack_release_digest: workload.pack_release_digest.as_deref().unwrap(),
            adapter_ref: CALLBACK_ADAPTER_REF,
            provider_delivery_id: "env-ack-deadline",
            request_digest: &digest,
            encrypted_payload: &encrypted_payload,
        },
    )
    .await?;
    tx.commit().await?;
    let delivery_id = match delivery {
        InsertInquiryCallbackDelivery::Inserted(delivery) => delivery.id,
        _ => unreachable!(),
    };

    let mut processor = ctx.pool.begin().await?;
    SensorAdmissionRepository::lock_workload_checks(&mut processor).await?;
    SensorRepository::find_by_id_for_update(&mut processor, fixture.sensor_id)
        .await?
        .expect("sensor exists");
    InquiryCallbackDeliveryRepository::find_by_id_for_update(&mut processor, delivery_id)
        .await?
        .expect("delivery exists");

    let response = tokio::time::timeout(
        Duration::from_secs(2),
        ctx.post(
            "/api/v1/internal/inquiry-callbacks/slack.socket_mode",
            body,
            Some(&callback_sensor_token(&fixture)?),
        ),
    )
    .await??;
    processor.rollback().await?;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await?,
        json!({"acknowledge": true})
    );
    Ok(())
}

#[tokio::test]
async fn metadata_callback_ingress_maps_assignee_acks_duplicates_and_rejects_conflicting_replay(
) -> TResult<()> {
    let ctx = TestContext::new().await?;
    let fixture = setup_callback_response_fixture(&ctx, "socket_success").await?;
    let token = callback_sensor_token(&fixture)?;
    let body = slack_socket_body(&fixture, "env-socket-success");
    let workload =
        SensorWorkloadRepository::find_by_id(&ctx.pool, fixture.workload_fence.workload_id)
            .await?
            .unwrap();
    let encrypted_payload = encrypt_json(
        &json!({
            "provider": "slack",
            "subject_kind": "user",
            "response_handle": fixture.response_handle,
            "tenant": "T123",
            "external_subject": "U456"
        }),
        ctx.state.config.security.encryption_key.as_deref().unwrap(),
    )?;
    let digest = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&body)?))
    );
    let mut tx = ctx.pool.begin().await?;
    let pending = InquiryCallbackDeliveryRepository::insert_or_load(
        &mut tx,
        CreateInquiryCallbackDelivery {
            sensor: fixture.sensor_id,
            integration_identity: fixture.integration_identity_id,
            workload: workload.id,
            assignment_generation: fixture.workload_fence.generation,
            pack_release: workload.pack_release.unwrap(),
            pack_release_digest: workload.pack_release_digest.as_deref().unwrap(),
            adapter_ref: CALLBACK_ADAPTER_REF,
            provider_delivery_id: "env-socket-success",
            request_digest: &digest,
            encrypted_payload: &encrypted_payload,
        },
    )
    .await?;
    assert!(matches!(
        pending,
        InsertInquiryCallbackDelivery::Inserted(_)
    ));
    tx.commit().await?;

    let response = ctx
        .post(
            "/api/v1/internal/inquiry-callbacks/slack.socket_mode",
            body.clone(),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await?,
        json!({"acknowledge": true})
    );
    let delivery = InquiryCallbackDeliveryRepository::find_by_sensor_adapter_provider_id(
        &ctx.pool,
        fixture.sensor_id,
        CALLBACK_ADAPTER_REF,
        "env-socket-success",
    )
    .await?
    .unwrap();
    attune_api::inquiry_response::process_inquiry_callback_delivery(&ctx.state, delivery.id)
        .await?;
    let inquiry = InquiryRepository::find_by_id(&ctx.pool, fixture.inquiry_id)
        .await?
        .unwrap();
    assert_eq!(inquiry.responded_by, Some(fixture.mapped_identity_id));
    assert_eq!(inquiry.response, Some(json!({"approved": true})));

    let duplicate = ctx
        .post(
            "/api/v1/internal/inquiry-callbacks/slack.socket_mode",
            body.clone(),
            Some(&token),
        )
        .await?;
    assert_eq!(duplicate.status(), StatusCode::OK);

    let mut conflicting = body;
    conflicting["accepts_response_payload"] = json!(false);
    let replay = ctx
        .post(
            "/api/v1/internal/inquiry-callbacks/slack.socket_mode",
            conflicting,
            Some(&token),
        )
        .await?;
    assert_eq!(replay.status(), StatusCode::CONFLICT);

    let delivery = InquiryCallbackDeliveryRepository::find_by_sensor_adapter_provider_id(
        &ctx.pool,
        fixture.sensor_id,
        CALLBACK_ADAPTER_REF,
        "env-socket-success",
    )
    .await?
    .unwrap();
    assert_eq!(delivery.state, "accepted");
    assert_eq!(delivery.inquiry, Some(fixture.inquiry_id));
    Ok(())
}

#[tokio::test]
async fn metadata_callback_ingress_rejects_disabled_adapter_and_acks_revoked_permission(
) -> TResult<()> {
    let ctx = TestContext::new().await?;
    let disabled = setup_callback_response_fixture(&ctx, "socket_disabled").await?;
    SensorRepository::update(
        &ctx.pool,
        disabled.sensor_id,
        UpdateSensorInput {
            config: Some(callback_sensor_config(false)),
            ..Default::default()
        },
    )
    .await?;
    let response = ctx
        .post(
            "/api/v1/internal/inquiry-callbacks/slack.socket_mode",
            slack_socket_body(&disabled, "env-disabled"),
            Some(&callback_sensor_token(&disabled)?),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let revoked = setup_callback_response_fixture(&ctx, "socket_revoked").await?;
    PermissionAssignmentRepository::delete(&ctx.pool, revoked.permission_assignment_id).await?;
    let response = ctx
        .post(
            "/api/v1/internal/inquiry-callbacks/slack.socket_mode",
            slack_socket_body(&revoked, "env-revoked"),
            Some(&callback_sensor_token(&revoked)?),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await?,
        json!({"acknowledge": true})
    );
    let delivery = InquiryCallbackDeliveryRepository::find_by_sensor_adapter_provider_id(
        &ctx.pool,
        revoked.sensor_id,
        CALLBACK_ADAPTER_REF,
        "env-revoked",
    )
    .await?
    .unwrap();
    attune_api::inquiry_response::process_inquiry_callback_delivery(&ctx.state, delivery.id)
        .await?;
    let delivery = InquiryCallbackDeliveryRepository::find_by_sensor_adapter_provider_id(
        &ctx.pool,
        revoked.sensor_id,
        CALLBACK_ADAPTER_REF,
        "env-revoked",
    )
    .await?
    .unwrap();
    assert_eq!(delivery.state, "rejected");
    assert_eq!(delivery.rejection_code.as_deref(), Some("forbidden"));
    assert_eq!(
        InquiryRepository::find_by_id(&ctx.pool, revoked.inquiry_id)
            .await?
            .unwrap()
            .status,
        InquiryStatus::Pending
    );
    Ok(())
}

#[tokio::test]
async fn callback_delivery_history_does_not_block_sensor_deletion() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let fixture = setup_callback_response_fixture(&ctx, "socket_sensor_delete").await?;

    assert!(SensorRepository::delete(&ctx.pool, fixture.sensor_id).await?);
    let delivery = InquiryCallbackDeliveryRepository::find_by_id(&ctx.pool, fixture.delivery_id)
        .await?
        .expect("delivery history remains after sensor deletion");
    assert_eq!(delivery.sensor, fixture.sensor_id);
    Ok(())
}

#[tokio::test]
async fn callback_response_resolves_actor_and_uses_bound_option() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let fixture = setup_callback_response_fixture(&ctx, "success").await?;

    let updated = attune_api::inquiry_response::submit_inquiry_response(
        &ctx.state,
        callback_submission(&fixture),
    )
    .await?;
    assert_eq!(updated.response, Some(json!({"approved": true})));
    assert_eq!(updated.responded_by, Some(fixture.mapped_identity_id));
    assert_eq!(
        updated.external_actor,
        Some(json!({
            "provider": "slack",
            "subject_kind": "user",
            "mapping_id": updated.external_actor.as_ref().unwrap()["mapping_id"],
            "integration_identity_id": fixture.integration_identity_id,
            "delivery_id": fixture.delivery_id,
        }))
    );

    let duplicate = attune_api::inquiry_response::submit_inquiry_response(
        &ctx.state,
        callback_submission(&fixture),
    )
    .await;
    assert!(matches!(
        duplicate,
        Err(attune_api::middleware::ApiError::Conflict(_))
    ));
    Ok(())
}

#[tokio::test]
async fn callback_response_uses_current_database_authorization() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let fixture = setup_callback_response_fixture(&ctx, "revoked_rbac").await?;
    assert!(
        PermissionAssignmentRepository::delete(&ctx.pool, fixture.permission_assignment_id).await?
    );

    let result = attune_api::inquiry_response::submit_inquiry_response(
        &ctx.state,
        callback_submission(&fixture),
    )
    .await;
    assert!(matches!(
        result,
        Err(attune_api::middleware::ApiError::Forbidden(_))
    ));
    assert_eq!(
        InquiryRepository::find_by_id(&ctx.pool, fixture.inquiry_id)
            .await?
            .expect("inquiry exists")
            .status,
        InquiryStatus::Pending
    );
    Ok(())
}

#[tokio::test]
async fn assignee_with_access_token_can_respond() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_ok").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "ok").await?;
    let exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, exec.id, Some(assignee.id)).await?;

    let token = generate_access_token(assignee.id, &assignee.login, &cfg)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn non_assignee_access_token_is_forbidden() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_real").await?;
    let other = create_identity(&ctx.pool, "other_user").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "non_assignee").await?;
    let exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, exec.id, Some(assignee.id)).await?;

    let token = generate_access_token(other.id, &other.login, &cfg)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn execution_token_self_response_is_blocked() -> TResult<()> {
    // The action that *created* the inquiry must not be allowed to respond to
    // it using its own execution-scoped token, even if the triggering
    // identity happens to be the assignee.
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_self").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "self").await?;
    let exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, exec.id, Some(assignee.id)).await?;

    // Execution token for the SAME execution that created the inquiry,
    // carrying the assignee identity in `sub`.
    let token = generate_execution_token(assignee.id, exec.id, &action.r#ref, &cfg, None)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body: serde_json::Value = resp.json().await?;
    let msg = body["error"].as_str().unwrap_or("");
    assert!(
        msg.contains("privilege loop") || msg.contains("cannot respond"),
        "unexpected error message: {}",
        msg
    );
    Ok(())
}

#[tokio::test]
async fn execution_token_for_different_execution_can_respond_when_assignee() -> TResult<()> {
    // An execution token for a *different* execution (e.g., a Slack-bridge
    // action handling the user's webhook reply) is allowed when its
    // identity matches the assignee.
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_other_exec").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "other_exec").await?;
    let creating_exec = create_execution(&ctx.pool, &action).await?;
    let other_exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, creating_exec.id, Some(assignee.id)).await?;

    let token = generate_execution_token(assignee.id, other_exec.id, &action.r#ref, &cfg, None)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn execution_token_for_different_execution_blocked_when_not_assignee() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_diff_id").await?;
    let other = create_identity(&ctx.pool, "non_assignee_id").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "exec_non_assignee").await?;
    let creating_exec = create_execution(&ctx.pool, &action).await?;
    let other_exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, creating_exec.id, Some(assignee.id)).await?;

    let token = generate_execution_token(other.id, other_exec.id, &action.r#ref, &cfg, None)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn unassigned_inquiry_accepts_any_authenticated_caller() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let caller = create_identity(&ctx.pool, "any_caller").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "unassigned").await?;
    let exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, exec.id, None).await?;

    let token = generate_access_token(caller.id, &caller.login, &cfg)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn nested_execution_token_self_response_is_blocked() -> TResult<()> {
    // A creates an inquiry; B is a child of A. A token scoped to B must not
    // be allowed to respond to A's inquiry — that's still a self-approval
    // loop in spirit (the workflow that created the inquiry can't approve
    // it via one of its own descendants).
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_nested").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "nested").await?;
    let exec_a = create_execution(&ctx.pool, &action).await?;
    let exec_b = create_child_execution(&ctx.pool, &action, exec_a.id).await?;
    let inquiry = create_inquiry(&ctx.pool, exec_a.id, Some(assignee.id)).await?;

    // Execution token scoped to the *child* execution B, carrying the
    // assignee identity.
    let token = generate_execution_token(assignee.id, exec_b.id, &action.r#ref, &cfg, None)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body: serde_json::Value = resp.json().await?;
    let msg = body["error"].as_str().unwrap_or("");
    assert!(
        msg.contains("descendant") || msg.contains("privilege loop"),
        "unexpected error message: {}",
        msg
    );
    Ok(())
}

#[tokio::test]
async fn deeply_nested_execution_token_self_response_is_blocked() -> TResult<()> {
    // A → B → C: token scoped to C must not be able to respond to A's inquiry.
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_deep").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "deep").await?;
    let exec_a = create_execution(&ctx.pool, &action).await?;
    let exec_b = create_child_execution(&ctx.pool, &action, exec_a.id).await?;
    let exec_c = create_child_execution(&ctx.pool, &action, exec_b.id).await?;
    let inquiry = create_inquiry(&ctx.pool, exec_a.id, Some(assignee.id)).await?;

    let token = generate_execution_token(assignee.id, exec_c.id, &action.r#ref, &cfg, None)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn responded_by_recorded_for_access_token() -> TResult<()> {
    // After a successful response, the inquiry row should reflect the
    // assignee's identity in `responded_at` (set non-null) and the response
    // should be persisted. We assert via DB state since the MQ event is
    // best-effort (publisher may be absent in the test harness).
    let ctx = TestContext::new().await?;
    let cfg = jwt_config();

    let assignee = create_identity(&ctx.pool, "assignee_audit").await?;
    let (_pack, action) = setup_pack_action(&ctx.pool, "audit").await?;
    let exec = create_execution(&ctx.pool, &action).await?;
    let inquiry = create_inquiry(&ctx.pool, exec.id, Some(assignee.id)).await?;

    let token = generate_access_token(assignee.id, &assignee.login, &cfg)?;

    let resp = ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", inquiry.id),
            respond_body(),
            Some(&token),
        )
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);

    let stored = InquiryRepository::find_by_id(&ctx.pool, inquiry.id)
        .await?
        .expect("inquiry should exist");
    assert!(stored.responded_at.is_some(), "responded_at should be set");
    assert_eq!(
        stored.status,
        attune_common::models::enums::InquiryStatus::Responded
    );
    assert_eq!(stored.responded_by, Some(assignee.id));
    assert_eq!(stored.response, Some(json!({ "approved": true })));
    Ok(())
}
