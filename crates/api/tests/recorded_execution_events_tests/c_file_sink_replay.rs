//! Exercises the real sealed-input/file-capture boundary without rewriting F's
//! fixtures or changing accepted inputs after their seal.
use super::*;
use attune_common::{
    models::{WorkerStatus, WorkerType},
    repositories::{
        file_origin::FileOriginRepository,
        runtime::{CreateWorkerInput, WorkerRepository},
        runtime_read::RuntimeReadRepository,
        worker_delivery::WorkerDeliveryRepository,
    },
};
use axum::{body::Body, http::Request};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

async fn register_delivery(fixture: &Fixture) -> Result<()> {
    let fields = ExecutionSecretValueRepository::find_stored_by_entity(
        &fixture.ctx.pool,
        ENTITY_EXECUTION_CONFIG,
        fixture.credential.execution_id,
    )
    .await?;
    let source = fields
        .iter()
        .find(|field| field.json_path == "/input")
        .unwrap();
    let provenance: attune_common::secret_provenance::SecretProvenance =
        serde_json::from_str(source.source_ref.as_deref().unwrap())?;
    assert!(
        provenance
            .origins
            .iter()
            .any(attune_common::repositories::immutable_input_origin::is_input_receipt),
        "this regression requires the actual sealed first receipt"
    );
    let worker = WorkerRepository::create(
        &fixture.ctx.pool,
        CreateWorkerInput {
            name: "c-captured-source-worker".into(),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
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
    WorkerDeliveryRepository::register(
        &mut tx,
        fixture.credential.execution_id,
        worker.id,
        uuid::Uuid::new_v4(),
        chrono::Utc::now() + chrono::Duration::minutes(5),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn send_bytes(
    fixture: &Fixture,
    method: &str,
    path: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> Result<axum::http::StatusCode> {
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {}", fixture.token))
        .header("content-type", content_type)
        .header("content-length", bytes.len())
        .header("x-attune-sha256", digest)
        .body(Body::from(bytes))?;
    let response = fixture.ctx.app.clone().oneshot(request).await?;
    let status = response.status();
    // Drain the owned response, but never print file bytes or provenance.
    let _ = axum::body::to_bytes(response.into_body(), 64 * 1024).await?;
    Ok(status)
}

async fn get_ready_file(fixture: &Fixture, path: &str, token: &str) -> Result<Vec<u8>> {
    let request = Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())?;
    let response = fixture.ctx.app.clone().oneshot(request).await?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024).await?;
    if status != axum::http::StatusCode::OK {
        // Test-only bounded response diagnostics. Never print the bearer token
        // or successful plaintext. Production diagnostics remain static enums.
        panic!(
            "Ready file GET returned {status}; error body={}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(1024)])
        );
    }
    Ok(bytes.to_vec())
}

async fn grant_human_reader(fixture: &Fixture, name: &str, grants: Value) -> Result<()> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let set = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("fixture.c_ready_file_{name}"),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(fixture.requester)
        .await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    Ok(())
}

#[tokio::test]
async fn producer_create_only_multipart_replays_original_delivery_not_a_sink_read() -> Result<()> {
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[json!({"resource":"artifacts","actions":["create"]})],
    )
    .await?;
    register_delivery(&fixture).await?;
    let multipart = b"--c-sink\r\nContent-Disposition: form-data; name=\"scope\"\r\n\r\naction\r\n--c-sink\r\nContent-Disposition: form-data; name=\"owner\"\r\n\r\nrecorded_emitter.emit\r\n--c-sink\r\nContent-Disposition: form-data; name=\"file\"; filename=\"out.txt\"\r\nContent-Type: text/plain\r\n\r\noutput\r\n--c-sink--\r\n".to_vec();
    let status = send_bytes(
        &fixture,
        "POST",
        "/api/v1/artifacts/ref/recorded_emitter.c_create_sink/versions/upload",
        "multipart/form-data; boundary=c-sink",
        multipart,
    )
    .await?;
    assert_eq!(
        status,
        axum::http::StatusCode::CREATED,
        "an output capture must not add artifact/execution read grants to input delivery"
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn pending_file_capture_is_opaque_exact_bound_and_not_an_ordinary_pending_read() -> Result<()>
{
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[
            json!({"resource":"artifacts","actions":["create","update","read","decrypt"]}),
            json!({"resource":"executions","actions":["read","decrypt"]}),
        ],
    )
    .await?;
    register_delivery(&fixture).await?;
    let allocation = fixture
        .ctx
        .post(
            "/api/v1/artifacts/ref/recorded_emitter.c_pending_sink/versions/file",
            json!({"scope":"action","owner":"recorded_emitter.emit","type":"file_text"}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(allocation.status(), StatusCode::CREATED);
    let allocated: Value = allocation.json().await?;
    let version = allocated["data"]["id"].as_i64().unwrap();
    let mut tx = fixture.ctx.pool.begin().await?;
    let saved = FileOriginRepository::find(&mut tx, version).await?.unwrap();
    let capture = RuntimeReadRepository::file_sink_capture(&mut tx, saved.origin_read)
        .await?
        .unwrap();
    assert!(capture.is_for(&fixture.credential));
    assert!(capture
        .rows()
        .iter()
        .any(|row| row.entity_type == ENTITY_EXECUTION_CONFIG && row.json_path == "/input"));
    let mut wrong = fixture.credential.clone();
    wrong.attempt = uuid::Uuid::new_v4();
    assert!(!capture.is_for(&wrong));
    wrong = fixture.credential.clone();
    wrong.admission_id += 1;
    assert!(!capture.is_for(&wrong));
    tx.rollback().await?;

    // The same serialized target/parents acquired as a normal registry read
    // cannot manufacture the file-sink role. Do this in a separate rolled-back
    // transaction so this deliberate acquisition does not alter the file flow.
    let mut tx = fixture.ctx.pool.begin().await?;
    let original = RuntimeReadRepository::find(&mut *tx, saved.origin_read).await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    let reference = RuntimeReadRepository::record(&mut tx, &authority, original).await?;
    let attune_common::secret_provenance::SecretOrigin::Entity { entity_id, .. } = reference else {
        unreachable!()
    };
    assert!(RuntimeReadRepository::file_sink_capture(&mut tx, entity_id)
        .await?
        .is_none());
    tx.rollback().await?;
    let path = format!(
        "/api/v1/internal/files/{}",
        allocated["data"]["file_path"].as_str().unwrap()
    );
    assert_eq!(
        send_bytes(&fixture, "PUT", &path, "text/plain", b"output".to_vec()).await?,
        axum::http::StatusCode::CREATED
    );
    // Publication success is not GET success. This header is the admitted
    // process token, deliberately NOT fixture.ctx.token's human Access token.
    assert_eq!(
        get_ready_file(&fixture, &path, &fixture.token)
            .await?
            .as_slice(),
        b"output"
    );

    let human = fixture.ctx.token.as_deref().unwrap();
    assert_eq!(
        fixture.ctx.get(&path, Some(human)).await?.status(),
        StatusCode::FORBIDDEN
    );
    grant_human_reader(&fixture, "without_source_decrypt", json!([
        {"resource":"artifacts","actions":["read","decrypt"],"constraints":{"refs":["recorded_emitter.c_pending_sink"]}},
        {"resource":"executions","actions":["read"],"constraints":{"pack_refs":["recorded_emitter"]}}
    ])).await?;
    assert_eq!(
        fixture.ctx.get(&path, Some(human)).await?.status(),
        StatusCode::FORBIDDEN
    );
    grant_human_reader(&fixture, "source_decrypt", json!([
        {"resource":"executions","actions":["decrypt"],"constraints":{"pack_refs":["recorded_emitter"]}}
    ])).await?;
    assert_eq!(
        get_ready_file(&fixture, &path, human).await?.as_slice(),
        b"output"
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}
