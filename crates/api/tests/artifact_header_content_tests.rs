//! Metadata visibility must never become a permit for protected content.
#[path = "../../common/tests/support/object_log.rs"]
mod object_log_fixture;

use helpers::delegated;
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
    execution_with_refs(ctx, action_ref, &[]).await
}

async fn execution_with_refs(ctx: &TestContext, action_ref: &str, refs: &[String]) -> Result<i64> {
    let response = ctx
        .post(
            "/api/v1/executions/execute",
            json!({
                "action_ref":action_ref, "parameters":{}, "permission_set_refs":refs
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
    let action_ref = managed_fixture::action_pack(&ctx).await?;
    let execution = execution_with_refs(&ctx, &action_ref, &["fixture.headers".into()]).await?;
    let mut tx = ctx.pool.begin().await?;
    let attempt = delegated::start(&mut tx, execution).await?;
    tx.commit().await?;
    let token = attune_common::auth::jwt::generate_delegated_execution_token(
        ctx.user.as_ref().ok_or("fixture identity missing")?.id,
        execution,
        &action_ref,
        &ctx.state.jwt_config,
        300,
        &["fixture.headers".into()],
        &[],
        attempt,
    )?;
    // Bound the handler's stored branch state independently of Tokio test
    // stack settings; the production acceptance also exercises actual polling.
    let user = attune_api::auth::middleware::AuthenticatedUser {
        claims: attune_common::auth::jwt::validate_token(&token, &ctx.state.jwt_config)?,
    };
    let numbered = attune_api::routes::artifacts::download_version(
        attune_api::auth::middleware::RequireAuth(user.clone()),
        axum::extract::State(ctx.state.clone()),
        axum::extract::Path((id, 1)),
        axum::extract::Query(Default::default()),
        axum::http::HeaderMap::new(),
    );
    assert!(
        std::mem::size_of_val(&numbered) <= 128 * 1024,
        "numbered download future uses {} bytes",
        std::mem::size_of_val(&numbered)
    );
    drop(numbered);
    let latest = attune_api::routes::artifacts::download_latest(
        attune_api::auth::middleware::RequireAuth(user),
        axum::extract::State(ctx.state.clone()),
        axum::extract::Path(id),
        axum::extract::Query(Default::default()),
        axum::http::HeaderMap::new(),
    );
    assert!(
        std::mem::size_of_val(&latest) <= 128 * 1024,
        "latest download future uses {} bytes",
        std::mem::size_of_val(&latest)
    );
    drop(latest);
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
    let producer = execution_with_refs(&ctx, &action_ref, &["fixture.headers".into()]).await?;
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
    let mut tx = ctx.pool.begin().await?;
    let attempt = delegated::start(&mut tx, producer).await?;
    tx.commit().await?;
    let token = attune_common::auth::jwt::generate_delegated_execution_token(
        ctx.user.as_ref().ok_or("fixture identity missing")?.id,
        producer,
        &action_ref,
        &ctx.state.jwt_config,
        300,
        &["fixture.headers".into()],
        &[],
        attempt,
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
    object_log_fixture::admission(&mut tx).await.unwrap();
    object_log_fixture::seal(&mut tx, stream.id, false).await?;
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

#[tokio::test]
async fn running_delegated_logs_enforce_frozen_use_ceiling_before_result_materialization(
) -> Result<()> {
    use attune_common::{
        auth::jwt::generate_access_token,
        models::ExecutionStatus,
        repositories::{
            execution::ExecutionRepository,
            identity::{CreateIdentityInput, IdentityRepository},
            FindById, FindByRef,
        },
        secret_values::ENTITY_EXECUTION_RESULT,
    };
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    grant_artifacts(&ctx).await?;
    let owner = ctx.user.as_ref().ok_or("owner missing")?.id;
    let signer = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "fixture.live_signer".into(), pack: None, pack_ref: None, label: None, description: None,
        grants: json!([{"resource":"keys","actions":["use"],"constraints":{"owner_types":["system"],"refs":["system.live_signer"]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: owner,
            permset: signer.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(owner).await;
    let action = managed_fixture::action_pack(&ctx).await?;
    let producer = execution_with_refs(&ctx, &action, &[signer.r#ref]).await?;
    let mut tx = ctx.pool.begin().await?;
    delegated::start(&mut tx, producer).await?;
    tx.commit().await?;
    let source = ExecutionRepository::find_by_id(&ctx.pool, producer)
        .await?
        .ok_or("source missing")?;
    assert_eq!(source.status, ExecutionStatus::Running);
    assert!(source.result.is_none());
    assert!(ExecutionSecretValueRepository::find_stored_by_entity(
        &ctx.pool,
        ENTITY_EXECUTION_RESULT,
        producer
    )
    .await?
    .is_empty());
    assert!(attune_common::repositories::execution_key_read::ExecutionKeyReadRepository::origins_for_executions(&ctx.pool, &[producer]).await?.is_empty());
    let log = artifact(
        &ctx,
        "fixture.log",
        ArtifactClassification::RuntimeLog,
        ArtifactType::FileText,
    )
    .await?;
    let selected = version(
        &ctx,
        log,
        Some(producer),
        "early-approved-operation-assertion",
    )
    .await?;
    let viewer = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "live-log-viewer".into(),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;
    let headers = PermissionSetRepository::find_by_ref(&ctx.pool, "fixture.headers")
        .await?
        .ok_or("header permissions missing")?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: viewer.id,
            permset: headers.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(viewer.id).await;
    let token = generate_access_token(viewer.id, &viewer.login, &ctx.state.jwt_config)?;
    let paths = [
        format!(
            "/api/v1/artifacts/{log}/versions/{}/download",
            selected.version
        ),
        format!("/api/v1/artifacts/{log}/download"),
    ];
    for path in &paths {
        assert_eq!(
            ctx.get(path, ctx.token()).await?.status(),
            StatusCode::OK,
            "the covered producer can read its early output"
        );
        let denied = ctx.get(path, Some(&token)).await?;
        assert_eq!(denied.status(), StatusCode::NOT_FOUND, "log read/decrypt alone cannot disclose an API-capable producer's output before result provenance exists");
    }
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: viewer.id,
            permset: signer.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(viewer.id).await;
    for path in &paths {
        let covered = ctx.get(path, Some(&token)).await?;
        assert_eq!(
            covered.status(),
            StatusCode::OK,
            "use coverage allows approved-operation output without private-key read/decrypt"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&covered.bytes().await?)?,
            json!({"body":"early-approved-operation-assertion"})
        );
    }
    // Opaque API-capable output requires execution decrypt even before any
    // result or runtime Key origin has been materialized.
    let read_only = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "live-log-read-only".into(),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;
    let read_set = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "fixture.live_read_only".into(), pack: None, pack_ref: None, label: None, description: None,
        grants: json!([
            {"resource":"artifacts","actions":["read"],"constraints":{"refs":["fixture.log"]}},
            {"resource":"executions","actions":["read"]},
            {"resource":"keys","actions":["use"],"constraints":{"owner_types":["system"],"refs":["system.live_signer"]}}
        ]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: read_only.id,
            permset: read_set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(read_only.id).await;
    let read_token = generate_access_token(read_only.id, &read_only.login, &ctx.state.jwt_config)?;
    for path in &paths {
        assert_eq!(
            ctx.get(path, Some(&read_token)).await?.status(),
            StatusCode::NOT_FOUND,
            "approved-operation coverage alone cannot replace execution decrypt for opaque logs"
        );
    }
    ctx.cleanup().await?;
    Ok(())
}

/// Diagnostics follow full content authorization and remain bound to one version.
#[tokio::test]
async fn pending_runtime_log_downloads_distinguish_finalizing_incomplete_and_denied() -> Result<()>
{
    use attune_common::{
        audit::{AuditEventFilters, AuditRepository},
        models::{ExecutionStatus, LogStreamBackend},
        repositories::{
            execution::{ExecutionRepository, UpdateExecutionInput},
            Update,
        },
    };
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
        Some(json!({"log_encoding":"attune_encrypted_log_v1"})),
        None,
    )
    .await?;
    let paths = [
        format!(
            "/api/v1/artifacts/{id}/versions/{}/download?version_id={}",
            selected.version, selected.id
        ),
        format!("/api/v1/artifacts/{id}/download?version_id={}", selected.id),
    ];
    for (status, code) in [
        (ExecutionStatus::Running, "LOG_FINALIZING"),
        (ExecutionStatus::Completed, "LOG_INCOMPLETE"),
        (ExecutionStatus::Failed, "LOG_INCOMPLETE"),
        (ExecutionStatus::Timeout, "LOG_INCOMPLETE"),
        (ExecutionStatus::Cancelled, "LOG_INCOMPLETE"),
        (ExecutionStatus::Abandoned, "LOG_INCOMPLETE"),
    ] {
        ExecutionRepository::update(
            &ctx.pool,
            producer,
            UpdateExecutionInput {
                status: Some(status),
                ..Default::default()
            },
        )
        .await?;
        for path in &paths {
            let response = ctx.get(path, ctx.token()).await?;
            assert_eq!(response.status(), StatusCode::CONFLICT);
            let body: Value = response.json().await?;
            assert_eq!(body["code"], code);
        }
    }
    // Readable metadata is not sufficient to learn the state of secret-bearing content.
    denied_key_origin(&ctx, producer).await?;
    for path in &paths {
        let response = ctx.get(path, ctx.token()).await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body: Value = response.json().await?;
        assert_ne!(body["code"], "LOG_INCOMPLETE");
    }
    ctx.flush_audit().await?;
    let events = AuditRepository::search(
        &ctx.pool,
        &AuditEventFilters {
            event_type: Some("artifact.downloaded".into()),
            resource_id: Some(id),
            ..Default::default()
        },
    )
    .await?;
    assert!(
        events.is_empty(),
        "failed downloads must not emit success audits"
    );
    ctx.cleanup().await
}

#[tokio::test]
async fn runtime_log_download_audit_requires_full_authorized_eof() -> Result<()> {
    use attune_common::{
        audit::{AuditEventFilters, AuditRepository},
        repositories::{
            identity::{IdentityRepository, UpdateIdentityInput},
            Update,
        },
    };
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
    let selected = version(&ctx, id, Some(producer), "owned log bytes").await?;
    let path = format!(
        "/api/v1/artifacts/{id}/versions/{}/download?version_id={}",
        selected.version, selected.id
    );
    let filters = AuditEventFilters {
        event_type: Some("artifact.downloaded".into()),
        resource_id: Some(id),
        ..Default::default()
    };
    let response = ctx.get(&path, ctx.token()).await?;
    assert_eq!(response.status(), StatusCode::OK);
    ctx.flush_audit().await?;
    assert!(AuditRepository::search(&ctx.pool, &filters)
        .await?
        .is_empty());
    assert!(!response.bytes().await?.is_empty());
    ctx.flush_audit().await?;
    let events = AuditRepository::search(&ctx.pool, &filters).await?;
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].details.as_ref().and_then(|d| d.get("delivery")),
        Some(&json!("verified_eof"))
    );

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::RANGE,
        axum::http::HeaderValue::from_static("bytes=0-0"),
    );
    let response = ctx.get_with_headers(&path, ctx.token(), headers).await?;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.bytes().await?.len(), 1);
    ctx.flush_audit().await?;
    assert_eq!(AuditRepository::search(&ctx.pool, &filters).await?.len(), 1);

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::RANGE,
        axum::http::HeaderValue::from_static("bytes=999999-"),
    );
    let response = ctx.get_with_headers(&path, ctx.token(), headers).await?;
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert!(!response.bytes().await?.is_empty());
    ctx.flush_audit().await?;
    assert_eq!(AuditRepository::search(&ctx.pool, &filters).await?.len(), 1);

    let response = ctx.get(&path, ctx.token()).await?;
    let mut body = response.into_response().into_body().into_data_stream();
    assert!(body.next().await.ok_or("missing log bytes")?.is_ok());
    drop(body); // A cancelled preview has not verified EOF.
    ctx.flush_audit().await?;
    assert_eq!(AuditRepository::search(&ctx.pool, &filters).await?.len(), 1);

    let response = ctx.get(&path, ctx.token()).await?;
    assert_eq!(response.status(), StatusCode::OK);
    IdentityRepository::update(
        &ctx.pool,
        ctx.user.as_ref().ok_or("reader missing")?.id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    assert!(response.bytes().await.is_err());
    ctx.flush_audit().await?;
    assert_eq!(AuditRepository::search(&ctx.pool, &filters).await?.len(), 1);
    ctx.cleanup().await
}

#[tokio::test]
async fn runtime_log_http1_download_reaches_audited_eof_for_nonempty_and_empty_bodies() -> Result<()>
{
    use attune_common::audit::{AuditEventFilters, AuditRepository};
    use axum::http::{header, Version};
    use tokio_util::sync::CancellationToken;

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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let stop = CancellationToken::new();
    let shutdown = stop.clone();
    let app = ctx.app.clone();
    // JoinSet aborts the server on panic; the normal path stops and joins it.
    let mut servers = tokio::task::JoinSet::new();
    servers.spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
    });
    let client = reqwest::Client::builder().http1_only().build()?;
    let checks: Result<()> = async {
        for payload in [b"HTTP/1 runtime log bytes".as_slice(), b""] {
            let selected = ArtifactVersionRepository::create(
                &ctx.pool,
                CreateArtifactVersionInput {
                    artifact: id,
                    execution: Some(producer),
                    content_type: Some("text/plain".into()),
                    content: Some(payload.to_vec()),
                    content_json: None,
                    file_path: None,
                    meta: None,
                    created_by: None,
                },
            )
            .await?;
            let response = client
                .get(format!(
                    "http://{address}/api/v1/artifacts/{id}/versions/{}/download?version_id={}",
                    selected.version, selected.id
                ))
                .bearer_auth(ctx.token().ok_or("reader token missing")?)
                .header(header::ACCEPT_ENCODING, "identity")
                .send()
                .await?;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.version(), Version::HTTP_11);
            assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
            assert!(!response.headers().contains_key(header::CONTENT_LENGTH));
            assert_eq!(response.bytes().await?.as_ref(), payload);
            ctx.flush_audit().await?;
            let events = AuditRepository::search(
                &ctx.pool,
                &AuditEventFilters {
                    event_type: Some("artifact.downloaded".into()),
                    resource_id: Some(id),
                    ..Default::default()
                },
            )
            .await?;
            let matching: Vec<_> = events
                .iter()
                .filter(|event| {
                    event
                        .details
                        .as_ref()
                        .and_then(|details| details.get("version_id"))
                        == Some(&json!(selected.id))
                })
                .collect();
            assert_eq!(matching.len(), 1, "HTTP/1 EOF must emit one success audit");
            assert_eq!(
                matching[0]
                    .details
                    .as_ref()
                    .and_then(|details| details.get("delivery")),
                Some(&json!("verified_eof"))
            );
        }
        Ok(())
    }
    .await;
    drop(client);
    stop.cancel();
    let server = tokio::time::timeout(std::time::Duration::from_secs(10), servers.join_next())
        .await?
        .ok_or("HTTP/1 server task missing")??;
    server?;
    let cleanup = ctx.cleanup().await;
    checks?;
    cleanup
}
