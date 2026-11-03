//! Metadata visibility must never become a permit for protected content.
mod helpers;
// This action-only suite shares the admitted pack fixture with the sensor suite;
// its retained-generation helper is intentionally unused in this executable.
#[allow(dead_code)]
#[path = "artifact_read_performance/managed_fixture.rs"]
mod managed_fixture;

use attune_common::{
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{
            ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
            CreateArtifactVersionInput,
        },
        execution_secret_value::ExecutionSecretValueRepository,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        key::{CreateKeyInput, KeyRepository},
        Create,
    },
    secret_provenance::SecretProvenance,
    secret_values::{prepare_secret_values, SecretValueInput, ENTITY_EXECUTION_CONFIG},
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::{json, Value};

#[test]
fn content_openapi_exposes_optional_immutable_version_id() -> Result<()> {
    use utoipa::OpenApi;
    let spec = serde_json::to_value(attune_api::openapi::ApiDoc::openapi())?;
    for path in [
        "/api/v1/artifacts/{id}/versions/{version}",
        "/api/v1/artifacts/{id}/versions/latest",
        "/api/v1/artifacts/{id}/versions/{version}/download",
        "/api/v1/artifacts/{id}/download",
        "/api/v1/artifacts/{id}/stream",
    ] {
        let parameters = spec["paths"][path]["get"]["parameters"]
            .as_array()
            .ok_or("OpenAPI parameters missing")?;
        let selector = parameters
            .iter()
            .find(|parameter| parameter["name"] == "version_id")
            .ok_or("immutable selector missing")?;
        assert_eq!(selector["in"], "query", "{path}");
        assert_ne!(selector["required"], true, "{path}");
    }
    Ok(())
}

async fn grant_artifacts(ctx: &TestContext) -> Result<()> {
    let identity = ctx.user.as_ref().ok_or("fixture identity missing")?.id;
    let set = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "fixture.headers".into(), pack: None, pack_ref: None, label: None, description: None,
        grants: json!([
            {"resource":"artifacts","actions":["read"],"constraints":{"refs":["fixture.log","fixture.progress"]}},
            {"resource":"executions","actions":["read","decrypt"]},
            {"resource":"actions","actions":["read","execute"]}
        ]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    Ok(())
}

async fn artifact(
    ctx: &TestContext,
    reference: &str,
    classification: ArtifactClassification,
    kind: ArtifactType,
) -> Result<i64> {
    Ok(ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: reference.into(),
            scope: OwnerType::Action,
            owner: "artifact_perf.run".into(),
            r#type: kind,
            visibility: ArtifactVisibility::Private,
            classification,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 1000,
            name: Some("Safe header name".into()),
            description: Some("protected description".into()),
            content_type: Some("application/private-fixture".into()),
            data: Some(json!({"progress":"protected data"})),
        },
    )
    .await?
    .id)
}

async fn execution(ctx: &TestContext, action_ref: &str) -> Result<i64> {
    let response = ctx
        .post(
            "/api/v1/executions/execute",
            json!({
                "action_ref":action_ref, "parameters":{}, "permission_set_refs":[]
            }),
            ctx.token(),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"]["id"]
        .as_i64()
        .ok_or_else(|| "execution missing".into())
}

async fn version(
    ctx: &TestContext,
    artifact: i64,
    execution: Option<i64>,
    body: &str,
) -> Result<attune_common::models::ArtifactVersion> {
    Ok(ArtifactVersionRepository::create(
        &ctx.pool,
        CreateArtifactVersionInput {
            artifact,
            execution,
            content_type: Some("application/json".into()),
            content: None,
            content_json: Some(json!({"body":body})),
            file_path: None,
            meta: Some(json!({"private":"version metadata"})),
            created_by: None,
        },
    )
    .await?)
}

async fn assert_safe_header(ctx: &TestContext, id: i64) -> Result<()> {
    for path in [
        format!("/api/v1/artifacts/{id}"),
        "/api/v1/artifacts/ref/fixture.log".into(),
    ] {
        let response = ctx.get(&path, ctx.token()).await?;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body: Value = response.json().await?;
        let header = body["data"].as_object().ok_or("header missing")?;
        assert_eq!(header.len(), 12);
        for key in header.keys() {
            assert!(
                [
                    "id",
                    "ref",
                    "scope",
                    "owner",
                    "type",
                    "visibility",
                    "classification",
                    "name",
                    "created",
                    "updated",
                    "retention_policy",
                    "retention_limit"
                ]
                .contains(&key.as_str()),
                "unexpected header field: {key}"
            );
        }
    }
    Ok(())
}

async fn denied_key_origin(ctx: &TestContext, execution: i64) -> Result<()> {
    let encryption_key = ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .ok_or("fixture encryption key missing")?;
    let key = KeyRepository::create_encrypted_in_pool(
        &ctx.pool,
        CreateKeyInput {
            local_ref: "content_private".into(),
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Owned historical Key".into(),
            encrypted: true,
            encryption_key_hash: None,
            value: json!("private fixture material"),
        },
        encryption_key,
    )
    .await?;
    let prepared = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/private".into(),
            value: json!("private fixture material"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&SecretProvenance::key(
                attune_common::key_access::key_origin(&key),
            ))?),
        }],
        encryption_key,
    )?;
    ExecutionSecretValueRepository::upsert_many(
        &ctx.pool,
        ENTITY_EXECUTION_CONFIG,
        execution,
        &prepared,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn runtime_log_header_at_257_links_exposes_only_allowlisted_metadata() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    grant_artifacts(&ctx).await?;
    let action_ref = managed_fixture::action_pack(&ctx).await?;
    let id = artifact(
        &ctx,
        "fixture.log",
        ArtifactClassification::RuntimeLog,
        ArtifactType::FileText,
    )
    .await?;
    for _ in 0..257 {
        let execution = execution(&ctx, &action_ref).await?;
        version(&ctx, id, Some(execution), "must not appear in the header").await?;
    }
    assert_safe_header(&ctx, id).await?;
    ctx.cleanup().await
}

