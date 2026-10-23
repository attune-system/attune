use super::*;
use attune_common::{
    models::{ArtifactBodyState, WorkerType},
    repositories::{
        artifact::ArtifactVersionRepository,
        runtime::{CreateWorkerInput, WorkerRepository},
        worker_delivery::{DeliveryContext, WorkerDeliveryRepository},
        FindById,
    },
};
use axum::{body::Body, http::Request};
use tower::ServiceExt;
use uuid::Uuid;

async fn created_file_allocation(
    response: super::helpers::TestResponse,
    site: &str,
) -> Result<Value> {
    let status = response.status();
    let body = response.text().await?;
    assert_eq!(status, StatusCode::CREATED, "{site}: {body}");
    Ok(serde_json::from_str(&body)?)
}

async fn ready_file_response(
    response: super::helpers::TestResponse,
    site: &str,
) -> Result<super::helpers::TestResponse> {
    let status = response.status();
    if status != StatusCode::OK {
        let body = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            axum::body::to_bytes(response.into_response().into_body(), 64 * 1024),
        )
        .await??;
        panic!(
            "{site}: expected Ready HTTP 200, got {status}; body={}",
            String::from_utf8_lossy(&body)
        );
    }
    Ok(response)
}

struct PauseAfterPut {
    inner: std::sync::Arc<dyn attune_common::blob_store::BlobStore>,
    uploaded: tokio::sync::Notify,
    finish: tokio::sync::Semaphore,
    fail_delete_once: std::sync::atomic::AtomicBool,
}
impl std::fmt::Debug for PauseAfterPut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PauseAfterPut")
    }
}
#[async_trait::async_trait]
impl attune_common::blob_store::BlobStore for PauseAfterPut {
    async fn preflight(
        &self,
    ) -> std::result::Result<(), attune_common::blob_store::BlobStoreError> {
        self.inner.preflight().await
    }
    async fn put(
        &self,
        key: &attune_common::blob_store::ObjectKey,
        body: attune_common::blob_store::BlobBody,
        digest: [u8; 32],
    ) -> std::result::Result<
        attune_common::blob_store::StoredObject,
        attune_common::blob_store::BlobStoreError,
    > {
        let stored = self.inner.put(key, body, digest).await?;
        self.uploaded.notify_one();
        self.finish
            .acquire()
            .await
            .expect("owned publication barrier")
            .forget();
        Ok(stored)
    }
    async fn get(
        &self,
        key: &attune_common::blob_store::ObjectKey,
        version: &attune_common::blob_store::ProviderVersion,
        range: Option<attune_common::blob_store::ByteRange>,
    ) -> std::result::Result<
        attune_common::blob_store::BlobReader,
        attune_common::blob_store::BlobStoreError,
    > {
        self.inner.get(key, version, range).await
    }
    async fn put_file_atomic(
        &self,
        key: &attune_common::blob_store::ObjectKey,
        body: attune_common::blob_store::BlobBody,
        size: u64,
        digest: [u8; 32],
    ) -> std::result::Result<
        attune_common::blob_store::StoredObject,
        attune_common::blob_store::BlobStoreError,
    > {
        let stored = self.inner.put_file_atomic(key, body, size, digest).await?;
        self.uploaded.notify_one();
        self.finish
            .acquire()
            .await
            .expect("owned atomic publication barrier")
            .forget();
        Ok(stored)
    }
    async fn get_pinned(
        &self,
        key: &attune_common::blob_store::ObjectKey,
        version: &attune_common::blob_store::ProviderVersion,
        size: u64,
        digest: [u8; 32],
        range: Option<attune_common::blob_store::ByteRange>,
    ) -> std::result::Result<
        attune_common::blob_store::BlobReader,
        attune_common::blob_store::BlobStoreError,
    > {
        self.inner
            .get_pinned(key, version, size, digest, range)
            .await
    }
    async fn head(
        &self,
        key: &attune_common::blob_store::ObjectKey,
    ) -> std::result::Result<
        Option<attune_common::blob_store::StoredObject>,
        attune_common::blob_store::BlobStoreError,
    > {
        self.inner.head(key).await
    }
    async fn head_version(
        &self,
        key: &attune_common::blob_store::ObjectKey,
        version: &attune_common::blob_store::ProviderVersion,
    ) -> std::result::Result<
        Option<attune_common::blob_store::StoredObject>,
        attune_common::blob_store::BlobStoreError,
    > {
        self.inner.head_version(key, version).await
    }
    async fn delete(
        &self,
        key: &attune_common::blob_store::ObjectKey,
        version: &attune_common::blob_store::ProviderVersion,
    ) -> std::result::Result<(), attune_common::blob_store::BlobStoreError> {
        if self
            .fail_delete_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(attune_common::blob_store::BlobStoreError::Conflict);
        }
        self.inner.delete(key, version).await
    }
}

async fn delivery(fixture: &Fixture) -> Result<DeliveryContext> {
    let worker = WorkerRepository::create(
        &fixture.ctx.pool,
        CreateWorkerInput {
            name: "recorded-file-worker".into(),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(attune_common::models::WorkerStatus::Active),
            capabilities: None,
            meta: None,
        },
    )
    .await?;
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            worker: Some(worker.id),
            ..Default::default()
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let context = WorkerDeliveryRepository::register(
        &mut tx,
        fixture.credential.execution_id,
        worker.id,
        Uuid::new_v4(),
        chrono::Utc::now() + chrono::Duration::minutes(5),
    )
    .await?;
    tx.commit().await?;
    Ok(context)
}

async fn raw(
    fixture: &Fixture,
    method: &str,
    path: &str,
    token: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> Result<axum::http::StatusCode> {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", content_type)
        .header("content-length", bytes.len())
        .header("x-attune-sha256", digest)
        .body(Body::from(bytes))?;
    let response = fixture.ctx.app.clone().oneshot(request).await?;
    let status = response.status();
    if !status.is_success() {
        // Consume the actual response before returning the status to callers.
        // Allocation-only diagnostics cannot explain multipart/PUT failures.
        let body = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            axum::body::to_bytes(response.into_body(), 64 * 1024),
        )
        .await??;
        eprintln!(
            "file request {method} {path}: HTTP {status}; body={}",
            String::from_utf8_lossy(&body)
        );
    }
    Ok(status)
}

