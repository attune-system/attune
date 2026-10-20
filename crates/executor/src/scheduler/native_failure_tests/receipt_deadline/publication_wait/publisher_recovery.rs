//! A real Publisher recovery RPC, held at a loopback AMQP OpenOk frame. No
//! production hook, injected error, socket deadline or withdrawal assertion.
use super::*;
use attune_common::mq::{publisher::GuardedPublicationError, Connection};
use std::{
    net::{IpAddr, SocketAddr},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    },
};

const MAX_FRAME: usize = 128 * 1024; // negotiated frame_max, including header/end
const MAX_PAYLOAD: usize = MAX_FRAME - 8;
// Owned test transport only, never an input-authority window. The runner's
// disposable broker advertises the same heartbeat to avoid negotiating 60s.
const RECOVERY_HEARTBEAT_SECS: u16 = 180;
const RECOVERY_TRANSPORT_TIMEOUT_SECS: u64 = 150;

struct FrameGate {
    blocked: oneshot::Sender<u16>,
    release: oneshot::Receiver<()>,
    forwarded: oneshot::Sender<()>,
    release_limit: tokio::time::Instant,
}

struct GateTicket {
    blocked: oneshot::Receiver<u16>,
    release: oneshot::Sender<()>,
    forwarded: oneshot::Receiver<()>,
}

/// Exactly one accepted peer, one upstream connection and one owned task. The
/// two forwarding futures run inline, not detached. No byte/URI Debug logging.
struct OwnedOpenOkProxy {
    url: String,
    gate: Arc<Mutex<Option<FrameGate>>>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Result<()>>>,
}

impl OwnedOpenOkProxy {
    async fn start(vhost: &str) -> Result<Self> {
        let base = std::env::var("ATTUNE_TEST_CACHE_BROKER_URL")
            .map_err(|_| anyhow::anyhow!("requires the owned receipt broker runner"))?;
        let mut uri: lapin::uri::AMQPUri = format!("{base}/{vhost}")
            .parse()
            .map_err(|_| anyhow::anyhow!("owned broker URI is invalid"))?;
        anyhow::ensure!(
            uri.scheme == lapin::uri::AMQPScheme::AMQP,
            "this bounded test proxy requires the owned plain AMQP loopback lane"
        );
        let ip: IpAddr =
            uri.authority.host.parse().map_err(|_| {
                anyhow::anyhow!("owned broker must have a literal loopback address")
            })?;
        anyhow::ensure!(
            ip.is_loopback() && uri.authority.port != 0,
            "proxy refuses non-loopback or unspecified broker endpoints"
        );
        let upstream = SocketAddr::new(ip, uri.authority.port);
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let local = listener.local_addr()?;
        uri.authority.host = local.ip().to_string();
        uri.authority.port = local.port();
        uri.query.frame_max = Some(MAX_FRAME as u32);
        uri.query.heartbeat = Some(RECOVERY_HEARTBEAT_SECS);
        uri.query.connection_timeout = Some(10_000);
        let url = uri.to_string();
        let gate = Arc::new(Mutex::new(None));
        let forwarding_gate = Arc::clone(&gate);
        let (stop_send, mut stop_recv) = oneshot::channel();
        let task = tokio::spawn(async move {
            let work = async {
                let (client, _) = listener.accept().await?;
                drop(listener); // refuse any second connection/reconnect
                let server =
                    tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(upstream))
                        .await
                        .map_err(|_| anyhow::anyhow!("owned proxy upstream connect timed out"))??;
                let (mut client_read, mut client_write) = client.into_split();
                let (mut server_read, mut server_write) = server.into_split();
                let outbound = async {
                    // Includes protocol header/authentication without inspecting
                    // or recording credentials, bodies, keys or packet nonces.
                    tokio::io::copy(&mut client_read, &mut server_write).await?;
                    server_write.shutdown().await?;
                    Ok::<(), anyhow::Error>(())
                };
                let inbound =
                    forward_server_frames(&mut server_read, &mut client_write, forwarding_gate);
                tokio::try_join!(outbound, inbound)?;
                Ok::<(), anyhow::Error>(())
            };
            tokio::pin!(work);
            tokio::select! {
                biased;
                result = &mut work => result,
                _ = &mut stop_recv => Ok(()),
            }
            // Returning drops the pinned work future and every listener/socket
            // half it owns. Socket/listener close is synchronous Drop; awaiting
            // this one task proves no peer/forwarding task remains detached.
        });
        Ok(Self {
            url,
            gate,
            stop: Some(stop_send),
            task: Some(task),
        })
    }

    async fn arm_next_open_ok(&self, release_limit: tokio::time::Instant) -> Result<GateTicket> {
        let (blocked_send, blocked_recv) = oneshot::channel();
        let (release_send, release_recv) = oneshot::channel();
        let (forwarded_send, forwarded_recv) = oneshot::channel();
        let mut slot = self.gate.lock().await;
        anyhow::ensure!(slot.is_none(), "an owned OpenOk gate is already armed");
        *slot = Some(FrameGate {
            blocked: blocked_send,
            release: release_recv,
            forwarded: forwarded_send,
            release_limit,
        });
        Ok(GateTicket {
            blocked: blocked_recv,
            release: release_send,
            forwarded: forwarded_recv,
        })
    }

    async fn shutdown(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
                Ok(result) => result??,
                Err(_) => {
                    task.abort();
                    let joined = tokio::time::timeout(Duration::from_secs(5), &mut task).await;
                    anyhow::ensure!(
                        joined.is_ok(),
                        "aborted proxy task did not join within cleanup bound"
                    );
                    anyhow::bail!("owned proxy required abort-and-join during cleanup");
                }
            }
        }
        Ok(())
    }
}