#[tokio::test]
async fn selected_runtime_log_body_ignores_denied_sibling_key_sources() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    grant_artifacts(&ctx).await?;
    let action_ref = managed_fixture::action_pack(&ctx).await?;
    let old_execution = execution(&ctx, &action_ref).await?;
    denied_key_origin(&ctx, old_execution).await?;
    let selected_execution = execution(&ctx, &action_ref).await?;
    let id = artifact(
        &ctx,
        "fixture.log",
        ArtifactClassification::RuntimeLog,
        ArtifactType::FileText,
    )
    .await?;
    let old = version(&ctx, id, Some(old_execution), "denied old body").await?;
    let selected = version(&ctx, id, Some(selected_execution), "selected body").await?;
    let path = format!(
        "/api/v1/artifacts/{id}/versions/{}/download?version_id={}",
        selected.version, selected.id
    );
    let response = ctx.get(&path, ctx.token()).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&response.bytes().await?)?;
    assert_eq!(body, json!({"body":"selected body"}));
    let old_path = format!(
        "/api/v1/artifacts/{id}/versions/{}/download?version_id={}",
        old.version, old.id
    );
    assert_eq!(
        ctx.get(&old_path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    // The newest metadata-visible version is denied; latest must not fall back.
    version(&ctx, id, Some(old_execution), "denied newest body").await?;
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts/{id}/download"), ctx.token())
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    let response = ctx.get(&path, ctx.token()).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&response.bytes().await?)?;
    assert_eq!(body, json!({"body":"selected body"}));
    assert_safe_header(&ctx, id).await?;
    ctx.cleanup().await
}

#[tokio::test]
async fn immutable_version_id_rejects_recreated_same_number_content() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    grant_artifacts(&ctx).await?;
    let id = artifact(
        &ctx,
        "fixture.log",
        ArtifactClassification::General,
        ArtifactType::FileText,
    )
    .await?;
    let deleted = version(&ctx, id, None, "deleted body").await?;
    assert!(ArtifactVersionRepository::delete(&ctx.pool, deleted.id).await?);
    let replacement = version(&ctx, id, None, "replacement body").await?;
    assert_eq!(replacement.version, deleted.version);
    assert_ne!(replacement.id, deleted.id);
    for suffix in [
        "versions/1",
        "versions/latest",
        "versions/1/download",
        "download",
        "stream",
    ] {
        let path = format!("/api/v1/artifacts/{id}/{suffix}?version_id={}", deleted.id);
        assert_eq!(
            ctx.get(&path, ctx.token()).await?.status(),
            StatusCode::NOT_FOUND,
            "{suffix}"
        );
    }
    for selector in [String::new(), format!("?version_id={}", replacement.id)] {
        let path = format!("/api/v1/artifacts/{id}/versions/1/download{selector}");
        let response = ctx.get(&path, ctx.token()).await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(&response.bytes().await?)?;
        assert_eq!(body, json!({"body":"replacement body"}));
    }
    ctx.cleanup().await
}

