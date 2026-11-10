use super::*;
use crate::commit_relay::{Class, Decision, OwnedCommitRelay};
use attune_common::{
    models::LogStreamBackend, repositories::log_stream::LogStreamRepository,
    runtime_log_completion::FrameTotals,
};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
struct SealGetBarrier {
    inner: std::sync::Arc<dyn attune_common::blob_store::BlobStore>,
    uploaded: tokio::sync::Notify,
    finish: tokio::sync::Semaphore,
    gets: std::sync::atomic::AtomicUsize,
}
impl std::fmt::Debug for SealGetBarrier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SealGetBarrier")
    }
}
#[async_trait::async_trait]
impl attune_common::blob_store::BlobStore for SealGetBarrier {
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
        self.gets.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.uploaded.notify_one();
        self.finish
            .acquire()
            .await
            .expect("owned GET gate")
            .forget();
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
        self.inner.delete(key, version).await
    }
}

async fn snapshot(pool: &sqlx::PgPool, stream: i64, version: i64) -> Result<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('phase',s.close_phase,'revision',s.close_revision,'input',s.close_input,'hash',s.close_input_sha256,'deadline',s.close_deadline,'sealed',s.sealed,'sealed_at',s.sealed_at,'state',v.body_state,'meta',v.meta,'size',v.size_bytes,'digest',v.sha256,'membership',(SELECT jsonb_agg(to_jsonb(x) ORDER BY x.sequence) FROM log_segment x WHERE x.stream=s.id),'evidence',(SELECT jsonb_agg(to_jsonb(e)) FROM runtime_log_final_manifest e WHERE e.artifact_version=v.id)) FROM log_stream s JOIN artifact_version v ON v.id=s.artifact_version WHERE s.id=$1 AND v.id=$2").bind(stream).bind(version).fetch_one(pool).await?)
}
async fn run(final_loss: bool, revoke: bool, twice: bool, forwarded_expiry: bool) -> Result<()> {
    let backend = LogStreamBackend::ObjectSegments;
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
    let (_, effect) = WorkerDeliveryRepository::require_launch_context(&mut tx, &context).await?;
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

    let mut totals = FrameTotals::default();
    totals.record(&first, 16384)?;
    let manifest = totals.manifest(context.execution, version.id)?;
    let authenticated = manifest.authenticate(key)?;
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
        StatusCode::CREATED
    );
    let store = Arc::new(SealGetBarrier {
        inner: fixture.ctx.state.blob_store.clone(),
        uploaded: tokio::sync::Notify::new(),
        finish: tokio::sync::Semaphore::new(0),
        gets: std::sync::atomic::AtomicUsize::new(0),
    });
    let options = fixture.ctx.pool.connect_options();
    let target = format!("{}:{}", options.get_host(), options.get_port());
    let mut relay = OwnedCommitRelay::start(
        target,
        stream.id,
        vec![
            if final_loss {
                Class::Publication
            } else {
                Class::SelectedOnly
            },
            Class::SelectedOnly,
        ],
    )
    .await?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .min_connections(0)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(
            (*options)
                .clone()
                .host("127.0.0.1")
                .port(relay.port)
                .ssl_mode(sqlx::postgres::PgSslMode::Disable),
        )
        .await?;
    let mut state = (*fixture.ctx.state).clone();
    state.db = pool.clone();
    state.blob_store = store.clone();
    let state = Arc::new(state);
    let app = attune_api::server::Server::new(state.clone()).router();
    let (token, credential_deadline) = if forwarded_expiry {
        let mut claims =
            attune_common::auth::jwt::validate_token(&token, &fixture.ctx.state.jwt_config)?;
        claims.exp = chrono::Utc::now().timestamp() + 5;
        let deadline = claims.exp;
        (
            jsonwebtoken::encode(
                &jsonwebtoken::Header::default(),
                &claims,
                &jsonwebtoken::EncodingKey::from_secret(
                    fixture.ctx.state.jwt_config.secret.as_bytes(),
                ),
            )?,
            deadline,
        )
    } else {
        (token, 0)
    };
    let request = || {
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/internal/logs/{}/seal", version.id))
            .header("authorization", format!("Bearer {token}"))
            .header("x-attune-log-final-manifest", authenticated.clone())
            .body(Body::empty())
            .unwrap()
    };
    // The process credential deliberately has no consumer declaration here.
    let denied = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/internal/logs/{}/seal", version.id))
        .header("authorization", format!("Bearer {}", fixture.token))
        .header("x-attune-log-final-manifest", authenticated.clone())
        .body(Body::empty())?;
    assert_eq!(
        app.clone().oneshot(denied).await?.status(),
        StatusCode::FORBIDDEN
    );
    let mut http = tokio::spawn(app.clone().oneshot(request()));
    let mut original_pending = None;
    if final_loss {
        tokio::time::timeout(Duration::from_secs(10), store.uploaded.notified()).await?;
        let pending = snapshot(&fixture.ctx.pool, stream.id, version.id).await?;
        assert_eq!(pending["phase"], "closing");
        assert_eq!(pending["state"], "pending");
        assert_eq!(pending["sealed"], false);
        assert!(pending["evidence"].is_null());
        original_pending = Some(pending);
        store.finish.add_permits(1);
    }
    let held = tokio::time::timeout(Duration::from_secs(10), relay.next_event()).await??;
    assert_eq!(held.status, b'I');
    assert_eq!(
        held.class,
        if final_loss {
            Class::Publication
        } else {
            Class::SelectedOnly
        }
    );
    let first_pid = held.backend_pid;
    let first_conn = held.connection;
    let committed = snapshot(&fixture.ctx.pool, stream.id, version.id).await?;
    if let Some(pending) = original_pending {
        for field in ["revision", "input", "hash", "deadline", "membership"] {
            assert_eq!(committed[field], pending[field]);
        }
    }
    assert_eq!(committed["revision"], 1);
    assert_eq!(committed["membership"].as_array().unwrap().len(), 1);
    assert_eq!(committed["membership"][0]["sequence"], 0);
    use sha2::{Digest, Sha256};
    let expected_digest = format!("{:x}", Sha256::digest(&first));
    if final_loss {
        assert_eq!(committed["digest"], expected_digest);
    }
    assert_eq!(
        committed["phase"],
        if final_loss { "published" } else { "closing" }
    );
    assert_eq!(
        committed["state"],
        if final_loss { "ready" } else { "pending" }
    );
    assert_eq!(committed["sealed"], final_loss);
    assert_eq!(store.gets.load(Ordering::SeqCst), usize::from(final_loss));
    if final_loss {
        assert_eq!(committed["size"], first.len() as i64);
        assert_eq!(committed["evidence"].as_array().unwrap().len(), 1);
    } else {
        assert!(committed["evidence"].is_null());
    }
    assert!(tokio::time::timeout(Duration::from_millis(50), &mut http)
        .await
        .is_err());
    if revoke || forwarded_expiry {
        if revoke {
            RuntimeAuthorityRepository::revoke_admission(
                &mut *fixture.ctx.pool.acquire().await?,
                fixture.credential.admission_id,
            )
            .await?;
        }
        if forwarded_expiry {
            while chrono::Utc::now().timestamp() < credential_deadline {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        held.decide(if forwarded_expiry {
            Decision::Forward
        } else {
            Decision::Disconnect
        })?;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), http)
                .await???
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            snapshot(&fixture.ctx.pool, stream.id, version.id).await?,
            committed
        );
        assert_eq!(store.gets.load(Ordering::SeqCst), usize::from(final_loss));
        drop(app);
        drop(state);
        pool.close().await;
        let report = relay.shutdown().await?;
        assert_eq!(report.held, 1);
        assert_eq!(report.disconnected, usize::from(!forwarded_expiry));
        assert_eq!(report.publications, usize::from(final_loss));
        assert_no_backends(&fixture.ctx.pool, &report.backend_pids).await?;
        fixture.ctx.cleanup().await?;
        return Ok(());
    }
    held.decide(Decision::Disconnect)?;
    let retry = tokio::time::timeout(Duration::from_secs(10), relay.next_event()).await??;
    assert_ne!(retry.backend_pid, first_pid);
    assert_ne!(retry.connection, first_conn);
    assert_eq!(retry.class, Class::SelectedOnly);
    assert_eq!(
        snapshot(&fixture.ctx.pool, stream.id, version.id).await?,
        committed
    );
    assert_eq!(store.gets.load(Ordering::SeqCst), usize::from(final_loss));
    retry.decide(if twice {
        Decision::Disconnect
    } else {
        Decision::Forward
    })?;
    if twice {
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), http)
                .await???
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            snapshot(&fixture.ctx.pool, stream.id, version.id).await?,
            committed
        );
        assert_eq!(store.gets.load(Ordering::SeqCst), 0);
        http = tokio::spawn(app.clone().oneshot(request()));
    }
    if !final_loss {
        tokio::time::timeout(Duration::from_secs(10), store.uploaded.notified()).await?;
        store.finish.add_permits(1);
    }
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), http)
            .await???
            .status(),
        StatusCode::OK
    );
    let published = snapshot(&fixture.ctx.pool, stream.id, version.id).await?;
    assert_eq!(published["phase"], "published");
    assert_eq!(published["state"], "ready");
    assert_eq!(published["sealed"], true);
    for field in ["revision", "input", "hash", "deadline", "membership"] {
        assert_eq!(published[field], committed[field]);
    }
    assert_eq!(published["evidence"].as_array().unwrap().len(), 1);
    assert_eq!(published["digest"], expected_digest);
    assert_eq!(published["size"], first.len() as i64);
    if final_loss {
        assert_eq!(published, committed);
    }
    let mut conn = fixture.ctx.pool.acquire().await?;
    let recorded =
        attune_common::repositories::runtime_log_completion::RuntimeLogCompletionRepository::find(
            &mut conn, version.id,
        )
        .await?
        .unwrap();
    assert_eq!(recorded.cipher_sha256, expected_digest);
    assert_eq!(recorded.stream, stream.id);
    assert_eq!(recorded.attempt, context.attempt);
    assert_eq!(recorded.worker_instance, context.worker_instance);
    assert_eq!(recorded.frame_count, 1);
    assert_eq!(recorded.plain_bytes, 16384);
    assert_eq!(recorded.cipher_bytes, first.len() as i64);
    assert!(
        attune_common::runtime_log_completion::FinalLogManifest::open(
            key,
            &recorded.authenticated_manifest
        )? == manifest
    );
    drop(conn);
    assert_eq!(
        app.clone().oneshot(request()).await?.status(),
        StatusCode::OK
    );
    let changed = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/internal/logs/{}/seal?truncated=true",
            version.id
        ))
        .header("authorization", format!("Bearer {token}"))
        .header("x-attune-log-final-manifest", authenticated.clone())
        .body(Body::empty())?;
    assert_eq!(
        app.clone().oneshot(changed).await?.status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        snapshot(&fixture.ctx.pool, stream.id, version.id).await?,
        published
    );
    assert_eq!(store.gets.load(Ordering::SeqCst), 1);
    drop(app);
    drop(state);
    pool.close().await;
    let report = relay.shutdown().await?;
    assert_eq!(report.held, 2);
    assert_eq!(report.disconnected, if twice { 2 } else { 1 });
    assert_eq!(report.publications, 1);
    assert_no_backends(&fixture.ctx.pool, &report.backend_pids).await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}
