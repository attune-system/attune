//! Real TCP pressure on the stock Axum/Hyper router. Counts are application Body
//! handoffs, not kernel writes. Bytes already handed to Hyper may remain queued.
use super::worker_live_streams::{worker_fixture, WorkerLog};
use super::*;
use attune_common::{models::LogStreamBackend, repositories::artifact::ArtifactVersionRepository};
use axum::body::Body;
use futures::StreamExt;
use std::{
    os::fd::AsRawFd,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct TcpRouter {
    address: std::net::SocketAddr,
    handed_bytes: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for TcpRouter {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn small_buffer(fd: std::os::fd::RawFd, option: libc::c_int) -> Result<()> {
    let bytes: libc::c_int = 4096;
    // Owned test sockets only. Linux inherits SO_SNDBUF on accepted sockets.
    let result = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            option,
            (&bytes as *const libc::c_int).cast(),
            std::mem::size_of_val(&bytes) as libc::socklen_t,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

impl TcpRouter {
    async fn start(fixture: &Fixture) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        small_buffer(listener.as_raw_fd(), libc::SO_SNDBUF)?;
        let address = listener.local_addr()?;
        let handed_bytes = Arc::new(AtomicUsize::new(0));
        let observe = handed_bytes.clone();
        let app = fixture
            .ctx
            .app
            .clone()
            .layer(axum::middleware::map_response(
                move |response: axum::response::Response| {
                    let observe = observe.clone();
                    async move {
                        let (parts, body) = response.into_parts();
                        let stream = body.into_data_stream().inspect(move |chunk| {
                            if let Ok(bytes) = chunk {
                                observe.fetch_add(bytes.len(), Ordering::SeqCst);
                            }
                        });
                        // Synchronous observation only. No new await between production
                        // authorization and returning this frame to Hyper.
                        axum::response::Response::from_parts(parts, Body::from_stream(stream))
                    }
                },
            ));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Ok(Self {
            address,
            handed_bytes,
            task,
        })
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        token: &str,
    ) -> Result<tokio::net::TcpStream> {
        let mut socket = tokio::net::TcpStream::connect(self.address).await?;
        small_buffer(socket.as_raw_fd(), libc::SO_RCVBUF)?;
        socket.write_all(format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n", self.address).as_bytes()).await?;
        Ok(socket)
    }

    async fn pressure(&self, total_plaintext: usize) -> Result<usize> {
        Ok(
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                loop {
                    let before = self.handed_bytes.load(Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    let after = self.handed_bytes.load(Ordering::SeqCst);
                    if before > 0 && after == before && after < total_plaintext {
                        return after;
                    }
                }
            })
            .await?,
        )
    }
}

async fn tcp_pressure_case(cancel: bool) -> Result<()> {
    let fixture = worker_fixture().await?;
    let mut log = WorkerLog::allocate(&fixture, LogStreamBackend::SharedFile).await?;
    let mut prefix = vec![b'p'; 16 * 1024];
    prefix[..13].copy_from_slice(b"queued-prefix");
    for _ in 0..128 {
        log.append(&fixture, &prefix).await?;
    }
    let server = TcpRouter::start(&fixture).await?;
    let socket = server
        .request(
            "GET",
            &format!("/api/v1/executions/{}/logs/stdout/stream", log.execution),
            &fixture.token,
        )
        .await?;
    let queued = server.pressure(128 * prefix.len()).await?;
    assert!(
        queued > 0 && queued < 128 * prefix.len(),
        "the paused real socket leaves application chunks unyielded"
    );
    if cancel {
        drop(socket);
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                fixture.ctx.state.execution_log_streams.drain_releases().await;
                let leases: i64 = sqlx::query_scalar("SELECT count(*) FROM execution_log_stream_lease WHERE expires_at>clock_timestamp()")
                    .fetch_one(&fixture.ctx.pool).await.unwrap();
                if leases == 0 { break; }
                tokio::task::yield_now().await;
            }
        }).await?;
        let stopped = server.handed_bytes.load(Ordering::SeqCst);
        log.append(&fixture, b"cancelled-stream-must-not-be-polled")
            .await?;
        assert_eq!(server.handed_bytes.load(Ordering::SeqCst), stopped);
    } else {
        let mut tx = fixture.ctx.pool.begin().await?;
        RuntimeAuthorityRepository::revoke_admission(&mut tx, fixture.credential.admission_id)
            .await?;
        tx.commit().await?;
        // This frame did not exist when any earlier chunk was authorized. The
        // independent producer remains valid, but the reader cannot acquire it.
        log.append(&fixture, b"TCP-UNYIELDED-AFTER-REVOCATION-MUST-NOT-ESCAPE")
            .await?;
        let mut bytes = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            socket.take(4 * 1024 * 1024).read_to_end(&mut bytes),
        )
        .await??;
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("HTTP/1.1 200"));
        assert!(
            text.contains("queued-prefix"),
            "preauthorized Hyper/kernel queues are not retroactively revoked"
        );
        assert!(text.contains("event: error"));
        assert!(!text.contains("TCP-UNYIELDED-AFTER-REVOCATION-MUST-NOT-ESCAPE"));
        assert!(!text.contains("event: done"));
    }
    drop(server);
    drop(log);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn paused_tcp_reader_cannot_acquire_unyielded_live_frames_after_revocation() -> Result<()> {
    tcp_pressure_case(false).await
}