async fn forward_server_frames(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    gate: Arc<Mutex<Option<FrameGate>>>,
) -> Result<()> {
    // Fixed allocation, independent of peer-provided length and frame count.
    // Only the AMQP 7-byte header and four method-ID bytes are interpreted.
    let mut payload = vec![0_u8; MAX_PAYLOAD];
    loop {
        let mut header = [0_u8; 7];
        if reader.read(&mut header[..1]).await? == 0 {
            writer.shutdown().await?;
            return Ok(());
        }
        reader.read_exact(&mut header[1..]).await?;
        let channel = u16::from_be_bytes([header[1], header[2]]);
        let length = u32::from_be_bytes([header[3], header[4], header[5], header[6]]) as usize;
        anyhow::ensure!(
            matches!(header[0], 1 | 2 | 3 | 8) && length <= MAX_PAYLOAD,
            "owned proxy received an unsupported or oversized AMQP frame"
        );
        reader.read_exact(&mut payload[..length]).await?;
        let mut end = [0_u8; 1];
        reader.read_exact(&mut end).await?;
        anyhow::ensure!(end[0] == 0xce, "owned proxy frame terminator is invalid");
        let open_ok = header[0] == 1
            && channel != 0
            && length >= 4
            && u16::from_be_bytes([payload[0], payload[1]]) == 20
            && u16::from_be_bytes([payload[2], payload[3]]) == 11;
        let held = if open_ok {
            gate.lock().await.take()
        } else {
            None
        };
        if let Some(held) = held {
            held.blocked
                .send(channel)
                .map_err(|_| anyhow::anyhow!("OpenOk controller dropped before observation"))?;
            tokio::time::timeout_at(held.release_limit, held.release)
                .await
                .map_err(|_| anyhow::anyhow!("owned OpenOk gate was not released"))?
                .map_err(|_| anyhow::anyhow!("OpenOk release controller dropped"))?;
            writer.write_all(&header).await?;
            writer.write_all(&payload[..length]).await?;
            writer.write_all(&end).await?;
            let _ = held.forwarded.send(());
        } else {
            writer.write_all(&header).await?;
            writer.write_all(&payload[..length]).await?;
            writer.write_all(&end).await?;
        }
    }
}

#[derive(Default)]
struct CallbackObservations {
    calls: AtomicUsize,
    denials: AtomicUsize,
    release_requested: AtomicBool,
    denials_after_release: AtomicUsize,
}

type GuardedOutcome = std::result::Result<(), GuardedPublicationError<anyhow::Error>>;
type PublicationTask = tokio::task::JoinHandle<(Publisher, GuardedOutcome)>;