#[tokio::test]
async fn ordinary_artifact_content_preserves_nonrecorded_execution_credentials() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    grant_artifacts(&ctx).await?;
    let id = artifact(
        &ctx,
        "fixture.log",
        ArtifactClassification::General,
        ArtifactType::FileText,
    )
    .await?;
    let selected = version(&ctx, id, None, "ordinary body").await?;
    let token = attune_common::auth::jwt::generate_execution_token_with_permission_sets(
        ctx.user.as_ref().ok_or("fixture identity missing")?.id,
        1234,
        "artifact_perf.run",
        &ctx.state.jwt_config,
        Some(300),
        &["fixture.headers".into()],
    )?;
    for suffix in [
        "versions/1",
        "versions/latest",
        "versions/1/download",
        "download",
    ] {
        let path = format!("/api/v1/artifacts/{id}/{suffix}?version_id={}", selected.id);
        let response = ctx.get(&path, Some(&token)).await?;
        assert_eq!(response.status(), StatusCode::OK, "{suffix}");
        let _ = response.bytes().await?;
    }
    ctx.cleanup().await
}

#[tokio::test]
async fn nonsensitive_runtime_log_preserves_legacy_execution_reads_through_chunks_and_frames(
) -> Result<()> {
    use futures::StreamExt;
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    grant_artifacts(&ctx).await?;
    let action_ref = managed_fixture::action_pack(&ctx).await?;
    let producer = execution(&ctx, &action_ref).await?;
    let id = artifact(
        &ctx,
        "fixture.log",
        ArtifactClassification::RuntimeLog,
        ArtifactType::FileText,
    )
    .await?;
    let path = format!("legacy/{producer}.log");
    let disk_path = std::path::Path::new(&ctx.state.config.artifacts_dir).join(&path);
    std::fs::create_dir_all(disk_path.parent().ok_or("log directory missing")?)?;
    std::fs::write(&disk_path, "legacy stdout\n")?;
    let selected = ArtifactVersionRepository::create(
        &ctx.pool,
        CreateArtifactVersionInput {
            artifact: id,
            execution: Some(producer),
            content_type: Some("text/plain".into()),
            content: None,
            content_json: None,
            file_path: Some(path),
            meta: None,
            created_by: None,
        },
    )
    .await?;
    let token = attune_common::auth::jwt::generate_execution_token_with_permission_sets(
        ctx.user.as_ref().ok_or("fixture identity missing")?.id,
        producer,
        &action_ref,
        &ctx.state.jwt_config,
        Some(300),
        &["fixture.headers".into()],
    )?;
    for suffix in [
        "versions/1",
        "versions/latest",
        "versions/1/download",
        "download",
    ] {
        let path = format!("/api/v1/artifacts/{id}/{suffix}?version_id={}", selected.id);
        let response = ctx.get(&path, Some(&token)).await?;
        assert_eq!(response.status(), StatusCode::OK, "{suffix}");
        let bytes = response.bytes().await?;
        if suffix.ends_with("download") {
            assert_eq!(bytes, b"legacy stdout\n");
        }
    }
    let response = ctx
        .get(
            &format!("/api/v1/artifacts/{id}/stream?version_id={}", selected.id),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_response().into_body().into_data_stream();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(frame) = stream.next().await {
            let frame = frame?;
            let frame = String::from_utf8_lossy(&frame);
            assert!(!frame.contains("event: error"), "{frame}");
            if frame.contains("legacy stdout") {
                return Ok::<_, Box<dyn std::error::Error>>(());
            }
        }
        Err("stream ended without the selected log frame".into())
    })
    .await??;
    drop(stream);
    ctx.cleanup().await
}

#[tokio::test]
async fn generic_segment_stream_completes_after_selected_version_seals() -> Result<()> {
    segment_stream_seal_case(false).await
}

#[tokio::test]
async fn generic_segment_stream_rechecks_identity_freeze_after_selected_version_seals() -> Result<()>
{
    segment_stream_seal_case(true).await
}

