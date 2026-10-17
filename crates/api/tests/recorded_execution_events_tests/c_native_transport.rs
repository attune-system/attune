//! Native source receipts carried through the existing worker/log/HTTP barriers.
//! No replacement source proof, global hook, or new server fixture.
use super::live_decoder_gate::{DecoderGate, WaitAt};
use super::worker_live_streams::{exact_log_grants, grant_human, WorkerLog};
use super::*;
use attune_common::{
    models::LogStreamBackend,
    repositories::{
        artifact::ArtifactVersionRepository, log_stream::LogStreamRepository, FindById,
    },
    runtime_authority::AdmittedPermissionSet,
};
use axum::{body::Body, http::Request};
use futures::TryStreamExt;
use std::{sync::Arc, time::Duration};
use tower::ServiceExt;

#[derive(Clone, Copy)]
pub(super) enum Checkpoint {
    Head,
    Chunk,
    Eof,
    DownloadChunk,
    DownloadEof,
    Human(super::c_native_human_transport::Change, Boundary),
    OriginalContributor {
        boundary: Boundary,
        human: bool,
        weak: bool,
    },
    Socket,
    Mixed {
        boundary: Boundary,
        human: bool,
    },
}

#[derive(Clone, Copy)]
pub(super) enum Boundary {
    Head,
    Chunk,
    Eof,
    DownloadChunk,
    DownloadEof,
    Socket,
}
impl Boundary {
    fn checkpoint(self) -> Checkpoint {
        match self {
            Self::Head => Checkpoint::Head,
            Self::Chunk => Checkpoint::Chunk,
            Self::Eof => Checkpoint::Eof,
            Self::DownloadChunk => Checkpoint::DownloadChunk,
            Self::DownloadEof => Checkpoint::DownloadEof,
            Self::Socket => Checkpoint::Socket,
        }
    }
}

#[tokio::test]
async fn native_execution_wait_expiry_after_http_head_chunk_and_eof_waits() -> Result<()> {
    for at in [
        Checkpoint::Head,
        Checkpoint::Chunk,
        Checkpoint::Eof,
        Checkpoint::DownloadChunk,
        Checkpoint::DownloadEof,
    ] {
        super::c_native_execution_wait::execution_wait(Some(at)).await?;
    }
    Ok(())
}