#[tokio::test]
async fn recorded_file_allocations_bind_worker_delivery_and_reject_impersonation() -> Result<()> {
    let fixture = fixture(true).await?;
    let context = delivery(&fixture).await?;
    let path = "/api/v1/artifacts/ref/recorded_emitter.files/versions/file";
    let allocation = fixture
        .ctx
        .post(
            path,
            json!({"scope":"action","owner":"recorded_emitter.emit","type":"file_text"}),
            Some(&fixture.token),
        )
        .await?;
    let body = created_file_allocation(allocation, "recorded worker file allocation").await?;
    let version = body["data"]["id"].as_i64().unwrap();
    let artifact = body["data"]["artifact"].as_i64().unwrap();
    let mut conn = fixture.ctx.pool.acquire().await?;
    let permit = WorkerDeliveryRepository::for_version(&mut conn, version)
        .await?
        .unwrap();
    assert_eq!(permit.context, context.id);
    assert!(WorkerDeliveryRepository::require(
        &mut conn,
        permit.id,
        context.worker,
        Uuid::new_v4(),
        version
    )
    .await
    .is_err());
    drop(conn);
    let forbidden = fixture
        .ctx
        .post(
            &format!("/api/v1/artifacts/{artifact}/versions/file"),
            json!({"execution":fixture.credential.execution_id+1}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    let foreign = fixture
        .ctx
        .post(
            "/api/v1/artifacts/ref/foreign_files/versions/file",
            json!({"scope":"pack","owner":"foreign_events"}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    // Both multipart production routes use the same source/target publication fence.
    let bytes = b"--f-boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"file.txt\"\r\nContent-Type: text/plain\r\n\r\nfile output\r\n--f-boundary--\r\n".to_vec();
    for path in [
        format!("/api/v1/artifacts/{artifact}/versions/upload"),
        "/api/v1/artifacts/ref/recorded_emitter.files/versions/upload".into(),
    ] {
        assert_eq!(
            raw(
                &fixture,
                "POST",
                &path,
                &fixture.token,
                "multipart/form-data; boundary=f-boundary",
                bytes.clone()
            )
            .await?,
            axum::http::StatusCode::CREATED
        );
    }
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn pending_destination_is_not_an_acquired_read_during_process_file_publication() -> Result<()>
{
    use attune_common::repositories::runtime_read::RuntimeReadRepository;
    let fixture = fixture(true).await?;
    delivery(&fixture).await?;
    let response = fixture
        .ctx
        .post(
            "/api/v1/artifacts/ref/recorded_emitter.pending_sink_control/versions/file",
            json!({"scope":"action","owner":"recorded_emitter.emit","type":"file_text"}),
            Some(&fixture.token),
        )
        .await?;
    let allocated = created_file_allocation(response, "pending destination control").await?;
    let path = format!(
        "/api/v1/internal/files/{}",
        allocated["data"]["file_path"].as_str().unwrap()
    );
    let before = RuntimeReadRepository::references_for_executions(
        &fixture.ctx.pool,
        &[fixture.credential.execution_id],
    )
    .await?;
    // A write permit does not make the Pending body readable. Keep the ordinary
    // reader denial while authorizing publication's original sources instead.
    assert!(!fixture
        .ctx
        .get(&path, Some(&fixture.token))
        .await?
        .status()
        .is_success());
    let after = RuntimeReadRepository::references_for_executions(
        &fixture.ctx.pool,
        &[fixture.credential.execution_id],
    )
    .await?;
    assert_eq!(
        before, after,
        "denied Pending body reads must not become acquired inputs"
    );
    let bytes = b"original process output".to_vec();
    assert_eq!(
        raw(
            &fixture,
            "PUT",
            &path,
            &fixture.token,
            "text/plain",
            bytes.clone()
        )
        .await?,
        axum::http::StatusCode::CREATED
    );
    // Standard grants publication and scoped artifact reads, not decrypt. Do
    // not widen the producer merely to force its own Ready-body GET to pass.
    assert_eq!(
        fixture.ctx.get(&path, Some(&fixture.token)).await?.status(),
        StatusCode::FORBIDDEN
    );
    use attune_common::repositories::identity::{
        CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
        PermissionAssignmentRepository, PermissionSetRepository,
    };
    let reader = IdentityRepository::create(
        &fixture.ctx.pool,
        CreateIdentityInput {
            login: format!("file_reader_{}", Uuid::new_v4().simple()),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await?;
    assert_ne!(reader.id, fixture.requester);
    assert_ne!(reader.id, fixture.credential.sponsor_identity_id);
    let artifact = allocated["data"]["artifact"]
        .as_i64()
        .ok_or("file allocation must expose its artifact ID")?;
    let reader_set=PermissionSetRepository::create(&fixture.ctx.pool,CreatePermissionSetInput {
        r#ref:"fixture.pending_file_reader".into(),pack:None,pack_ref:None,label:None,description:None,
        grants:json!([
            {"resource":"artifacts","actions":["read","decrypt"],"constraints":{"ids":[artifact]}},
            {"resource":"executions","actions":["read","decrypt"],"constraints":{"ids":[fixture.credential.execution_id]}}
        ]),
    }).await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: reader.id,
            permset: reader_set.id,
        },
    )
    .await?;
    let reader_token = attune_common::auth::jwt::generate_access_token(
        reader.id,
        &reader.login,
        &fixture.ctx.state.jwt_config,
    )?;
    let delivered = ready_file_response(
        fixture.ctx.get(&path, Some(&reader_token)).await?,
        "independent scoped Ready reader",
    )
    .await?;
    assert_eq!(delivered.bytes().await?.as_slice(), bytes.as_slice());
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_empty_cache_tombstone_can_be_physically_reclaimed_at_an_exclusive_outer_entry(
) -> Result<()> {
    use attune_common::repositories::cache::CacheNamespaceRepository;
    let fixture = fixture_with_capabilities(false, false, &["read"], &["create", "delete"]).await?;
    let created = fixture.ctx.post("/api/v1/cache/namespaces",
        json!({"owner_type":"pack","owner_ref":"recorded_emitter","namespace":"empty_file_custody_control"}),Some(&fixture.token)).await?;
    let status = created.status();
    let body: Value = created.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let namespace = body["data"]["id"].as_i64().unwrap();
    let deleted=fixture.ctx.delete("/api/v1/cache/namespaces/empty_file_custody_control?owner_type=pack&owner_ref=recorded_emitter",Some(&fixture.token)).await?;
    let status = deleted.status();
    let body: Value = deleted.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    // API tombstoning commits its ordinary effect transaction. Reclamation
    // starts a NEW owning transaction, exclusive graph FIRST. Do not smuggle a
    // shared source/PinMutation transaction into a late destructive upgrade.
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            CacheNamespaceRepository::delete_tombstoned_if_empty(&fixture.ctx.pool, namespace)
        )
        .await??
    );
    assert!(
        CacheNamespaceRepository::find_by_id(&fixture.ctx.pool, namespace)
            .await?
            .is_none()
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn by_ref_initial_upload_keeps_create_only_authority_distinct_from_update() -> Result<()> {
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &["read"],
        &[],
        &[json!({"resource":"artifacts","actions":["create"]})],
    )
    .await?;
    delivery(&fixture).await?;
    let bytes = b"--f-create\r\nContent-Disposition: form-data; name=\"scope\"\r\n\r\naction\r\n--f-create\r\nContent-Disposition: form-data; name=\"owner\"\r\n\r\nrecorded_emitter.emit\r\n--f-create\r\nContent-Disposition: form-data; name=\"file\"; filename=\"file.txt\"\r\nContent-Type: text/plain\r\n\r\ncreated file\r\n--f-create--\r\n".to_vec();
    let path = "/api/v1/artifacts/ref/recorded_emitter.create_only/versions/upload";
    assert_eq!(
        raw(
            &fixture,
            "POST",
            path,
            &fixture.token,
            "multipart/form-data; boundary=f-create",
            bytes.clone()
        )
        .await?,
        axum::http::StatusCode::CREATED
    );
    assert_eq!(
        raw(
            &fixture,
            "POST",
            path,
            &fixture.token,
            "multipart/form-data; boundary=f-create",
            bytes
        )
        .await?,
        axum::http::StatusCode::FORBIDDEN
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn terminal_worker_delivery_is_version_bound_and_revocation_still_wins() -> Result<()> {
    let fixture = fixture(true).await?;
    let context = delivery(&fixture).await?;
    let allocation = fixture
        .ctx
        .post(
            "/api/v1/artifacts/ref/recorded_emitter.final/versions/file",
            json!({"scope":"action","owner":"recorded_emitter.emit","type":"file_text"}),
            Some(&fixture.token),
        )
        .await?;
    let body = created_file_allocation(allocation, "terminal completion file allocation").await?;
    let version = body["data"]["id"].as_i64().unwrap();
    let file_path = body["data"]["file_path"].as_str().unwrap();
    let mut conn = fixture.ctx.pool.acquire().await?;
    let permit = WorkerDeliveryRepository::for_version(&mut conn, version)
        .await?
        .unwrap();
    let token =
        attune_common::worker_delivery::mint(&context, &permit, &fixture.ctx.state.jwt_config)?;
    drop(conn);
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        fixture
            .ctx
            .get("/api/v1/schema-types", Some(&token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let path = format!("/api/v1/internal/files/{file_path}");
    let ordinary_worker = attune_common::auth::jwt::generate_worker_token_with_instance(
        context.worker,
        &context.worker.to_string(),
        context.worker_instance,
        &fixture.ctx.state.jwt_config,
        Some(60),
    )?;
    assert_eq!(
        raw(
            &fixture,
            "PUT",
            &path,
            &ordinary_worker,
            "text/plain",
            b"final bytes".to_vec()
        )
        .await?,
        axum::http::StatusCode::FORBIDDEN
    );
    assert_eq!(
        raw(
            &fixture,
            "PUT",
            &path,
            &fixture.token,
            "text/plain",
            b"final bytes".to_vec()
        )
        .await?,
        axum::http::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        raw(
            &fixture,
            "PUT",
            &path,
            &token,
            "text/plain",
            b"final bytes".to_vec()
        )
        .await?,
        axum::http::StatusCode::CREATED
    );
    let ready = ArtifactVersionRepository::find_by_id(&fixture.ctx.pool, version)
        .await?
        .unwrap();
    assert_eq!(ready.body_state, Some(ArtifactBodyState::Ready));
    let mut tx = fixture.ctx.pool.begin().await?;
    assert!(WorkerDeliveryRepository::start(
        &mut tx,
        fixture.credential.execution_id,
        context.worker
    )
    .await
    .is_err());
    WorkerDeliveryRepository::require(
        &mut tx,
        permit.id,
        context.worker,
        context.worker_instance,
        version,
    )
    .await?;
    assert!(WorkerDeliveryRepository::require(
        &mut tx,
        permit.id,
        context.worker + 1,
        context.worker_instance,
        version
    )
    .await
    .is_err());
    tx.rollback().await?;
    // Recheck under a fresh transaction, exactly as final publication does.
    RuntimeAuthorityRepository::revoke_admission(
        &mut *fixture.ctx.pool.acquire().await?,
        fixture.credential.admission_id,
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    assert!(WorkerDeliveryRepository::require(
        &mut tx,
        permit.id,
        context.worker,
        context.worker_instance,
        version
    )
    .await
    .is_err());
    tx.rollback().await?;
    assert_eq!(
        raw(
            &fixture,
            "PUT",
            &path,
            &token,
            "text/plain",
            b"final bytes".to_vec()
        )
        .await?,
        axum::http::StatusCode::FORBIDDEN
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn terminal_log_delivery_keeps_caps_retries_and_sealing_operation_specific() -> Result<()> {
    use attune_common::{models::LogStreamBackend, repositories::log_stream::LogStreamRepository};
    let fixture = fixture(true).await?;
    let context = delivery(&fixture).await?;
    let created = fixture
        .ctx
        .post(
            "/api/v1/artifacts",
            json!({"ref":"recorded_emitter.final.stdout.log",
        "scope":"action","owner":"recorded_emitter.emit","type":"file_text"}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let artifact: Value = created.json().await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let (_, effect) = WorkerDeliveryRepository::require_launch_context(&mut tx, &context).await?;
    let version = ArtifactVersionRepository::create_log_pending_with_conn(
        &mut tx,
        artifact["data"]["id"].as_i64().unwrap(),
        "recorded_emitter.final.stdout.log",
        LogStreamBackend::ObjectSegments,
        context.execution,
        json!({"stream":"stdout"}),
    )
    .await?;
    let stream = LogStreamRepository::create_with_backend_with_conn(
        &mut tx,
        version.id,
        LogStreamBackend::ObjectSegments,
        16,
        500,
    )
    .await?;
    let permit = WorkerDeliveryRepository::bind(
        &mut tx,
        &context,
        version.id,
        Some(stream.id),
        4,
        2,
        attune_common::rbac::Action::Update,
        &effect,
    )
    .await?;
    tx.commit().await?;
    let token =
        attune_common::worker_delivery::mint(&context, &permit, &fixture.ctx.state.jwt_config)?;
    ExecutionRepository::update(
        &fixture.ctx.pool,
        context.execution,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            ..Default::default()
        },
    )
    .await?;
    let path = format!("/api/v1/internal/logs/{}/segments", version.id);
    assert_eq!(
        raw(
            &fixture,
            "PUT",
            &format!("{path}/0"),
            &token,
            "text/plain",
            b"abc".to_vec()
        )
        .await?,
        axum::http::StatusCode::CREATED
    );
    assert_eq!(
        raw(
            &fixture,
            "PUT",
            &format!("{path}/1"),
            &token,
            "text/plain",
            b"de".to_vec()
        )
        .await?,
        axum::http::StatusCode::FORBIDDEN
    );
    assert_eq!(
        raw(
            &fixture,
            "PUT",
            &format!("{path}/1"),
            &token,
            "text/plain",
            b"d".to_vec()
        )
        .await?,
        axum::http::StatusCode::CREATED
    );
    assert_eq!(
        raw(
            &fixture,
            "PUT",
            &format!("{path}/1"),
            &token,
            "text/plain",
            b"d".to_vec()
        )
        .await?,
        axum::http::StatusCode::OK
    );
    let wrong = format!("/api/v1/internal/logs/{}/segments/0", version.id + 1);
    assert!(
        raw(&fixture, "PUT", &wrong, &token, "text/plain", b"x".to_vec())
            .await?
            .is_client_error()
    );
    let sealed = fixture
        .ctx
        .post(
            &format!("/api/v1/internal/logs/{}/seal", version.id),
            json!({}),
            Some(&token),
        )
        .await?;
    assert_eq!(
        sealed.status(),
        StatusCode::OK,
        "seal error: {}",
        sealed.text().await?
    );
    let mut conn = fixture.ctx.pool.acquire().await?;
    let retained = ArtifactVersionRepository::find_by_id(&mut *conn, version.id)
        .await?
        .unwrap();
    assert_eq!(retained.body_state, Some(ArtifactBodyState::Ready));
    assert_eq!(retained.size_bytes, Some(4));
    // Current-attempt replacement retires even a still-unexpired delivery token.
    sqlx::query("UPDATE runtime_authority_execution_fence SET attempt=$2 WHERE execution=$1")
        .bind(context.execution)
        .bind(Uuid::new_v4())
        .execute(&mut *conn)
        .await?;
    assert!(WorkerDeliveryRepository::require(
        &mut conn,
        permit.id,
        context.worker,
        context.worker_instance,
        version.id
    )
    .await
    .is_err());
    drop(conn);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn revocation_between_external_put_and_ready_commit_denies_final_publication() -> Result<()> {
    paused_publication_is_revalidated(false).await
}

#[tokio::test]
async fn generic_read_between_external_put_and_ready_commit_denies_process_file_effect(
) -> Result<()> {
    paused_publication_is_revalidated(true).await
}

#[tokio::test]
async fn worker_file_approval_expiry_after_source_before_version_lock_denies_put_and_ready(
) -> Result<()> {
    approval_expiry_after_version_wait(true, true).await
}

#[tokio::test]
async fn worker_file_live_and_unbounded_approvals_survive_the_same_version_wait() -> Result<()> {
    approval_expiry_after_version_wait(false, true).await?;
    approval_expiry_after_version_wait(false, false).await
}

async fn approval_expiry_after_version_wait(expire: bool, bound_approval: bool) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::time::Duration;
    let deadline = chrono::Utc::now() + chrono::Duration::seconds(30);
    let fixture = fixture_with_approval_expiry(
        false,
        false,
        &["read", "decrypt"],
        &[],
        &[
            json!({"resource":"artifacts","actions":["read","decrypt","create","update"]}),
            json!({"resource":"executions","actions":["read","decrypt"]}),
        ],
        bound_approval.then_some(deadline),
    )
    .await?;
    let context = delivery(&fixture).await?;
    let allocated = fixture
        .ctx
        .post(
            "/api/v1/artifacts/ref/recorded_emitter.expiring_file/versions/file",
            json!({"scope":"action","owner":"recorded_emitter.emit","type":"file_text"}),
            Some(&fixture.token),
        )
        .await?;
    let allocated = created_file_allocation(allocated, "revocation test file allocation").await?;
    let version = allocated["data"]["id"].as_i64().unwrap();
    let mut conn = fixture.ctx.pool.acquire().await?;
    let permit = WorkerDeliveryRepository::for_version(&mut conn, version)
        .await?
        .unwrap();
    drop(conn);
    let token =
        attune_common::worker_delivery::mint(&context, &permit, &fixture.ctx.state.jwt_config)?;
    let mut blocker = fixture.ctx.pool.begin().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    ArtifactVersionRepository::find_by_id_for_update(&mut blocker, version)
        .await?
        .unwrap();
    let bytes = b"must not reach external PUT";
    let digest = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let request = Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/v1/internal/files/{}",
            allocated["data"]["file_path"].as_str().unwrap()
        ))
        .header("authorization", format!("Bearer {token}"))
        .header("content-length", bytes.len())
        .header("x-attune-sha256", digest)
        .body(Body::from(bytes.to_vec()))?;
    let app = fixture.ctx.app.clone();
    let mut upload = tokio::spawn(async move { app.oneshot(request).await });
    let blocked=tokio::time::timeout(Duration::from_secs(15),async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&fixture.ctx.pool).await?;
            if waiting {return Ok::<_,sqlx::Error>(());}
            tokio::task::yield_now().await;
        }
    }).await;
    if !matches!(&blocked, Ok(Ok(()))) {
        upload.abort();
        blocker.rollback().await?;
        let _ = upload.await;
        blocked??;
        unreachable!();
    }
    // Real target wait established above; expiry is a DB-clock condition, not a
    // fixed sleep or a process-global clock override.
    let expired = if expire {
        tokio::time::timeout(Duration::from_secs(40), async {
            loop {
                let expired: bool = sqlx::query_scalar("SELECT clock_timestamp() >= $1")
                    .bind(deadline)
                    .fetch_one(&fixture.ctx.pool)
                    .await?;
                if expired {
                    return Ok::<_, sqlx::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
    } else {
        Ok(Ok(()))
    };
    blocker.rollback().await?;
    if !matches!(&expired, Ok(Ok(()))) {
        upload.abort();
        let _ = upload.await;
        expired??;
        unreachable!();
    }
    let response = match tokio::time::timeout(Duration::from_secs(10), &mut upload).await {
        Ok(response) => response??,
        Err(error) => {
            upload.abort();
            let _ = upload.await;
            return Err(error.into());
        }
    };
    assert_eq!(
        response.status(),
        if expire {
            axum::http::StatusCode::FORBIDDEN
        } else {
            axum::http::StatusCode::CREATED
        }
    );
    let retained = ArtifactVersionRepository::find_by_id(&fixture.ctx.pool, version)
        .await?
        .unwrap();
    assert_eq!(
        retained.body_state,
        Some(if expire {
            ArtifactBodyState::Pending
        } else {
            ArtifactBodyState::Ready
        })
    );
    assert_eq!(
        attune_common::repositories::file_origin::FileOriginRepository::body(
            &fixture.ctx.pool,
            version
        )
        .await?
        .is_none(),
        expire
    );
    let key = attune_common::blob_store::ObjectKey::new(retained.object_key.unwrap())?;
    assert_eq!(
        fixture.ctx.state.blob_store.head(&key).await?.is_none(),
        expire
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn key_only_sdk_read_encrypts_public_multipart_file_and_retains_key_receipt() -> Result<()> {
    use attune_common::{
        models::OwnerType,
        repositories::{
            file_origin::FileOriginRepository,
            key::{CreateKeyInput, KeyRepository},
            runtime_read::RuntimeReadRepository,
        },
    };
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &["read", "decrypt"],
        &[],
        &[
            json!({"resource":"artifacts","actions":["read","decrypt","create","update"]}),
            json!({"resource":"executions","actions":["read","decrypt"]}),
        ],
    )
    .await?;
    let context = delivery(&fixture).await?;
    let key = KeyRepository::create(
        &fixture.ctx.pool,
        CreateKeyInput {
            local_ref: "sdk_file_key".into(),
            owner_type: OwnerType::Pack,
            owner_identity: None,
            owner_pack: Some(fixture.pack_id),
            owner_pack_ref: Some("recorded_emitter".into()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "SDK file key".into(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!("key-only-file-copy"),
        },
    )
    .await?;
    let read = fixture
        .ctx
        .get(&format!("/api/v1/keys/{}", key.r#ref), Some(&fixture.token))
        .await?;
    assert_eq!(read.status(), StatusCode::OK);
    let _: Value = read.json().await?;
    assert!(RuntimeReadRepository::references_for_executions(
        &fixture.ctx.pool,
        &[context.execution]
    )
    .await?
    .is_empty());
    let multipart=b"--file-key\r\nContent-Disposition: form-data; name=\"scope\"\r\n\r\naction\r\n--file-key\r\nContent-Disposition: form-data; name=\"owner\"\r\n\r\nrecorded_emitter.emit\r\n--file-key\r\nContent-Disposition: form-data; name=\"visibility\"\r\n\r\npublic\r\n--file-key\r\nContent-Disposition: form-data; name=\"file\"; filename=\"out.txt\"\r\nContent-Type: text/plain\r\n\r\nkey-only-file-copy\r\n--file-key--\r\n".to_vec();
    assert_eq!(
        raw(
            &fixture,
            "POST",
            "/api/v1/artifacts/ref/recorded_emitter.key_file/versions/upload",
            &fixture.token,
            "multipart/form-data; boundary=file-key",
            multipart
        )
        .await?,
        axum::http::StatusCode::CREATED
    );
    use attune_common::repositories::{artifact::ArtifactRepository, FindByRef};
    let artifact = ArtifactRepository::find_by_ref(&fixture.ctx.pool, "recorded_emitter.key_file")
        .await?
        .unwrap();
    for descriptor in [
        json!({"meta":{"copied":"key-only-file-copy"}}),
        json!({"created_by":"key-only-file-copy"}),
    ] {
        assert_eq!(
            fixture
                .ctx
                .post(
                    &format!("/api/v1/artifacts/{}/versions/file", artifact.id),
                    descriptor,
                    Some(&fixture.token)
                )
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    // A Key-only receipt must not escape through an inline/JSON file body API.
    assert_eq!(
        fixture
            .ctx
            .post(
                &format!("/api/v1/artifacts/{}/versions", artifact.id),
                json!({"content":"key-only-file-copy"}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        raw(
            &fixture,
            "PUT",
            &format!("/api/v1/artifacts/{}/data", artifact.id),
            &fixture.token,
            "application/json",
            serde_json::to_vec(&json!({"data":"key-only-file-copy"}))?
        )
        .await?,
        axum::http::StatusCode::BAD_REQUEST
    );
    let version = ArtifactVersionRepository::find_latest(&fixture.ctx.pool, artifact.id)
        .await?
        .unwrap();
    let mut conn = fixture.ctx.pool.acquire().await?;
    let source = FileOriginRepository::find(&mut conn, version.id)
        .await?
        .unwrap();
    assert!(source
        .receipts
        .to_string()
        .contains("execution_runtime_key_read"));
    assert!(!source.receipts.to_string().contains("key-only-file-copy"));
    assert_eq!(
        FileOriginRepository::body(&fixture.ctx.pool, version.id)
            .await?
            .unwrap()
            .plaintext_bytes,
        18
    );
    drop(conn);
    let download = fixture
        .ctx
        .get(
            &format!("/api/v1/artifacts/{}/download", artifact.id),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(download.bytes().await?.as_slice(), b"key-only-file-copy");
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn protected_a_to_file_b_retains_sources_encrypts_and_denies_public_bypass() -> Result<()> {
    use attune_common::{
        artifact_transport::{ArtifactFileTransport, VolumeTransport},
        repositories::{
            file_origin::FileOriginRepository,
            identity::{
                CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
                PermissionAssignmentRepository, PermissionSetRepository,
            },
            key::KeyRepository,
            runtime_read::RuntimeReadRepository,
        },
    };
    use futures::TryStreamExt;
    for shared_staging in [false, true] {
        let fixture = fixture_with_original_input(
            false,
            false,
            &["read", "decrypt"],
            &[],
            &[
                json!({"resource":"artifacts","actions":["read","decrypt","create","update"]}),
                json!({"resource":"executions","actions":["read","decrypt"]}),
            ],
            None,
            Some(("file_source", "retained-secret-file-bytes")),
        )
        .await?;
        let context = delivery(&fixture).await?;
        let encryption = fixture
            .ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .unwrap();
        // The original input was sealed before delivery. Resolve its actual
        // owner-qualified public reference without replacing that sealed input.
        let key_ref = attune_common::key_ref::canonical_key_ref(
            attune_common::models::OwnerType::Pack,
            Some("recorded_emitter"),
            "file_source",
        )?;
        let response = fixture
            .ctx
            .get(&format!("/api/v1/keys/{key_ref}"), Some(&fixture.token))
            .await?;
        let status = response.status();
        let key: Value = response.json().await?;
        assert_eq!(
            status,
            StatusCode::OK,
            "original sealed Key API response: {key}"
        );
        assert_eq!(
            key.pointer("/data/ref").and_then(Value::as_str),
            Some(key_ref.as_str())
        );
        assert_eq!(
            key.pointer("/data/local_ref").and_then(Value::as_str),
            Some("file_source")
        );
        assert_eq!(
            key.pointer("/data/owner_pack").and_then(Value::as_i64),
            Some(fixture.pack_id)
        );
        assert_eq!(
            key.pointer("/data/encrypted").and_then(Value::as_bool),
            Some(true)
        );
        let key_id = key
            .pointer("/data/id")
            .and_then(Value::as_i64)
            .filter(|id| *id > 0)
            .ok_or("successful original Key response must contain a positive /data/id")?;
        let original_file_key =
            super::c_file_b_key_grants::original_input_key(&fixture, context.execution, key_id)
                .await?;
        let a = fixture
            .ctx
            .post(
                "/api/v1/artifacts",
                json!({"ref":"recorded_emitter.source_a","scope":"action",
            "owner":"recorded_emitter.emit","type":"progress"}),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(a.status(), StatusCode::CREATED);
        let a: Value = a.json().await?;
        let a = a["data"]["id"].as_i64().unwrap();
        let source_version = fixture
            .ctx
            .post(
                &format!("/api/v1/artifacts/{a}/versions"),
                json!({"content":{"value":"retained-secret-file-bytes"}}),
                Some(&fixture.token),
            )
            .await?;
        let source_version_status = source_version.status();
        if source_version_status != StatusCode::CREATED {
            panic!(
                "protected-file source A version failed: {source_version_status} {}",
                source_version.text().await?
            );
        }
        // Actual protected router delivery, not a fabricated provenance row.
        let read_a = fixture
            .ctx
            .get(
                &format!("/api/v1/artifacts/{a}/versions/1"),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(read_a.status(), StatusCode::OK);
        let _: Value = read_a.json().await?;
        let allocation=fixture.ctx.post("/api/v1/artifacts/ref/recorded_emitter.file_b/versions/file",
            json!({"scope":"action","owner":"recorded_emitter.emit","type":"file_text","visibility":"public"}),Some(&fixture.token)).await?;
        let b = created_file_allocation(allocation, "protected A-to-file-B allocation").await?;
        assert!(!b.to_string().contains("retained-secret-file-bytes"));
        let version = b["data"]["id"].as_i64().unwrap();
        let artifact = b["data"]["artifact"].as_i64().unwrap();
        let file_path = b["data"]["file_path"].as_str().unwrap();
        let mut conn = fixture.ctx.pool.acquire().await?;
        let reservation = FileOriginRepository::find(&mut conn, version)
            .await?
            .unwrap();
        let permit = WorkerDeliveryRepository::for_version(&mut conn, version)
            .await?
            .unwrap();
        let acquired =
            RuntimeReadRepository::references_for_executions(&mut *conn, &[context.execution])
                .await?;
        assert!(!acquired[&context.execution]
            .contains(&RuntimeReadRepository::reference(reservation.origin_read)));
        drop(conn);
        let bytes = b"retained-secret-file-bytes".to_vec();
        if shared_staging {
            VolumeTransport::new(&fixture.ctx.state.config.artifacts_dir)
                .write_file(file_path, &bytes, None)
                .await?;
            // A closed source context is not a reason to drop an existing proof.
            let mut tx = fixture.ctx.pool.begin().await?;
            RuntimeReadRepository::close_execution(&mut tx, context.execution).await?;
            tx.commit().await?;
            ExecutionRepository::update(
                &fixture.ctx.pool,
                context.execution,
                UpdateExecutionInput {
                    status: Some(ExecutionStatus::Completed),
                    ..Default::default()
                },
            )
            .await?;
            let worker = attune_common::worker_delivery::mint(
                &context,
                &permit,
                &fixture.ctx.state.jwt_config,
            )?;
            assert_eq!(
                raw(
                    &fixture,
                    "POST",
                    &format!("/api/v1/internal/artifacts/complete/{file_path}"),
                    &worker,
                    "application/json",
                    vec![]
                )
                .await?,
                axum::http::StatusCode::OK
            );
        } else {
            let path = format!("/api/v1/internal/files/{file_path}");
            assert_eq!(
                raw(
                    &fixture,
                    "PUT",
                    &path,
                    &fixture.token,
                    "text/plain",
                    bytes.clone()
                )
                .await?,
                axum::http::StatusCode::CREATED
            );
            // Byte-identical retry reuses authenticated ciphertext, not a new nonce.
            assert_eq!(
                raw(
                    &fixture,
                    "PUT",
                    &path,
                    &fixture.token,
                    "text/plain",
                    bytes.clone()
                )
                .await?,
                axum::http::StatusCode::CREATED
            );
            assert_eq!(
                raw(
                    &fixture,
                    "PUT",
                    &path,
                    &fixture.token,
                    "text/plain",
                    b"replacement".to_vec()
                )
                .await?,
                axum::http::StatusCode::CONFLICT
            );
            let download = fixture
                .ctx
                .get(
                    &format!("/api/v1/artifacts/{artifact}/download"),
                    Some(&fixture.token),
                )
                .await?;
            let download = ready_file_response(download, "File-B process Ready download").await?;
            assert_eq!(download.bytes().await?.as_slice(), bytes.as_slice());
            // The internal entry must reach the same protected reader BEFORE
            // the legacy transfer gate rejects all recorded reads.
            let internal = fixture.ctx.get(&path, Some(&fixture.token)).await?;
            assert_eq!(internal.status(), StatusCode::OK);
            assert_eq!(internal.bytes().await?.as_slice(), bytes.as_slice());
            for method in ["GET", "HEAD"] {
                let response = fixture
                    .ctx
                    .app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method)
                            .uri(&path)
                            .header("authorization", format!("Bearer {}", fixture.token))
                            .header("range", "bytes=0-7")
                            .body(Body::empty())?,
                    )
                    .await?;
                // HEAD authenticates the whole file, while GET supports plaintext ranges.
                if method == "GET" {
                    assert_eq!(response.status(), axum::http::StatusCode::PARTIAL_CONTENT);
                    assert_eq!(
                        axum::body::to_bytes(response.into_body(), 1024)
                            .await?
                            .as_ref(),
                        &bytes[..8]
                    );
                } else {
                    assert_eq!(response.status(), axum::http::StatusCode::OK);
                    assert_eq!(
                        response.headers()["content-length"].to_str()?,
                        bytes.len().to_string()
                    );
                    assert!(axum::body::to_bytes(response.into_body(), 1024)
                        .await?
                        .is_empty());
                }
            }
            // Reading immutable B adds a later receipt; it must not relabel B
            // or reject a byte-identical Ready replay as a new tainted write.
            assert_eq!(
                raw(
                    &fixture,
                    "PUT",
                    &path,
                    &fixture.token,
                    "text/plain",
                    bytes.clone()
                )
                .await?,
                axum::http::StatusCode::CREATED
            );
            let mut conn = fixture.ctx.pool.acquire().await?;
            let unchanged = FileOriginRepository::find(&mut conn, version)
                .await?
                .unwrap();
            assert_eq!(unchanged.origin_read, reservation.origin_read);
            assert_eq!(unchanged.source_digest, reservation.source_digest);
        }
        let ready = ArtifactVersionRepository::find_by_id(&fixture.ctx.pool, version)
            .await?
            .unwrap();
        let proof = FileOriginRepository::body(&fixture.ctx.pool, version)
            .await?
            .unwrap();
        // Even legacy payload-bearing models cannot smuggle file bytes through
        // detail/history serializers. No DB write or read receipt is needed.
        let mut unsafe_projection = ready.clone();
        unsafe_projection.content_json = Some(json!({"secret":"retained-secret-file-bytes"}));
        unsafe_projection.meta = unsafe_projection.content_json.clone();
        unsafe_projection.created_by = Some("retained-secret-file-bytes".into());
        let summary =
            attune_api::dto::artifact::ArtifactVersionSummary::from(unsafe_projection.clone());
        assert!(summary.created_by.is_none());
        unsafe_projection.file_path = None;
        unsafe_projection.object_key = None;
        let projection =
            attune_api::dto::artifact::ArtifactVersionResponse::projection(unsafe_projection, true);
        assert!(projection.content_json.is_none());
        assert!(projection.meta.is_none());
        assert!(projection.created_by.is_none());
        assert_eq!(
            ready.sha256.as_deref(),
            Some(proof.ciphertext_sha256.as_str())
        );
        let key_object =
            attune_common::blob_store::ObjectKey::new(ready.object_key.clone().unwrap())?;
        let provider = attune_common::blob_store::ProviderVersion::from_stored(
            ready.provider_version.clone().unwrap(),
        )?;
        let stored: Vec<_> = fixture
            .ctx
            .state
            .blob_store
            .get(&key_object, &provider, None)
            .await?
            .try_collect()
            .await?;
        let stored = stored
            .into_iter()
            .flat_map(|chunk| chunk.to_vec())
            .collect::<Vec<u8>>();
        assert!(!String::from_utf8_lossy(&stored).contains("retained-secret-file-bytes"));
        assert_eq!(
            attune_common::runtime_file_body::open(
                &stored,
                &proof.manifest(),
                &proof.authenticated_manifest,
                encryption
            )?,
            bytes
        );

        let viewer = IdentityRepository::create(
            &fixture.ctx.pool,
            CreateIdentityInput {
                login: "file_viewer".into(),
                display_name: None,
                password_hash: None,
                attributes: json!({}),
            },
        )
        .await?;
        let grants=PermissionSetRepository::create(&fixture.ctx.pool,CreatePermissionSetInput {r#ref:"fixture.file_viewer".into(),pack:None,pack_ref:None,label:None,description:None,
            grants:json!([{"resource":"artifacts","actions":["read","decrypt"]},{"resource":"executions","actions":["read","decrypt"]}])}).await?;
        PermissionAssignmentRepository::create(
            &fixture.ctx.pool,
            CreatePermissionAssignmentInput {
                identity: viewer.id,
                permset: grants.id,
            },
        )
        .await?;
        let viewer_token = attune_common::auth::jwt::generate_access_token(
            viewer.id,
            &viewer.login,
            &fixture.ctx.state.jwt_config,
        )?;
        super::c_file_b_key_grants::refresh_and_observe(
            &fixture,
            &viewer_token,
            &original_file_key,
            super::c_file_b_key_grants::FileKeyRights::None,
        )
        .await?;
        // Public classification cannot bypass source grants, either by version,
        // latest download, or the internal path used by the SDK.
        for path in [
            format!("/api/v1/artifacts/{artifact}/download"),
            format!("/api/v1/artifacts/{artifact}/versions/1/download"),
            format!("/api/v1/internal/files/{file_path}"),
        ] {
            assert!(!fixture
                .ctx
                .get(&path, Some(&viewer_token))
                .await?
                .status()
                .is_success());
            let anonymous = Request::builder().uri(path.as_str()).body(Body::empty())?;
            assert!(!fixture
                .ctx
                .app
                .clone()
                .oneshot(anonymous)
                .await?
                .status()
                .is_success());
        }
        // Current Key metadata becoming public/plain cannot weaken the captured
        // encrypted Key origin. Read-only current grants still cannot decrypt B.
        KeyRepository::update(
            &fixture.ctx.pool,
            key_id,
            attune_common::repositories::key::UpdateKeyInput {
                encrypted: Some(false),
                value: Some(json!("new public value")),
                ..Default::default()
            },
        )
        .await?;
        let read_only = PermissionSetRepository::create(
            &fixture.ctx.pool,
            CreatePermissionSetInput {
                r#ref: "fixture.file_key_read_only".into(),
                pack: None,
                pack_ref: None,
                label: None,
                description: None,
                grants: json!([{"resource":"keys","actions":["read"]}]),
            },
        )
        .await?;
        PermissionAssignmentRepository::create(
            &fixture.ctx.pool,
            CreatePermissionAssignmentInput {
                identity: viewer.id,
                permset: read_only.id,
            },
        )
        .await?;
        super::c_file_b_key_grants::refresh_and_observe(
            &fixture,
            &viewer_token,
            &original_file_key,
            super::c_file_b_key_grants::FileKeyRights::Read,
        )
        .await?;
        assert!(!fixture
            .ctx
            .get(
                &format!("/api/v1/artifacts/{artifact}/download"),
                Some(&viewer_token)
            )
            .await?
            .status()
            .is_success());
        let key_grants = PermissionSetRepository::create(
            &fixture.ctx.pool,
            CreatePermissionSetInput {
                r#ref: "fixture.file_key_viewer".into(),
                pack: None,
                pack_ref: None,
                label: None,
                description: None,
                grants: json!([{"resource":"keys","actions":["read","decrypt"]}]),
            },
        )
        .await?;
        let source_grant = PermissionAssignmentRepository::create(
            &fixture.ctx.pool,
            CreatePermissionAssignmentInput {
                identity: viewer.id,
                permset: key_grants.id,
            },
        )
        .await?;
        super::c_file_b_key_grants::refresh_and_observe(
            &fixture,
            &viewer_token,
            &original_file_key,
            super::c_file_b_key_grants::FileKeyRights::ReadDecrypt,
        )
        .await?;
        let path = format!("/api/v1/artifacts/{artifact}/download");
        let allowed = ready_file_response(
            fixture.ctx.get(&path, Some(&viewer_token)).await?,
            "File-B human Ready download after original encrypted Key metadata becomes public",
        )
        .await?;
        assert_eq!(allowed.bytes().await?.as_slice(), bytes.as_slice());
        if shared_staging {
            use attune_common::repositories::file_evidence_retention::FileEvidenceRetentionRepository as Evidence;
            sqlx::query("UPDATE file_evidence_owner SET evidence_state='pending_audit',diagnostic='upgrade audit pending' WHERE version=$1")
                .bind(version).execute(&fixture.ctx.pool).await?;
            let quarantined = fixture.ctx.get(&path, Some(&viewer_token)).await?;
            assert!(
                !quarantined.status().is_success(),
                "quarantine is not historical disclosure authority"
            );
            assert!(
                Evidence::readmit(&fixture.ctx.pool, version).await?,
                "complete saved proofs must support positive historical readmission"
            );
            // The completed producer and its ancestors are retained evidence,
            // not a terminal credential exception. Historical disclosure works
            // after REAL owner GC attempts, rather than merely failing closed.
            assert!(!ExecutionRepository::delete(&fixture.ctx.pool, context.execution).await?);
            let removed = sqlx::query("DELETE FROM runtime_authority_admission WHERE id=$1")
                .bind(context.admission)
                .execute(&fixture.ctx.pool)
                .await?;
            assert_eq!(removed.rows_affected(), 0);
            let historical = fixture.ctx.get(&path, Some(&viewer_token)).await?;
            assert_eq!(historical.status(), StatusCode::OK);
            assert_eq!(historical.bytes().await?.as_slice(), bytes.as_slice());
            assert!(!fixture
                .ctx
                .get(&path, Some(&fixture.token))
                .await?
                .status()
                .is_success());
        }
        // Revoke the ORIGINAL encrypted source grant. Read-only current Key
        // permission cannot replace it, even though the current Key is plaintext.
        PermissionAssignmentRepository::delete(&fixture.ctx.pool, source_grant.id).await?;
        super::c_file_b_key_grants::refresh_and_observe(
            &fixture,
            &viewer_token,
            &original_file_key,
            super::c_file_b_key_grants::FileKeyRights::Read,
        )
        .await?;
        KeyRepository::delete(&fixture.ctx.pool, key_id).await?;
        assert!(!fixture
            .ctx
            .get(&path, Some(&viewer_token))
            .await?
            .status()
            .is_success());
        if !shared_staging {
            assert!(!fixture
                .ctx
                .get(&path, Some(&fixture.token))
                .await?
                .status()
                .is_success());
            let before = RuntimeReadRepository::references_for_executions(
                &fixture.ctx.pool,
                &[context.execution],
            )
            .await?;
            assert_eq!(
                raw(
                    &fixture,
                    "PUT",
                    &format!("/api/v1/artifacts/{artifact}"),
                    &fixture.token,
                    "application/json",
                    serde_json::to_vec(
                        &json!({"description":{"op":"set","value":"metadata-only change"}})
                    )?
                )
                .await?,
                axum::http::StatusCode::OK
            );
            assert_eq!(
                before,
                RuntimeReadRepository::references_for_executions(
                    &fixture.ctx.pool,
                    &[context.execution]
                )
                .await?
            );
        }
        let mut conn = fixture.ctx.pool.acquire().await?;
        assert!(sqlx::query(
            "UPDATE artifact_file_body SET origin_read=origin_read+1 WHERE artifact_version=$1"
        )
        .bind(version)
        .execute(&mut *conn)
        .await
        .is_err());
        assert!(
            sqlx::query("UPDATE artifact_version SET content='plaintext'::bytea WHERE id=$1")
                .bind(version)
                .execute(&mut *conn)
                .await
                .is_err()
        );
        drop(conn);
        assert!(ArtifactVersionRepository::delete(&fixture.ctx.pool, version).await?);
        assert!(
            sqlx::query("UPDATE artifact_version SET file_path=$1 WHERE artifact=$2")
                .bind(file_path)
                .bind(a)
                .execute(&fixture.ctx.pool)
                .await
                .is_err()
        );
        let staging = VolumeTransport::new(&fixture.ctx.state.config.artifacts_dir);
        // Simulate plaintext left by native staging after metadata cleanup.
        staging.write_file(file_path, &bytes, None).await?;
        let orphans = FileOriginRepository::orphaned_staging(&fixture.ctx.pool, 100).await?;
        assert!(orphans
            .iter()
            .any(|(path, id)| path == file_path && *id == version));
        for method in ["GET", "HEAD"] {
            let response = fixture
                .ctx
                .app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(format!("/api/v1/internal/files/{file_path}"))
                        .header("authorization", format!("Bearer {viewer_token}"))
                        .body(Body::empty())?,
                )
                .await?;
            assert!(!response.status().is_success());
        }
        staging.delete_file(file_path).await?;
        FileOriginRepository::release_staging(&fixture.ctx.pool, file_path, version).await?;
        let retained:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM file_evidence_owner WHERE version=$1 AND metadata_gone AND staging_done AND NOT object_done)")
            .bind(version).fetch_one(&fixture.ctx.pool).await?;
        assert!(
            retained,
            "metadata/native staging cleanup cannot release object obligations"
        );
        let ledger: i64 = sqlx::query_scalar(
            "SELECT id FROM object_maintenance_ledger WHERE object_key=$1 AND provider_version=$2",
        )
        .bind(key_object.as_str())
        .bind(provider.as_stored())
        .fetch_one(&fixture.ctx.pool)
        .await?;
        sqlx::query("UPDATE object_maintenance_ledger SET state='deleting' WHERE id=$1")
            .bind(ledger)
            .execute(&fixture.ctx.pool)
            .await?;
        let failing = PauseAfterPut {
            inner: fixture.ctx.state.blob_store.clone(),
            uploaded: tokio::sync::Notify::new(),
            finish: tokio::sync::Semaphore::new(0),
            fail_delete_once: std::sync::atomic::AtomicBool::new(true),
        };
        use attune_common::blob_store::BlobStore;
        assert!(failing.delete(&key_object, &provider).await.is_err());
        attune_common::repositories::object_maintenance::ObjectMaintenanceRepository::record_failure(&fixture.ctx.pool,ledger,"injected provider failure").await?;
        assert!(!ExecutionRepository::delete(&fixture.ctx.pool, context.execution).await?);
        // Retry performs actual exact-provider deletion, then the production
        // completion hook. Only now may the durable owner and its pins go.
        failing.delete(&key_object, &provider).await?;
        assert!(attune_common::repositories::object_maintenance::ObjectMaintenanceRepository::complete_delete(&fixture.ctx.pool,ledger).await?);
        let retained: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM file_evidence_owner WHERE version=$1)")
                .bind(version)
                .fetch_one(&fixture.ctx.pool)
                .await?;
        assert!(!retained);
        assert!(ExecutionRepository::delete(&fixture.ctx.pool, context.execution).await?);
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn protected_file_chunks_and_successful_eof_recheck_source_and_ready_pin() -> Result<()> {
    use futures::TryStreamExt;
    for barrier in ["next_chunk", "eof", "cleanup"] {
        let fixture = fixture_with_additional_grants(
            false,
            false,
            &["read", "decrypt"],
            &[],
            &[
                json!({"resource":"artifacts","actions":["read","decrypt","create","update"]}),
                json!({"resource":"executions","actions":["read","decrypt"]}),
            ],
        )
        .await?;
        let _context = delivery(&fixture).await?;
        let allocated = fixture
            .ctx
            .post(
                "/api/v1/artifacts/ref/recorded_emitter.chunked_file/versions/file",
                json!({"scope":"action","owner":"recorded_emitter.emit","type":"file_text"}),
                Some(&fixture.token),
            )
            .await?;
        let allocated =
            created_file_allocation(allocated, "protected chunk/EOF file allocation").await?;
        let version = allocated["data"]["id"].as_i64().unwrap();
        let path = format!(
            "/api/v1/internal/files/{}",
            allocated["data"]["file_path"].as_str().unwrap()
        );
        let bytes = vec![b'x'; if barrier == "eof" { 32 } else { 131073 }];
        assert_eq!(
            raw(
                &fixture,
                "PUT",
                &path,
                &fixture.token,
                "text/plain",
                bytes.clone()
            )
            .await?,
            axum::http::StatusCode::CREATED
        );
        let response = fixture
            .ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .header("authorization", format!("Bearer {}", fixture.token))
                    .body(Body::empty())?,
            )
            .await?;
        if response.status() != axum::http::StatusCode::OK {
            let status = response.status();
            let body = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                axum::body::to_bytes(response.into_body(), 64 * 1024),
            )
            .await??;
            panic!(
                "protected chunk/EOF {barrier} initial Ready GET: {status}; body={}",
                String::from_utf8_lossy(&body)
            );
        }
        assert!(!response.headers().contains_key("content-length"));
        let mut body = response.into_body().into_data_stream();
        let first = body
            .try_next()
            .await?
            .expect("first authenticated plaintext chunk");
        assert!(!first.is_empty());
        assert!(first.len() <= 64 * 1024);
        assert_eq!(first.as_ref(), &bytes[..first.len()]);
        if barrier == "cleanup" {
            assert!(attune_common::repositories::storage_maintenance::StorageMaintenanceRepository::mark_deleting(&fixture.ctx.pool,version).await?);
        } else {
            let mut tx = fixture.ctx.pool.begin().await?;
            RuntimeAuthorityRepository::revoke_admission(&mut tx, fixture.credential.admission_id)
                .await?;
            tx.commit().await?;
        }
        // EOF is an authority boundary too. A buffered plaintext remainder or
        // an already authenticated whole file does not survive this commit.
        assert!(
            body.try_next().await.is_err(),
            "barrier {barrier} must deny"
        );
        drop(body);
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

async fn paused_publication_is_revalidated(process_read: bool) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::{sync::Arc, time::Duration};
    let fixture = fixture(true).await?;
    let context = delivery(&fixture).await?;
    let allocation = fixture
        .ctx
        .post(
            "/api/v1/artifacts/ref/recorded_emitter.revoked_upload/versions/file",
            json!({"scope":"action","owner":"recorded_emitter.emit","type":"file_text"}),
            Some(&fixture.token),
        )
        .await?;
    let body =
        created_file_allocation(allocation, "captured-read frontier file allocation").await?;
    let version = body["data"]["id"].as_i64().unwrap();
    let mut conn = fixture.ctx.pool.acquire().await?;
    let permit = WorkerDeliveryRepository::for_version(&mut conn, version)
        .await?
        .unwrap();
    drop(conn);
    let token = if process_read {
        fixture.token.clone()
    } else {
        attune_common::worker_delivery::mint(&context, &permit, &fixture.ctx.state.jwt_config)?
    };
    let store = Arc::new(PauseAfterPut {
        inner: fixture.ctx.state.blob_store.clone(),
        uploaded: tokio::sync::Notify::new(),
        finish: tokio::sync::Semaphore::new(0),
        fail_delete_once: std::sync::atomic::AtomicBool::new(false),
    });
    let mut state = (*fixture.ctx.state).clone();
    state.blob_store = store.clone();
    let app = axum::Router::new()
        .nest("/api/v1", attune_api::routes::internal_files::routes())
        .with_state(Arc::new(state));
    let bytes = b"uploaded but not published";
    let digest = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let request = Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/v1/internal/files/{}",
            body["data"]["file_path"].as_str().unwrap()
        ))
        .header("authorization", format!("Bearer {token}"))
        .header("content-length", bytes.len())
        .header("x-attune-sha256", digest)
        .body(Body::from(bytes.to_vec()))?;
    let mut upload = tokio::spawn(app.oneshot(request));
    tokio::select! {
        result = &mut upload => {
            store.finish.add_permits(1);
            let response = result??;
            let status = response.status();
            let body = tokio::time::timeout(Duration::from_secs(5), axum::body::to_bytes(response.into_body(), 64 * 1024)).await??;
            fixture.ctx.cleanup().await?;
            return Err(format!("file upload returned before external PUT barrier: HTTP {status}; body={}", String::from_utf8_lossy(&body)).into());
        }
        result = tokio::time::timeout(Duration::from_secs(10), store.uploaded.notified()) => {
            if let Err(error) = result {
                store.finish.add_permits(1);
                upload.abort();
                let _ = upload.await;
                fixture.ctx.cleanup().await?;
                return Err(format!("file upload remained pending without reaching external PUT barrier: {error}").into());
            }
        }
    }
    // This commit must finish while the external PUT result is withheld. Holding
    // the reserve transaction through storage I/O would deadlock this barrier.
    let revoked = tokio::time::timeout(Duration::from_secs(10), async {
        let mut tx = fixture.ctx.pool.begin().await?;
        if process_read {
            use attune_common::repositories::runtime_read::{
                RuntimeReadOrigin, RuntimeReadRepository, RuntimeReadTarget,
            };
            let authority = RuntimeAuthorityRepository::validate_execution_credential(
                &mut tx,
                &fixture.credential,
            )
            .await?;
            RuntimeReadRepository::record(
                &mut tx,
                &authority,
                RuntimeReadOrigin {
                    target: RuntimeReadTarget::Execution {
                        execution: fixture.credential.execution_id,
                    },
                    provenance: vec![],
                    parents: vec![],
                },
            )
            .await?;
        } else {
            RuntimeAuthorityRepository::revoke_admission(&mut tx, fixture.credential.admission_id)
                .await?;
        }
        tx.commit().await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await;
    store.finish.add_permits(1);
    let response = match tokio::time::timeout(Duration::from_secs(10), &mut upload).await {
        Ok(result) => result??,
        Err(error) => {
            upload.abort();
            let _ = upload.await;
            return Err(error.into());
        }
    };
    revoked??;
    assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
    let retained = ArtifactVersionRepository::find_by_id(&fixture.ctx.pool, version)
        .await?
        .unwrap();
    assert_eq!(retained.body_state, Some(ArtifactBodyState::Pending));
    drop(store);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn encrypted_terminal_logs_require_a_complete_authenticated_manifest_on_both_backends(
) -> Result<()> {
    use attune_common::{
        artifact_transport::{ArtifactFileTransport, VolumeTransport},
        models::LogStreamBackend,
        repositories::{
            log_stream::LogStreamRepository, runtime_log_completion::RuntimeLogCompletionRepository,
        },
        runtime_log_completion::FrameTotals,
    };
    for backend in [
        LogStreamBackend::ObjectSegments,
        LogStreamBackend::SharedFile,
    ] {
        let fixture = fixture(true).await?;
        let context = delivery(&fixture).await?;
        let created = fixture
            .ctx
            .post(
                "/api/v1/artifacts",
                json!({"ref":"recorded_emitter.encrypted.stdout.log",
            "scope":"action","owner":"recorded_emitter.emit","type":"file_text"}),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(created.status(), StatusCode::CREATED);
        let body: Value = created.json().await?;
        let mut tx = fixture.ctx.pool.begin().await?;
        let (_, effect) =
            WorkerDeliveryRepository::require_launch_context(&mut tx, &context).await?;
        let version = ArtifactVersionRepository::create_log_pending_with_conn(
            &mut tx,
            body["data"]["id"].as_i64().unwrap(),
            "recorded_emitter.encrypted.stdout.log",
            backend,
            context.execution,
            json!({"stream":"stdout"}),
        )
        .await?;
        ArtifactVersionRepository::mark_encrypted_log(&mut *tx, version.id).await?;
        let wire_cap = attune_common::runtime_read_log::maximum_encoded_bytes(32768)?;
        let stream = LogStreamRepository::create_with_backend_with_conn(
            &mut tx, version.id, backend, wire_cap, 500,
        )
        .await?;
        let permit = WorkerDeliveryRepository::bind_encrypted_log(
            &mut tx,
            &context,
            version.id,
            stream.id,
            wire_cap as i64,
            wire_cap as i64,
            32896,
            &effect,
        )
        .await?;
        // C intentionally closes acquisition before the platform finishes logs.
        let _ = attune_common::repositories::runtime_read::RuntimeReadRepository::close_execution(
            &mut tx,
            context.execution,
        )
        .await?;
        tx.commit().await?;
        ExecutionRepository::update(
            &fixture.ctx.pool,
            context.execution,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Completed),
                ..Default::default()
            },
        )
        .await?;
        let token =
            attune_common::worker_delivery::mint(&context, &permit, &fixture.ctx.state.jwt_config)?;
        let key = fixture
            .ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .unwrap();
        let first = attune_common::runtime_read_log::encode(
            key,
            context.execution,
            version.id,
            0,
            &vec![255; 16384],
        )?;
        let last =
            attune_common::runtime_read_log::encode(key, context.execution, version.id, 1, b"end")?;
        let mut totals = FrameTotals::default();
        totals.record(&first, 16384)?;
        totals.record(&last, 3)?;
        let manifest = totals.manifest(context.execution, version.id)?;
        let authenticated = manifest.authenticate(key)?;
        let volume = VolumeTransport::new(&fixture.ctx.state.config.artifacts_dir);
        if backend == LogStreamBackend::SharedFile {
            volume
                .write_file(
                    version.file_path.as_deref().unwrap(),
                    &first,
                    Some("text/plain"),
                )
                .await?;
        } else {
            assert_eq!(
                raw(
                    &fixture,
                    "PUT",
                    &format!("/api/v1/internal/logs/{}/segments/0", version.id),
                    &token,
                    "application/octet-stream",
                    first.clone()
                )
                .await?,
                axum::http::StatusCode::CREATED
            );
        }
        let seal = |proof: Option<String>| {
            let mut request = Request::builder()
                .method("POST")
                .uri(format!("/api/v1/internal/logs/{}/seal", version.id))
                .header("authorization", format!("Bearer {token}"));
            if let Some(proof) = proof {
                request = request.header("x-attune-log-final-manifest", proof);
            }
            fixture
                .ctx
                .app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
        };
        assert_eq!(
            seal(None).await?.status(),
            axum::http::StatusCode::FORBIDDEN
        );
        // Every stored prefix frame authenticates, but the final digest/count do not match.
        assert_eq!(
            seal(Some(authenticated.clone())).await?.status(),
            axum::http::StatusCode::FORBIDDEN
        );
        let mut conn = fixture.ctx.pool.acquire().await?;
        assert!(RuntimeLogCompletionRepository::find(&mut conn, version.id)
            .await?
            .is_none());
        assert_eq!(
            ArtifactVersionRepository::find_by_id(&mut *conn, version.id)
                .await?
                .unwrap()
                .body_state,
            Some(ArtifactBodyState::Pending)
        );
        drop(conn);
        if backend == LogStreamBackend::SharedFile {
            volume
                .append_log_file(version.file_path.as_deref().unwrap(), &last)
                .await?;
        } else {
            assert_eq!(
                raw(
                    &fixture,
                    "PUT",
                    &format!("/api/v1/internal/logs/{}/segments/1", version.id),
                    &token,
                    "application/octet-stream",
                    last.clone()
                )
                .await?,
                axum::http::StatusCode::CREATED
            );
        }
        assert_eq!(
            seal(Some(authenticated.clone())).await?.status(),
            axum::http::StatusCode::OK
        );
        assert_eq!(
            seal(Some(authenticated)).await?.status(),
            axum::http::StatusCode::OK
        );
        assert_eq!(
            seal(Some(manifest.authenticate(key)?)).await?.status(),
            axum::http::StatusCode::FORBIDDEN
        );
        let mut conn = fixture.ctx.pool.acquire().await?;
        let recorded = RuntimeLogCompletionRepository::find(&mut conn, version.id)
            .await?
            .unwrap();
        assert_eq!(recorded.stream, stream.id);
        assert_eq!(recorded.frame_count, 2);
        assert_eq!(recorded.plain_bytes, 16387);
        assert_eq!(recorded.cipher_bytes, (first.len() + last.len()) as i64);
        assert_eq!(recorded.attempt, context.attempt);
        assert_eq!(recorded.worker_instance, context.worker_instance);
        let opened = attune_common::runtime_log_completion::FinalLogManifest::open(
            key,
            &recorded.authenticated_manifest,
        )?;
        assert!(opened == manifest);
        let ready = ArtifactVersionRepository::find_by_id(&mut *conn, version.id)
            .await?
            .unwrap();
        assert_eq!(ready.body_state, Some(ArtifactBodyState::Ready));
        assert_eq!(
            ready.meta.as_ref().unwrap()["encrypted_log_manifest"].as_str(),
            Some(recorded.authenticated_manifest.as_str())
        );
        let replacement = sqlx::query("UPDATE artifact_version SET meta=jsonb_set(meta,'{encrypted_log_manifest}','\"replacement\"'::jsonb) WHERE id=$1")
            .bind(version.id).execute(&mut *conn).await;
        assert!(replacement.is_err());
        drop(conn);
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}