#[tokio::test]
async fn tcp_cancellation_drops_the_protected_body_and_releases_its_stream_lease() -> Result<()> {
    tcp_pressure_case(true).await
}

async fn head_after_target_wait(expire: bool) -> Result<()> {
    let fixture = worker_fixture().await?;
    let mut log = WorkerLog::allocate(&fixture, LogStreamBackend::ObjectSegments).await?;
    log.append(&fixture, b"HEAD-private-log-body-must-not-be-polled")
        .await?;
    log.seal(&fixture).await?;
    let (reader, token, approval) = super::worker_live_streams::fixed_reader(
        &fixture,
        super::worker_live_streams::exact_log_grants(&log),
        expire.then(|| chrono::Duration::seconds(5)),
    )
    .await?;
    let version = ArtifactVersionRepository::find_by_artifact_and_execution(
        &fixture.ctx.pool,
        log.artifact,
        log.execution,
    )
    .await?
    .unwrap();
    let server = TcpRouter::start(&fixture).await?;
    let mut lock = fixture.ctx.pool.begin().await?;
    ArtifactVersionRepository::find_by_id_for_update(&mut lock, version.id)
        .await?
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await?;
    let socket = server
        .request(
            "HEAD",
            &format!("/api/v1/artifacts/{}/download", version.artifact),
            &token,
        )
        .await?;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&fixture.ctx.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await?;
    if expire {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let expired: bool = sqlx::query_scalar("SELECT clock_timestamp()>=expires_at FROM runtime_external_approval WHERE id=$1")
                    .bind(approval).fetch_one(&fixture.ctx.pool).await.unwrap();
                if expired { break; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await?;
    }
    lock.rollback().await?;
    let mut bytes = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        socket.take(64 * 1024).read_to_end(&mut bytes),
    )
    .await??;
    let response = String::from_utf8_lossy(&bytes);
    if expire {
        assert!(!response.starts_with("HTTP/1.1 200"));
    } else {
        assert!(response.starts_with("HTTP/1.1 200"));
    }
    assert!(response.ends_with("\r\n\r\n"));
    assert!(!response.contains("HEAD-private-log-body-must-not-be-polled"));
    assert_eq!(
        server.handed_bytes.load(Ordering::SeqCst),
        0,
        "HEAD never needs a Body poll to enforce authority"
    );
    assert_ne!(reader.execution_id, log.execution);
    drop(server);
    drop(log);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn protected_head_checks_current_authority_without_body_polling_after_a_real_target_wait(
) -> Result<()> {
    head_after_target_wait(false).await?;
    head_after_target_wait(true).await
}
