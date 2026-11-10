use super::*;
use futures::FutureExt;
#[path = "object_seal_boundary.rs"]
mod gate;
use attune_common::{
    models::LogStreamBackend, repositories::log_stream::LogStreamRepository,
    runtime_log_completion::FrameTotals,
};
use gate::SealGetBarrier;
use std::{sync::Arc, time::Duration};
async fn snapshot(pool: &sqlx::PgPool, stream: i64, version: i64) -> Result<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('phase',s.close_phase,'revision',s.close_revision,'input',s.close_input,'hash',s.close_input_sha256,'deadline',s.close_deadline,'sealed',s.sealed,'sealed_at',s.sealed_at,'state',v.body_state,'meta',v.meta,'size',v.size_bytes,'digest',v.sha256,'membership',(SELECT jsonb_agg(to_jsonb(x) ORDER BY x.sequence) FROM log_segment x WHERE x.stream=s.id),'evidence',(SELECT jsonb_agg(to_jsonb(e)) FROM runtime_log_final_manifest e WHERE e.artifact_version=v.id)) FROM log_stream s JOIN artifact_version v ON v.id=s.artifact_version WHERE s.id=$1 AND v.id=$2").bind(stream).bind(version).fetch_one(pool).await?)
}
async fn boundary(mode: u8) -> Result<()> {
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
    tx.commit().await?;
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
    let mut state = (*fixture.ctx.state).clone();
    state.blob_store = store.clone();
    let state = Arc::new(state);
    let app = attune_api::server::Server::new(state.clone()).router();
    if mode != 2 {
        ExecutionRepository::update(
            &fixture.ctx.pool,
            context.execution,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Completed),
                ..Default::default()
            },
        )
        .await?;
    }
    let mut claims =
        attune_common::auth::jwt::validate_token(&token, &fixture.ctx.state.jwt_config)?;
    let mut credential = token.clone();
    let uri = format!("/api/v1/internal/logs/{}/seal", version.id);
    let request = |credential: &str| {
        Request::builder()
            .method("POST")
            .uri(&uri)
            .header("authorization", format!("Bearer {credential}"))
            .header("x-attune-log-final-manifest", authenticated.clone())
            .body(Body::empty())
            .unwrap()
    };
    if mode == 1 {
        claims.exp = chrono::Utc::now().timestamp() + 4;
        credential = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(fixture.ctx.state.jwt_config.secret.as_bytes()),
        )?;
    }
    let mut http = tokio::spawn(app.clone().oneshot(request(&credential)));
    let mut joined = false;
    let result=std::panic::AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(10),store.uploaded.notified()).await?;
        let pending=snapshot(&fixture.ctx.pool,stream.id,version.id).await?;
        assert_eq!(pending["phase"],"closing");assert_eq!(pending["state"],"pending");assert_eq!(pending["sealed"],false);assert!(pending["evidence"].is_null());
        let mut blocker=None;
        if mode==0 {assert!(RuntimeAuthorityRepository::revoke_admission(&mut *fixture.ctx.pool.acquire().await?,fixture.credential.admission_id).await?);}
        if mode==1 {
            let mut tx=fixture.ctx.pool.begin().await?;
            let pid:i32=sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *tx).await?;
            sqlx::query("SELECT id FROM artifact_version WHERE id=$1 FOR UPDATE").bind(version.id).execute(&mut *tx).await?;
            blocker=Some((tx,pid));
        }
        if mode==2 {
            let binding=RuntimeAuthorityRepository::begin_execution_attempt(&mut *fixture.ctx.pool.acquire().await?,context.execution,context.worker,Uuid::new_v4()).await?;
            assert_ne!(binding.attempt,context.attempt);
        }
        store.finish.add_permits(1);
        if let Some((tx,pid))=blocker {
            tokio::time::timeout(Duration::from_secs(3),async {loop {
                let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND datid=(SELECT oid FROM pg_database WHERE datname=current_database()))").bind(pid).fetch_one(&fixture.ctx.pool).await?;
                if waiting {break Ok::<_,sqlx::Error>(());} tokio::task::yield_now().await;
            }}).await??;
            sqlx::query("SELECT pg_sleep(GREATEST(0,$1-extract(epoch FROM clock_timestamp()))::double precision)").bind(claims.exp as f64).execute(&fixture.ctx.pool).await?;
            let expired:bool=sqlx::query_scalar("SELECT extract(epoch FROM clock_timestamp()) >= $1").bind(claims.exp as f64).fetch_one(&fixture.ctx.pool).await?;assert!(expired);tx.rollback().await?;
        }
        let response=tokio::time::timeout(Duration::from_secs(10),&mut http).await???;
        joined=true;
        let status=response.status(); let body=axum::body::to_bytes(response.into_body(),65536).await?;
        assert_eq!(status,StatusCode::FORBIDDEN,"{}",String::from_utf8_lossy(&body));
        assert_eq!(snapshot(&fixture.ctx.pool,stream.id,version.id).await?,pending);
        if mode==1 {
            store.finish.add_permits(1);
            assert_eq!(app.clone().oneshot(request(&token)).await?.status(),StatusCode::OK);
            let published=snapshot(&fixture.ctx.pool,stream.id,version.id).await?;
            assert_eq!(published["state"],"ready");assert_eq!(published["phase"],"published");assert_eq!(published["sealed"],true);
            for field in ["revision","input","hash","deadline","membership"] {assert_eq!(published[field],pending[field]);}
        }
        Ok::<_,Box<dyn std::error::Error>>(())
    }).catch_unwind().await;
    store.finish.add_permits(10);
    if !joined {
        if !http.is_finished() {
            http.abort();
        }
        let _ = http.await;
    }
    let drained = fixture
        .ctx
        .state
        .file_transfers
        .drain(Duration::from_secs(5))
        .await;
    drop(app);
    drop(state);
    drop(store);
    fixture.ctx.cleanup().await?;
    assert!(drained, "owned seal drain watchdog");
    match result {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
#[tokio::test]
async fn object_seal_delivery_admission_revoked_during_get() -> Result<()> {
    boundary(0).await
}
#[tokio::test]
async fn object_seal_delivery_credential_expires_during_target_wait_and_recovers() -> Result<()> {
    boundary(1).await
}

#[tokio::test]
async fn object_seal_delivery_real_attempt_replacement_during_get_denies_old_context() -> Result<()>
{
    boundary(2).await
}