async fn recovery_wait_case(expire: bool) {
    let vhost = if expire {
        "receipt-recovery-expiry"
    } else {
        "receipt-recovery-control"
    };
    let broker = Arc::new(CacheBrokerFixture::connect(vhost).await);
    let setup = AssertUnwindSafe(fixture::original_executor_finite_publication_pair(
        canonical_task_graph,
    ))
    .catch_unwind()
    .await;
    let (db, files, parent, workflow, completed) = match setup {
        Ok(fixture) => fixture,
        Err(panic) => {
            let cleanup = AssertUnwindSafe(
                Arc::try_unwrap(broker)
                    .ok()
                    .expect("no publisher task exists during fixture setup")
                    .close_bounded(),
            )
            .catch_unwind()
            .await;
            match cleanup {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!(
                    "owned bounded broker cleanup also failed during setup failure: {error}"
                ),
                Err(_) => {
                    eprintln!("owned bounded broker cleanup also panicked during setup failure")
                }
            }
            std::panic::resume_unwind(panic);
        }
    };
    let mut proxy: Option<OwnedOpenOkProxy> = None;
    let mut connection: Option<Connection> = None;
    let mut publisher: Option<Publisher> = None;
    let mut operation: Option<PublicationTask> = None;
    let mut gate_release = None;
    let observations = Arc::new(CallbackObservations::default());
    let result = AssertUnwindSafe(async {
        let pool = db.pool();
        proxy = Some(OwnedOpenOkProxy::start(vhost).await?);
        // Both connection and publisher ownership stay outside this caught
        // future. Even partial Publisher construction failure closes connection.
        connection = Some(tokio::time::timeout(Duration::from_secs(10), Connection::connect(&proxy.as_ref().unwrap().url))
            .await.map_err(|_| anyhow::anyhow!("proxy AMQP connect timed out"))?
            .map_err(|_| anyhow::anyhow!("owned proxy AMQP connection failed"))?);
        publisher = Some(tokio::time::timeout(Duration::from_secs(10), Publisher::new(connection.as_ref().unwrap(), attune_common::mq::PublisherConfig {
            confirm_publish:true, timeout_secs:RECOVERY_TRANSPORT_TIMEOUT_SECS, exchange:"attune.executions".into(),
        })).await.map_err(|_| anyhow::anyhow!("owned proxy Publisher handshake timed out"))?
            .map_err(|_| anyhow::anyhow!("owned proxy Publisher handshake failed"))?);
        anyhow::ensure!(publisher.as_ref().unwrap().channel().await.status().connected(),
            "proxy publisher handshake did not complete");
        // Finish proxy AMQP/confirm handshake BEFORE original consent exists.
        let mut clock = pool.acquire().await?;
        let consent = issue_later_input_consent(&db, &parent).await?;
        let transport: Arc<dyn attune_common::artifact_transport::ArtifactFileTransport> =
            Arc::new(attune_common::artifact_transport::VolumeTransport::new(files.path().to_str()
                .ok_or_else(|| anyhow::anyhow!("owned fixture path unavailable"))?));
        // Real producer commit and publication, not a hand-written child insert
        // or local fake commit. No 07 application-handoff observer is installed.
        ExecutionScheduler::advance_workflow_owned(
            pool, &broker.publisher, &AtomicUsize::new(0), files.path().to_str().unwrap(),
            &transport, 1024 * 1024, 1000, Some(fixture::KEY), &completed,
            &SchedulerMetadataCaches::new(), None,
        ).await?;
        let children = ExecutionRepository::find_by_parent(pool, parent.id).await?;
        let first = children.iter().find(|row| row.workflow_task.as_ref()
            .is_some_and(|task| task.task_name == "consumer"))
            .ok_or_else(|| anyhow::anyhow!("actual first committed consumer missing"))?;
        let child = children.iter().find(|row| row.workflow_task.as_ref()
            .is_some_and(|task| task.task_name == "finite_later"))
            .ok_or_else(|| anyhow::anyhow!("actual finite committed consumer missing"))?;
        anyhow::ensure!(child.status == ExecutionStatus::Requested && child.permission_set_refs.is_empty(),
            "recovery target must remain the actual Requested empty-SDK child");
        let original_row = serde_json::to_value(child)?;
        let original_fields = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *clock, ENTITY_EXECUTION_CONFIG, child.id,
        ).await?;
        anyhow::ensure!(original_fields.iter().any(|field| field.json_path == "/finite"),
            "actual finite typed input delivery is missing");
        let original_child_ids: std::collections::BTreeSet<_> = children.iter().map(|row| row.id).collect();
        let mut original_requested_payload = None;
        for expected in [first.id, child.id] {
            let message = broker.receive().await;
            anyhow::ensure!(message["payload"]["execution_id"] == serde_json::json!(expected),
                "original actual-owner publication order differs");
            if expected == child.id { original_requested_payload = Some(message["payload"].clone()); }
        }
        broker.assert_empty().await; // Drain original publication, not its authority.
        let pending = PreparedWorkflowInputPublications::prepare_pending_data(
            child, child.action.ok_or_else(|| anyhow::anyhow!("actual child action missing"))?,
            &child.action_ref, &parent,
        )?;
        let mut data = WorkflowPublications::default();
        data.push(pending.clone());
        let mut validation = pool.begin().await?;
        let proofs = PreparedWorkflowInputPublications::prepare(&mut validation, &data, Some(fixture::KEY)).await?;
        anyhow::ensure!(proofs.publication_wait_deadline(child.id)? == Some(consent.deadline),
            "reopened original typed input lacks its exact original finite approval deadline");
        proofs.ensure_fresh_on(&mut validation).await?;
        validation.commit().await?;
        proofs.ensure_fresh()?;
        let closed = publisher.as_ref().unwrap().channel().await;
        tokio::time::timeout(Duration::from_secs(5),
            closed.close(200, "owned test closes only the publisher channel".into())).await
            .map_err(|_| anyhow::anyhow!("owned publisher initial channel close timed out"))?
            .map_err(|_| anyhow::anyhow!("owned publisher initial channel close failed"))?;
        anyhow::ensure!(!closed.status().connected(),
            "recovery probe requires an actually Closed channel");
        proofs.ensure_child_fresh(child.id)?;
        let readiness_deadline = pair_consent_readiness_deadline(&mut clock, &consent).await?;
        let GateTicket { blocked, release, forwarded } = proxy.as_ref().unwrap()
            .arm_next_open_ok(consent.control_wait_limit()).await?;
        gate_release = Some(release);
        let envelope = MessageEnvelope::new(MessageType::ExecutionRequested, ExecutionRequestedPayload {
            execution_id:pending.execution_id, action_id:Some(pending.action_id), action_ref:pending.action_ref,
            parent_id:Some(pending.parent_id), enforcement_id:pending.enforcement_id, config:pending.config,
            release_id:pending.release_id, release_digest:pending.release_digest,
        }).with_source("executor-scheduler");
        let expected_payload = serde_json::to_value(&envelope.payload)?;
        anyhow::ensure!(original_requested_payload.as_ref() == Some(&expected_payload),
            "canonical pending payload differs from the actual original child request");
        let owned_publisher = publisher.take().ok_or_else(|| anyhow::anyhow!("owned publisher unavailable"))?;
        let callback_observations = Arc::clone(&observations);
        let child_id = child.id;
        operation = Some(tokio::spawn(async move {
            let fresh = || {
                callback_observations.calls.fetch_add(1, Ordering::SeqCst);
                let result = proofs.ensure_child_fresh(child_id);
                if result.is_err() {
                    callback_observations.denials.fetch_add(1, Ordering::SeqCst);
                    if callback_observations.release_requested.load(Ordering::SeqCst) {
                        callback_observations.denials_after_release.fetch_add(1, Ordering::SeqCst);
                    }
                }
                result // Only genuine retained proofs decide, never observations.
            };
            // Intentionally omit the OPTIONAL public wait date to isolate the
            // real-proof callback on a recovery wake from the date-budget timer.
            // The one originally issued 120s approval remains mandatory.
            // The owned 150s transport timeout is NOT authorization grace.
            let outcome = owned_publisher.publish_envelope_guarded(&envelope, None, &fresh).await;
            (owned_publisher, outcome)
        }));
        let opened_channel = tokio::time::timeout_at(readiness_deadline, blocked).await
            .map_err(|_| anyhow::anyhow!("real Publisher recovery did not block at broker Channel.OpenOk"))??;
        anyhow::ensure!(tokio::time::Instant::now() < readiness_deadline,
            "real recovery readiness observed after original consent readiness deadline");
        anyhow::ensure!(opened_channel != 0, "gate observed connection control, not a channel recovery");
        anyhow::ensure!(observations.calls.load(Ordering::SeqCst) > 0
            && observations.denials.load(Ordering::SeqCst) == 0,
            "real initial closed-channel attempt must have passed the genuine fresh callback");
        let still_original_and_fresh: bool = sqlx::query_scalar(
            "SELECT expires_at=$2 AND expires_at>clock_timestamp() FROM runtime_external_approval WHERE id=$1"
        ).bind(consent.id).bind(consent.deadline).fetch_one(&mut *clock).await?;
        anyhow::ensure!(still_original_and_fresh, "real recovery gate was not reached before original consent expiry");
        broker.assert_empty().await;
        if expire { wait_for_pair_consent_expiry(&mut clock, &consent).await?; }
        anyhow::ensure!(!operation.as_ref().unwrap().is_finished(),
            "publication ended before the controlled recovery wait was released");
        observations.release_requested.store(true, Ordering::SeqCst);
        gate_release.take().ok_or_else(|| anyhow::anyhow!("recovery release sender missing"))?
            .send(()).map_err(|_| anyhow::anyhow!("proxy stopped before recovery release"))?;
        tokio::time::timeout(Duration::from_secs(5), forwarded).await
            .map_err(|_| anyhow::anyhow!("held actual OpenOk was not forwarded after release"))??;
        let joined = tokio::time::timeout(Duration::from_secs(10), operation.as_mut().unwrap()).await;
        let outcome = match joined {
            Ok(joined) => {
                operation.take(); // Never poll a completed JoinHandle twice.
                let (returned_publisher, outcome) = joined?;
                publisher = Some(returned_publisher);
                outcome
            }
            Err(_) => return Err(anyhow::anyhow!("guarded publication did not finish after real recovery release")),
        };
        if expire {
            match outcome {
                Err(GuardedPublicationError::Guard(error)) => anyhow::ensure!(
                    matches!(error.downcast_ref::<attune_common::Error>(), Some(attune_common::Error::PermissionDenied(_))),
                    "real proof callback must supply expiry denial"),
                Err(GuardedPublicationError::Transport(_)) => anyhow::bail!("transport timeout/error is not original-proof expiry coverage"),
                Ok(()) => anyhow::bail!("recovery accepted an expired original input proof"),
            }
            anyhow::ensure!(observations.denials_after_release.load(Ordering::SeqCst) > 0,
                "original proof was not checked and denied after releasing the real recovery wait");
            broker.assert_empty().await;
        } else {
            match outcome {
                Ok(()) => {}
                Err(_) => anyhow::bail!("before-expiry recovery control failed"),
            }
            anyhow::ensure!(observations.denials.load(Ordering::SeqCst) == 0,
                "positive control observed a proof denial");
            let message = broker.receive().await;
            anyhow::ensure!(message["payload"] == expected_payload,
                "real recovery did not publish the exact original Requested child payload");
            broker.assert_empty().await; // Exactly one reused-row request, no duplicate.
        }
        let after = ExecutionRepository::find_by_parent(pool, parent.id).await?;
        anyhow::ensure!(after.iter().map(|row| row.id).collect::<std::collections::BTreeSet<_>>() == original_child_ids,
            "publisher recovery minted a replacement child");
        let unchanged = after.iter().find(|row| row.id == child.id)
            .ok_or_else(|| anyhow::anyhow!("original child disappeared during recovery"))?;
        anyhow::ensure!(serde_json::to_value(unchanged)? == original_row,
            "original Requested row or authority selection changed during recovery");
        let after_fields = ExecutionSecretValueRepository::find_stored_by_entity(&mut *clock, ENTITY_EXECUTION_CONFIG, child.id).await?;
        anyhow::ensure!(after_fields == original_fields, "original ciphertext/typed provenance changed during recovery");
        let mut tx = pool.begin().await?;
        let approval = Authority::find_approval(&mut tx, consent.id).await?
            .ok_or_else(|| anyhow::anyhow!("original consent disappeared"))?;
        anyhow::ensure!(approval_snapshot(&approval)? == consent.snapshot, "original consent was rewritten or replaced");
        let root = Accepted::revalidate_execution(&mut tx, parent.id, fixture::KEY).await?;
        root.ensure_fresh_on(&mut tx).await?;
        let durable = WorkflowExecutionRepository::find_by_id(&mut *tx, workflow.id).await?
            .ok_or_else(|| anyhow::anyhow!("original workflow missing"))?;
        anyhow::ensure!(durable.status == ExecutionStatus::Running, "publication must not execute or complete work");
        tx.rollback().await?;
        Ok::<(), anyhow::Error>(())
    }).catch_unwind().await;
    // First release the owned frame, then join/abort-and-join any publication
    // borrow. Connection ownership stays accessible even if its task panics.
    if let Some(release) = gate_release.take() {
        let _ = release.send(());
    }
    let mut cleanup_failures = Vec::new();
    if let Some(mut pending) = operation.take() {
        match tokio::time::timeout(Duration::from_secs(10), &mut pending).await {
            Ok(Ok((returned, _))) => publisher = Some(returned),
            Ok(Err(_)) => cleanup_failures.push("publication task join"),
            Err(_) => {
                pending.abort();
                if tokio::time::timeout(Duration::from_secs(5), &mut pending)
                    .await
                    .is_err()
                {
                    cleanup_failures.push(
                        "aborted publication task join timed out; supervisor containment required",
                    );
                }
                cleanup_failures.push("publication task abort-and-join");
            }
        }
    }
    if let Some(owned) = publisher.take() {
        let channel_cleanup = AssertUnwindSafe(async {
            let channel = owned.channel().await;
            if channel.status().connected() {
                channel
                    .close(200, "owned recovery publisher cleanup".into())
                    .await?;
            }
            Ok::<(), attune_common::mq::MqError>(())
        })
        .catch_unwind();
        if !matches!(
            tokio::time::timeout(Duration::from_secs(5), channel_cleanup).await,
            Ok(Ok(Ok(())))
        ) {
            cleanup_failures.push("proxy publisher channel acquisition/close");
        }
        drop(owned);
    }
    if let Some(owned) = connection.take() {
        if !matches!(
            tokio::time::timeout(
                Duration::from_secs(5),
                AssertUnwindSafe(owned.close()).catch_unwind()
            )
            .await,
            Ok(Ok(Ok(())))
        ) {
            cleanup_failures.push("proxy AMQP connection");
        }
    }
    if let Some(owned) = proxy.take() {
        if !matches!(
            AssertUnwindSafe(owned.shutdown()).catch_unwind().await,
            Ok(Ok(()))
        ) {
            cleanup_failures.push("TCP proxy task/peers");
        }
    }
    let broker_cleanup = AssertUnwindSafe(async {
        Arc::try_unwrap(broker)
            .ok()
            .expect("all direct broker borrows ended")
            .close_bounded()
            .await
    })
    .catch_unwind()
    .await;
    let database_cleanup = tokio::time::timeout(
        Duration::from_secs(30),
        AssertUnwindSafe(db.cleanup()).catch_unwind(),
    )
    .await;
    let files_cleanup = files.close();
    // Every cleanup was attempted/awaited before surfacing scenario assertions.
    let broker_failure = match broker_cleanup {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error.to_string()),
        Err(_) => Some("direct broker cleanup panicked".to_owned()),
    };
    assert!(
        broker_failure.is_none(),
        "owned individually bounded direct broker teardown failed: {broker_failure:?}"
    );
    assert!(
        matches!(database_cleanup, Ok(Ok(Ok(())))),
        "owned database cleanup failed, panicked or exceeded 30s"
    );
    files_cleanup.expect("owned fixture files cleanup failed");
    assert!(
        cleanup_failures.is_empty(),
        "owned recovery cleanup failed at {cleanup_failures:?}"
    );
    match result {
        Ok(result) => result.expect("real original-input Publisher recovery regression failed"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test]
#[ignore = "requires owned RabbitMQ and PostgreSQL"]
async fn original_finite_input_denies_new_attempt_after_real_publisher_recovery_wait() {
    recovery_wait_case(true).await;
}

#[tokio::test]
#[ignore = "requires owned RabbitMQ and PostgreSQL"]
async fn original_finite_input_real_publisher_recovery_before_expiry_control() {
    recovery_wait_case(false).await;
}
