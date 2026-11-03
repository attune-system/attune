//! Cursor contract and selected-run summaries through authenticated real routes.
mod helpers;

use attune_common::{
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{
            ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
            CreateArtifactVersionInput,
        },
        execution::{CreateExecutionInput, ExecutionRepository},
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        Create, Delete,
    },
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::{json, Value};

#[tokio::test]
async fn artifact_cursors_use_the_effective_signing_key_not_optional_config_input() -> Result<()> {
    use std::sync::Arc;

    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;

    let ctx = TestContext::new().await?.with_auth().await?;
    for index in 0..2 {
        ArtifactRepository::create(
            &ctx.pool,
            CreateArtifactInput {
                r#ref: format!("cursor.effective_key{index}"),
                scope: OwnerType::System,
                owner: String::new(),
                r#type: ArtifactType::Other,
                visibility: ArtifactVisibility::Public,
                classification: ArtifactClassification::General,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 1,
                name: None,
                description: None,
                content_type: None,
                data: None,
            },
        )
        .await?;
    }
    let mut config = ctx.state.config.as_ref().clone();
    config.security.jwt_secret = None;
    // Hosts can supply an effective JWT configuration independently of optional
    // config input. Cursors must use the same actual key as authentication.
    let mut state = attune_api::state::AppState::new_with_audit(
        ctx.pool.clone(),
        config,
        ctx.state.audit_emitter.clone(),
    );
    state.jwt_config = ctx.state.jwt_config.clone();
    let app = attune_api::server::Server::new(Arc::new(state)).router();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/artifacts?per_page=1")
                .header("authorization", format!("Bearer {}", ctx.token().unwrap()))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await?)?;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    let cursor = body["next_cursor"].as_str().ok_or("continuation missing")?;
    let next = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/artifacts?per_page=1&cursor={cursor}"))
                .header("authorization", format!("Bearer {}", ctx.token().unwrap()))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(next.status(), StatusCode::OK);
    let next: Value = serde_json::from_slice(&to_bytes(next.into_body(), 4096).await?)?;
    assert_ne!(next["items"][0]["id"], body["items"][0]["id"]);
    drop(app);
    ctx.cleanup().await
}

#[tokio::test]
async fn artifact_cursor_routes_select_exact_runs_and_bind_continuations() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    let identity = ctx.user.as_ref().ok_or("fixture identity missing")?.id;
    let permission = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "cursor.reader".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([
                {"resource": "artifacts", "actions": ["read"]},
                {"resource": "executions", "actions": ["read"]}
            ]),
        },
    )
    .await?;
    let assignment = PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: permission.id,
        },
    )
    .await?;
    let execution = ExecutionRepository::create(
        &ctx.pool,
        CreateExecutionInput {
            action_ref: "cursor.action".into(),
            executor: Some(identity),
            ..Default::default()
        },
    )
    .await?;
    let unrelated = ExecutionRepository::create(
        &ctx.pool,
        CreateExecutionInput {
            action_ref: "cursor.action".into(),
            executor: Some(identity),
            ..Default::default()
        },
    )
    .await?;
    let mut selected_id = 0;
    let mut artifact_ids = Vec::new();
    for index in 0..2 {
        let artifact = ArtifactRepository::create(
            &ctx.pool,
            CreateArtifactInput {
                r#ref: format!("cursor.artifact{index}"),
                scope: OwnerType::Identity,
                owner: identity.to_string(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::RuntimeLog,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 1000,
                name: Some("cursor fixture".into()),
                description: Some("private description".into()),
                content_type: None,
                data: Some(json!({"private": true})),
            },
        )
        .await?;
        artifact_ids.push(artifact.id);
        for (run, size) in [(execution.id, 2), (execution.id, 3), (unrelated.id, 9)] {
            let version = ArtifactVersionRepository::create(
                &ctx.pool,
                CreateArtifactVersionInput {
                    artifact: artifact.id,
                    execution: Some(run),
                    content_type: Some(format!("fixture/type{size}")),
                    content: Some(vec![b'x'; size]),
                    content_json: None,
                    file_path: Some(format!("fixture/{index}/{size}.txt")),
                    meta: Some(json!({"private": true})),
                    created_by: Some("private creator".into()),
                },
            )
            .await?;
            if index == 1 && size == 3 {
                selected_id = version.id;
            }
        }
    }
    let path = format!("/api/v1/executions/{}/artifacts?per_page=1", execution.id);
    let response = ctx.get(&path, ctx.token()).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let first: Value = response.json().await?;
    assert_eq!(first["items"][0]["selected_version"]["id"], selected_id);
    assert_eq!(
        first["items"][0]["selected_version"]["execution"],
        execution.id
    );
    assert_eq!(first["items"][0]["selected_version"]["version"], 2);
    assert_eq!(first["items"][0]["size_bytes"], 3);
    assert_eq!(first["items"][0]["content_type"], "fixture/type3");
    assert_eq!(first["has_more"], true);
    assert!(first.get("total").is_none());
    let cursor = first["next_cursor"]
        .as_str()
        .ok_or("missing continuation")?;
    let next: Value = ctx
        .get(&format!("{path}&cursor={cursor}"), ctx.token())
        .await?
        .json()
        .await?;
    assert_eq!(next["items"].as_array().unwrap().len(), 1);
    assert_ne!(first["items"][0]["id"], next["items"][0]["id"]);
    assert_eq!(next["has_more"], false);
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts?cursor={cursor}"), ctx.token())
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let global: Value = ctx
        .get("/api/v1/artifacts?name=cursor&per_page=1", ctx.token())
        .await?
        .json()
        .await?;
    assert!(global["items"][0]["selected_version"].is_null());
    assert!(global["items"][0]["content_type"].is_null());
    assert!(global["items"][0]["size_bytes"].is_null());
    let cursor = global["next_cursor"]
        .as_str()
        .ok_or("missing global continuation")?;
    let global_cursor = cursor.to_owned();
    assert_eq!(
        ctx.get(
            &format!("/api/v1/artifacts?name=changed&cursor={cursor}"),
            ctx.token()
        )
        .await?
        .status(),
        StatusCode::BAD_REQUEST
    );
    let history = format!("/api/v1/artifacts/{}/versions?per_page=1", artifact_ids[1]);
    let versions: Value = ctx.get(&history, ctx.token()).await?.json().await?;
    assert_eq!(versions["items"][0]["version"], 3);
    assert!(versions["items"][0].get("file_path").is_none());
    assert!(versions["items"][0].get("meta").is_none());
    let cursor = versions["next_cursor"]
        .as_str()
        .ok_or("missing version continuation")?;
    let next: Value = ctx
        .get(&format!("{history}&cursor={cursor}"), ctx.token())
        .await?
        .json()
        .await?;
    assert_eq!(next["items"][0]["version"], 2);
    for invalid in ["per_page=0", "per_page=101", "cursor=invalid", "page=2"] {
        assert_eq!(
            ctx.get(&format!("/api/v1/artifacts?{invalid}"), ctx.token())
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    PermissionAssignmentRepository::delete(&ctx.pool, assignment.id).await?;
    let revoked: Value = ctx
        .get(
            &format!("/api/v1/artifacts?name=cursor&cursor={global_cursor}"),
            ctx.token(),
        )
        .await?
        .json()
        .await?;
    assert!(revoked["items"].as_array().unwrap().is_empty());
    ctx.cleanup().await?;
    Ok(())
}