#[tokio::test]
async fn native_inquiry_wait_expiry_after_http_head_chunk_and_eof_waits() -> Result<()> {
    for at in [
        Checkpoint::Head,
        Checkpoint::Chunk,
        Checkpoint::Eof,
        Checkpoint::DownloadChunk,
        Checkpoint::DownloadEof,
    ] {
        super::c_native_related_wait::related_wait(
            super::c_native_related_wait::Kind::Inquiry,
            true,
            Some(at),
        )
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn native_queue_wait_expiry_after_http_head_chunk_and_eof_waits() -> Result<()> {
    for at in [
        Checkpoint::Head,
        Checkpoint::Chunk,
        Checkpoint::Eof,
        Checkpoint::DownloadChunk,
        Checkpoint::DownloadEof,
    ] {
        super::c_native_related_wait::related_wait(
            super::c_native_related_wait::Kind::Queue,
            true,
            Some(at),
        )
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn native_cache_original_deadline_after_http_head_chunk_and_eof_waits() -> Result<()> {
    for at in [
        Checkpoint::Head,
        Checkpoint::Chunk,
        Checkpoint::Eof,
        Checkpoint::DownloadChunk,
        Checkpoint::DownloadEof,
    ] {
        super::c_native_cache::cache_http_case(true, 30, Some(at)).await?;
    }
    Ok(())
}

/// Issue an independent fixed reader approval against the actual native
/// fixture's leaf invocation. Original source admissions and contributor
/// manifests are not changed by this reader-only policy revision.
async fn reader(
    fixture: &Fixture,
    template: Id,
    policy: Id,
    grants: Vec<Value>,
    finite: bool,
) -> Result<(
    RuntimeCredentialBinding,
    String,
    Option<chrono::DateTime<chrono::Utc>>,
)> {
    grant_human(fixture, grants.clone()).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let original = RuntimeAuthorityRepository::execution_admission(&mut tx, template)
        .await?
        .unwrap();
    let policy = RuntimeAuthorityRepository::find_policy(&mut tx, policy)
        .await?
        .unwrap();
    let grants = grants
        .into_iter()
        .map(serde_json::from_value)
        .collect::<std::result::Result<Vec<attune_common::rbac::Grant>, _>>()?;
    let deadline = finite.then(|| chrono::Utc::now() + chrono::Duration::seconds(8));
    let approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        fixture.pack_id,
        fixture.requester,
        grants.clone(),
        deadline,
    )
    .await?;
    let set = AdmittedPermissionSet::external(
        "recorded_emitter.c_native_transport",
        &policy.policy.owner,
        &grants,
        &[approval.approval()?],
    )?;
    let mut next = policy.policy.clone();
    next.invocations
        .iter_mut()
        .find(|invocation| invocation.binding == original.authority.binding)
        .unwrap()
        .permission_sets
        .insert(set.reference.clone(), set);
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &next).await?;
    assert!(
        next.concrete_targets.iter().all(|(digest, binding)| policy
            .policy
            .concrete_targets
            .get(digest)
            == Some(binding)),
        "a reader-only approval cannot change any original native reader target binding"
    );
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id).await?;
    let mut refs = original.authority.selected_refs.clone();
    refs.push("recorded_emitter.c_native_transport".to_owned());
    refs.sort();
    refs.dedup();
    let selected = policy.policy.select(
        policy.id,
        &original.authority.binding,
        &refs,
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: fixture.requester,
                login: fixture.ctx.user.as_ref().unwrap().login.clone(),
            },
        },
    )?;
    let pin = ExecutableSnapshotRepository::resolve_for_action_ref(
        &mut *tx,
        original.authority.binding.action_ref(),
    )
    .await?;
    let credential =
        super::c_native_execution_wait::capture_reader(&mut tx, &pin, &selected).await?;
    tx.commit().await?;
    let token = generate_admitted_execution_token(
        &credential,
        &refs,
        &fixture.ctx.state.jwt_config,
        Some(360),
    )?;
    Ok((credential, token, deadline))
}

pub(super) async fn expires(deadline: chrono::DateTime<chrono::Utc>) -> Result<()> {
    // Real time is the contract. Do not mutate the original absolute deadline.
    tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            let remaining = (deadline - chrono::Utc::now()).to_std().unwrap_or_default();
            if remaining.is_zero() {
                break;
            }
            tokio::time::sleep_until(tokio::time::Instant::now() + remaining).await;
        }
    })
    .await?;
    Ok(())
}

struct Cancel(tokio::task::AbortHandle);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn next_nonempty(body: &mut axum::body::BodyDataStream) -> Result<Option<axum::body::Bytes>> {
    while let Some(chunk) = body.try_next().await? {
        if !chunk.is_empty() {
            return Ok(Some(chunk));
        }
    }
    Ok(None)
}

