mod helpers;

use attune_api::authz::AuthorizationService;
use attune_common::{
    models::enums::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{ArtifactRepository, CreateArtifactInput},
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        runtime::{CreateRuntimeInput, RuntimeRepository},
        trigger::{CreateSensorInput, SensorRepository},
        Create,
    },
};
use axum::http::StatusCode;
use serde_json::{json, Value};

use helpers::TestContext;

#[tokio::test]
async fn sensor_log_summary_identifies_created_stream_artifacts() {
    let context = TestContext::new()
        .await
        .expect("create test context")
        .with_auth()
        .await
        .expect("authenticate test user");
    let identity = context.user.as_ref().expect("authenticated identity");
    let permission_set = PermissionSetRepository::create(
        &context.pool,
        CreatePermissionSetInput {
            r#ref: format!("sensor_log_test.reader_{}", identity.id),
            pack: None,
            pack_ref: None,
            label: Some("Sensor log reader".to_string()),
            description: None,
            grants: json!([{"resource": "triggers", "actions": ["update"]}]),
        },
    )
    .await
    .expect("create sensor permission set");
    PermissionAssignmentRepository::create(
        &context.pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: permission_set.id,
        },
    )
    .await
    .expect("assign sensor permission set");
    AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    AuthorizationService::invalidate_permission_set_caches().await;

    let runtime = RuntimeRepository::create(
        &context.pool,
        CreateRuntimeInput {
            r#ref: "sensor_log_test.python".to_string(),
            pack: None,
            pack_ref: None,
            description: None,
            name: "python".to_string(),
            aliases: Vec::new(),
            distributions: json!({}),
            installation: None,
            execution_config: json!({"interpreter": {"binary": "python3"}}),
            auto_detected: false,
            detection_config: json!({}),
        },
    )
    .await
    .expect("create sensor runtime");
    let sensor_ref = "sensor_log_test.socket_mode";
    SensorRepository::create(
        &context.pool,
        CreateSensorInput {
            r#ref: sensor_ref.to_string(),
            pack: None,
            pack_ref: None,
            label: "Socket Mode".to_string(),
            description: None,
            entrypoint: "socket_mode.py".to_string(),
            runtime: runtime.id,
            runtime_ref: runtime.r#ref,
            runtime_version_constraint: None,
            enabled: true,
            param_schema: None,
            config: None,
            worker_selector: json!({}),
            worker_tolerations: json!([]),
            worker_affinity: json!({}),
            log_retention_policy: None,
            log_retention_limit: None,
            artifact_retention_policy: None,
            artifact_retention_limit: None,
        },
    )
    .await
    .expect("create sensor");
    let stderr = ArtifactRepository::create(
        &context.pool,
        CreateArtifactInput {
            r#ref: format!("sensor.{sensor_ref}.stderr"),
            scope: OwnerType::Sensor,
            owner: sensor_ref.to_string(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::RuntimeLog,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 4,
            name: None,
            description: None,
            content_type: Some("text/plain".to_string()),
            data: None,
        },
    )
    .await
    .expect("create stderr artifact");

    let response = context
        .get(&format!("/api/v1/sensors/{sensor_ref}/logs"), None)
        .await
        .expect("request sensor log summary");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("parse sensor log summary");
    assert_eq!(
        body["logs"],
        json!([
            {
                "stream": "stdout",
                "artifact_ref": format!("sensor.{sensor_ref}.stdout"),
                "artifact_id": null
            },
            {
                "stream": "stderr",
                "artifact_ref": format!("sensor.{sensor_ref}.stderr"),
                "artifact_id": stderr.id
            }
        ])
    );
}