#[tokio::test]
async fn object_seal_prepare_commit_ack_loss_reconciles_exact_selection() -> Result<()> {
    run(false, false, false, false).await
}
#[tokio::test]
async fn object_seal_final_commit_ack_loss_reconciles_without_reverification() -> Result<()> {
    run(true, false, false, false).await
}

async fn assert_no_backends(pool: &sqlx::PgPool, pids: &[i32]) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let remaining: i64 =
                sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE pid=ANY($1)")
                    .bind(pids)
                    .fetch_one(pool)
                    .await?;
            let locks: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE pid=ANY($1)")
                .bind(pids)
                .fetch_one(pool)
                .await?;
            if remaining == 0 && locks == 0 {
                break Ok::<_, sqlx::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    Ok(())
}
#[tokio::test]
async fn object_seal_prepare_commit_ack_loss_rechecks_revoked_admission() -> Result<()> {
    run(false, true, false, false).await
}
#[tokio::test]
async fn object_seal_final_commit_ack_loss_rechecks_revoked_admission() -> Result<()> {
    run(true, true, false, false).await
}
#[tokio::test]
async fn object_seal_prepare_commit_ack_loss_twice_retains_selection_for_recovery() -> Result<()> {
    run(false, false, true, false).await
}

#[tokio::test]
async fn object_seal_prepare_commit_ack_forwarded_after_credential_expiry_is_terminal() -> Result<()>
{
    run(false, false, false, true).await
}
#[tokio::test]
async fn object_seal_final_commit_ack_forwarded_after_credential_expiry_is_terminal() -> Result<()>
{
    run(true, false, false, true).await
}