/// The route opens while pending; its very next frame is polled only after the
/// same empty segmented log is sealed. No S3 bytes or wall-clock race is needed.
async fn segment_stream_seal_case(freeze_reader: bool) -> Result<()> {
    use attune_common::{
        models::{ArtifactBodyState, LogStreamBackend},
        repositories::{
            identity::{IdentityRepository, UpdateIdentityInput},
            log_stream::LogStreamRepository,
            Update,
        },
    };
    use futures::StreamExt;
    use sha2::{Digest, Sha256};
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    grant_artifacts(&ctx).await?;
    let action_ref = managed_fixture::action_pack(&ctx).await?;
    let producer = execution(&ctx, &action_ref).await?;
    let id = artifact(
        &ctx,
        "fixture.log",
        ArtifactClassification::RuntimeLog,
        ArtifactType::FileText,
    )
    .await?;
    let selected = ArtifactVersionRepository::create_log_pending(
        &ctx.pool,
        id,
        "fixture.log",
        LogStreamBackend::ObjectSegments,
        "text/plain".into(),
        Some(producer),
        None,
        None,
    )
    .await?;
    let stream = LogStreamRepository::create_with_backend(
        &ctx.pool,
        selected.id,
        LogStreamBackend::ObjectSegments,
        64 * 1024,
        100,
    )
    .await?;
    assert_eq!(selected.body_state, Some(ArtifactBodyState::Pending));
    assert!(selected.provider_version.is_none());
    let response = ctx
        .get(
            &format!("/api/v1/artifacts/{id}/stream?version_id={}", selected.id),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut tx = ctx.pool.begin().await?;
    LogStreamRepository::seal(&mut tx, stream.id, false).await?;
    let ready = ArtifactVersionRepository::mark_body_ready_in_transaction(
        &mut tx,
        selected.id,
        "segments:0",
        0,
        &hex::encode(Sha256::digest(b"")),
    )
    .await?
    .ok_or("selected version did not seal")?;
    tx.commit().await?;
    assert_eq!(ready.id, selected.id);
    assert_eq!(ready.execution, selected.execution);
    assert_eq!(ready.body_state, Some(ArtifactBodyState::Ready));
    if freeze_reader {
        IdentityRepository::update(
            &ctx.pool,
            ctx.user.as_ref().ok_or("reader missing")?.id,
            UpdateIdentityInput {
                frozen: Some(true),
                ..Default::default()
            },
        )
        .await?;
    }
    let mut body = response.into_response().into_body().into_data_stream();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(10), body.next())
        .await?
        .ok_or("stream ended without a frame")??;
    let frame = String::from_utf8_lossy(&frame);
    if freeze_reader {
        assert!(frame.contains("artifact_stream_access_revoked"), "{frame}");
        assert!(!frame.contains("event: done"), "{frame}");
    } else {
        assert!(frame.contains("event: done"), "{frame}");
        assert!(!frame.contains("artifact_stream_access_revoked"), "{frame}");
    }
    drop(body);
    ctx.cleanup().await
}

#[tokio::test]
async fn runtime_log_header_does_not_authorize_a_missing_selected_execution() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    grant_artifacts(&ctx).await?;
    let id = artifact(
        &ctx,
        "fixture.log",
        ArtifactClassification::RuntimeLog,
        ArtifactType::FileText,
    )
    .await?;
    ArtifactVersionRepository::create(
        &ctx.pool,
        CreateArtifactVersionInput {
            artifact: id,
            execution: Some(i64::MAX - 1),
            content_type: Some("text/plain".into()),
            content: None,
            content_json: Some(json!({"protected":"body"})),
            file_path: None,
            meta: Some(json!({"protected":"version metadata"})),
            created_by: None,
        },
    )
    .await?;
    for path in [
        format!("/api/v1/artifacts/{id}"),
        "/api/v1/artifacts/ref/fixture.log".into(),
    ] {
        let response = ctx.get(&path, ctx.token()).await?;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body: Value = response.json().await?;
        let header = body["data"].as_object().ok_or("header missing")?;
        assert_eq!(header.len(), 12);
        for key in header.keys() {
            assert!(
                [
                    "id",
                    "ref",
                    "scope",
                    "owner",
                    "type",
                    "visibility",
                    "classification",
                    "name",
                    "created",
                    "updated",
                    "retention_policy",
                    "retention_limit"
                ]
                .contains(&key.as_str()),
                "unexpected header field: {key}"
            );
        }
    }
    for suffix in [
        "versions/1",
        "versions/latest",
        "versions/1/download",
        "download",
        "stream",
    ] {
        let response = ctx
            .get(&format!("/api/v1/artifacts/{id}/{suffix}"), ctx.token())
            .await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{suffix}");
    }
    ctx.cleanup().await
}

#[tokio::test]
async fn ordinary_progress_detail_keeps_its_protected_data_projection() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    let id = artifact(
        &ctx,
        "fixture.progress",
        ArtifactClassification::General,
        ArtifactType::Progress,
    )
    .await?;
    let denied = ctx
        .get(&format!("/api/v1/artifacts/{id}"), ctx.token())
        .await?;
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    grant_artifacts(&ctx).await?;
    let response = ctx
        .get(&format!("/api/v1/artifacts/{id}"), ctx.token())
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["description"], "protected description");
    assert_eq!(body["data"]["data"], json!({"progress":"protected data"}));
    ctx.cleanup().await
}