pub(super) async fn exercise(
    fixture: &mut Fixture,
    execution: Id,
    template: Id,
    policy: Id,
    source_deadline: Option<chrono::DateTime<chrono::Utc>>,
    at: Checkpoint,
) -> Result<()> {
    let (at, human_change) = match at {
        Checkpoint::Human(change, boundary) => (boundary.checkpoint(), Some(change)),
        Checkpoint::OriginalContributor {
            boundary, human, ..
        } => (
            boundary.checkpoint(),
            human.then_some(super::c_native_human_transport::Change::SourceDeadline),
        ),
        Checkpoint::Mixed { boundary, human } => (
            boundary.checkpoint(),
            human.then_some(super::c_native_human_transport::Change::SourceDeadline),
        ),
        at => (at, None),
    };
    let original_fields = ExecutionSecretValueRepository::find_stored_by_entity(
        &fixture.ctx.pool,
        attune_common::secret_values::ENTITY_EXECUTION_RESULT,
        execution,
    )
    .await?;
    assert!(
        !original_fields.is_empty(),
        "transport must consume the real native receipt chain"
    );
    // Native workflow renderers selected no process API grants, so do not mint
    // a process JWT. Only worker registration/transport/seal methods are used;
    // the helper's unused process-token field remains empty, never authorized.
    let mut log = WorkerLog::for_execution(
        fixture,
        execution,
        String::new(),
        LogStreamBackend::ObjectSegments,
    )
    .await?;
    log.append(fixture, b"native-transport-prefix").await?;
    log.append(fixture, b"native-unyielded-frame").await?;
    log.seal(fixture).await?;
    assert_eq!(
        ExecutionRepository::find_by_id(&fixture.ctx.pool, execution)
            .await?
            .unwrap()
            .status,
        ExecutionStatus::Completed,
        "terminal observation must not need a running producer credential"
    );
    // The independent approval covers only this new log artifact. Preserve the
    // original full reader selection and its exact native source bindings,
    // including collected sources. Never re-resolve them to issue a new grant.
    let log_grants = exact_log_grants(&log)
        .into_iter()
        .filter(|grant| grant["resource"] == "artifacts")
        .collect::<Vec<_>>();
    assert_eq!(
        log_grants.len(),
        1,
        "the approval covers only the new artifact"
    );
    let human = if let Some(change) = human_change {
        Some(
            super::c_native_human_transport::create(
                fixture,
                template,
                policy,
                log_grants.clone(),
                change,
                source_deadline,
            )
            .await?,
        )
    } else {
        None
    };
    let (lease_identity, token, deadline) = if let Some(human) = &human {
        (human.identity, human.token.clone(), human.deadline)
    } else {
        let (credential, token, reader_deadline) = reader(
            fixture,
            template,
            policy,
            log_grants,
            source_deadline.is_none(),
        )
        .await?;
        assert_eq!(
            reader_deadline.is_none(),
            source_deadline.is_some(),
            "source expiry must not be explained by a reader approval deadline"
        );
        (
            credential.sponsor_identity_id,
            token,
            source_deadline.or(reader_deadline).unwrap(),
        )
    };
    let download = format!("/api/v1/artifacts/{}/download", log.artifact);
    // Actual completed-source HTTP positive before any pause. No result getter
    // or proof constructor stands in for the reader's source authorization.
    let response = fixture
        .ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(&download)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        chrono::Utc::now() < deadline,
        "source policy must still be live when the actual wait starts"
    );
    drop(response);

    if matches!(at, Checkpoint::Socket) {
        super::c_native_socket::exercise(
            fixture,
            execution,
            &token,
            deadline,
            source_deadline.is_some(),
            human.as_ref(),
        )
        .await?;
    } else if matches!(at, Checkpoint::Head) {
        let mut held = fixture.ctx.pool.begin().await?;
        ArtifactVersionRepository::find_by_id_for_update(&mut held, log.version)
            .await?
            .unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *held)
            .await?;
        let app = fixture.ctx.app.clone();
        let request = Request::builder()
            .method("HEAD")
            .uri(download)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())?;
        let pending = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
        let _cancel = Cancel(pending.abort_handle());
        // The pinned-version lock is after canonical source preparation. Also
        // require that backend to hold the native archive's source lock.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let ready: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(a.pid)) AND a.query LIKE '%artifact_version%' AND a.query LIKE '%FOR SHARE%' AND EXISTS(SELECT 1 FROM pg_locks l WHERE l.pid=a.pid AND l.relation='runtime_native_input_snapshot'::regclass AND l.granted AND l.mode='RowShareLock'))")
                    .bind(blocker).fetch_one(&fixture.ctx.pool).await.unwrap();
                if ready { break; }
                tokio::task::yield_now().await;
            }
        }).await?;
        assert!(!pending.is_finished());
        assert!(
            chrono::Utc::now() < deadline,
            "HEAD must reach the prepared-source target wait before expiry"
        );
        if let Some(human) = &human {
            human.change_after_wait(fixture, execution).await?;
        } else {
            expires(deadline).await?;
        }
        held.rollback().await?;
        let response = tokio::time::timeout(Duration::from_secs(10), pending).await??;
        assert!(
            !response.status().is_success(),
            "HEAD cannot succeed after its held-source target wait expires"
        );
        // Never poll a successful HEAD Body to obtain an authorization check.
        drop(response);
    } else {
        let eof = matches!(at, Checkpoint::Eof | Checkpoint::DownloadEof);
        let download_body = matches!(at, Checkpoint::DownloadChunk | Checkpoint::DownloadEof);
        let stream = LogStreamRepository::find_by_artifact_version(&fixture.ctx.pool, log.version)
            .await?
            .unwrap();
        assert_eq!(
            stream.next_sequence, 3,
            "two data appends and the real worker terminal frame"
        );
        let segment = LogStreamRepository::find_segment_by_sequence(
            &fixture.ctx.pool,
            stream.id,
            if eof { stream.next_sequence - 1 } else { 1 },
        )
        .await?
        .unwrap();
        let arrived = Arc::new(tokio::sync::Semaphore::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let gate = Arc::new(DecoderGate {
            inner: fixture.ctx.state.blob_store.clone(),
            key: segment.object_key,
            at: if eof { WaitAt::Eof } else { WaitAt::FirstRead },
            arrived: arrived.clone(),
            release: release.clone(),
        });
        let mut state = fixture.ctx.state.as_ref().clone();
        state.blob_store = gate;
        fixture.ctx.state = Arc::new(state);
        fixture.ctx.app = attune_api::server::Server::new(fixture.ctx.state.clone()).router();
        let response = fixture
            .ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(if download_body {
                        download
                    } else {
                        format!("/api/v1/executions/{execution}/logs/stdout/stream")
                    })
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();
        if download_body {
            assert!(
                next_nonempty(&mut body).await?.unwrap().as_ref() == b"native-transport-prefix"
            );
            if eof {
                assert!(
                    next_nonempty(&mut body).await?.unwrap().as_ref() == b"native-unyielded-frame"
                );
            }
        } else {
            assert!(super::live_streams::next_data(&mut body)
                .await?
                .contains("native-transport-prefix"));
            if eof {
                assert!(super::live_streams::next_data(&mut body)
                    .await?
                    .contains("native-unyielded-frame"));
            }
        }
        let pending = tokio::spawn(async move {
            if download_body {
                // Skip empty data frames so only actual successful EOF, an
                // unyielded plaintext chunk, or an error completes this poll.
                next_nonempty(&mut body).await.is_err()
            } else {
                let event = super::live_streams::next_data(&mut body).await.unwrap();
                event.contains("event: error")
                    && !event.contains("native-unyielded-frame")
                    && !event.contains("event: done")
            }
        });
        let _cancel = Cancel(pending.abort_handle());
        tokio::time::timeout(Duration::from_secs(5), arrived.acquire())
            .await??
            .forget();
        assert!(
            !pending.is_finished(),
            "actual blob decoder must be paused after native source preparation"
        );
        assert!(
            chrono::Utc::now() < deadline,
            "decoder wait must start before expiry"
        );
        let leases: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM execution_log_stream_lease WHERE identity_id=$1",
        )
        .bind(lease_identity)
        .fetch_one(&fixture.ctx.pool)
        .await?;
        if !download_body {
            assert!(leases > 0, "the actual stream owns a lease before expiry");
        }
        if let Some(human) = &human {
            human.change_after_wait(fixture, execution).await?;
        } else {
            expires(deadline).await?;
        }
        release.add_permits(1);
        assert!(
            tokio::time::timeout(Duration::from_secs(10), pending).await??,
            "after expiry the real body must fail, not yield unhanded plaintext or successful EOF"
        );
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            fixture
                .ctx
                .state
                .execution_log_streams
                .drain_releases()
                .await;
            let leases: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM execution_log_stream_lease WHERE identity_id=$1",
            )
            .bind(lease_identity)
            .fetch_one(&fixture.ctx.pool)
            .await
            .unwrap();
            if leases == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    if source_deadline.is_some() {
        assert_eq!(
            fixture
                .ctx
                .get(&format!("/api/v1/executions/{execution}"), Some(&token))
                .await?
                .status(),
            StatusCode::OK,
            "the current reader remains live after the original source deadline expires"
        );
    }
    assert!(
        original_fields
            == ExecutionSecretValueRepository::find_stored_by_entity(
                &fixture.ctx.pool,
                attune_common::secret_values::ENTITY_EXECUTION_RESULT,
                execution,
            )
            .await?,
        "native receipt ciphertext must remain unchanged across actual transport waits"
    );
    drop(log);
    Ok(())
}
