//! Retained file publication, separate from Run, raw capture and Setup output.
//! This component is not scheduled or mounted by the service until actual CP
//! publication effects are integrated and the combined boundary is validated.
use crate::{
    actuator::{ActuatorError, Result},
    cache::{CacheError, CacheStore},
    control_plane::ControlPlane,
};
use attune_common::{workload_artifact::*, workload_launch::*};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, sync::Arc};

pub struct ArtifactPublisher {
    cp: ControlPlane,
    cache: Arc<CacheStore>,
}
impl ArtifactPublisher {
    pub fn new(cp: ControlPlane, cache: Arc<CacheStore>) -> Self {
        Self { cp, cache }
    }

    /// One bounded pass. No retry after an ambiguous upload, no native absence
    /// inference and no resolve/private-context fallback. A later discovery
    /// pass may obtain a historical receipt or a fresh publication lease.
    pub async fn reconcile(&self, reserved: &ReservedLaunch) -> Result<ClaimedArtifactPublication> {
        // Authenticated Stopped means provider create is settled and Wait+
        // Remove acknowledged. Source revocation cannot reopen Run here.
        let status = self.cp.status(reserved).await?;
        if status.request != reserved.request || status.state != LaunchState::Stopped {
            return Err(ActuatorError::Authority);
        }
        let cache = self.cache.clone();
        let request = reserved.request.clone();
        tokio::task::spawn_blocking(move || cache.begin_artifact_cleanup(&request, &status))
            .await
            .map_err(|_| ActuatorError::FilesystemTask)??;
        self.reconcile_queued(&reserved.request).await
    }

    /// Historical public removal evidence survives CP discovery ending. Fresh
    /// claim authentication still fences controller/session/engine authority;
    /// no dispatch reconstruction, resolve or private context is required.
    pub async fn reconcile_queued(
        &self,
        request: &LaunchRequest,
    ) -> Result<ClaimedArtifactPublication> {
        let cache = self.cache.clone();
        let request = request.clone();
        let evidence =
            tokio::task::spawn_blocking(move || cache.artifact_cleanup_evidence(&request))
                .await
                .map_err(|_| ActuatorError::FilesystemTask)??;
        let manifest = evidence.manifest;
        let status = evidence.status;
        let claim = self.cp.claim_artifacts(&status.request).await?;
        if matches!(claim, ClaimedArtifactPublication::NotReady { .. }) {
            return Ok(claim);
        }
        match claim {
            ClaimedArtifactPublication::Transfer {
                lease,
                files,
                limits,
                ..
            } => {
                let cache = self.cache.clone();
                let saved = manifest.clone();
                let stopped = status.clone();
                let validation = tokio::task::spawn_blocking(move || {
                    cache.validate_settled_artifact_staging(&saved, &stopped, limits)
                })
                .await
                .map_err(|_| ActuatorError::FilesystemTask)?;
                if let Err(error) = validation {
                    let claim = self
                        .cp
                        .fail_artifacts(&lease, failure_reason(&error))
                        .await?;
                    return self.finish(&manifest, &status, claim).await;
                }
                for selected in files {
                    let cache = self.cache.clone();
                    let saved = manifest.clone();
                    let stopped = status.clone();
                    let file = selected.clone();
                    let maximum = selected
                        .max_bytes
                        .min(limits.max_total_bytes)
                        .min(self.cache.artifact_limits().max_total_bytes);
                    let measured = tokio::task::spawn_blocking(move || {
                        let body =
                            cache.open_settled_artifact(&saved, &stopped, &file.relative_path)?;
                        measure(body, maximum)
                    })
                    .await
                    .map_err(|_| ActuatorError::FilesystemTask)?;
                    let (body, size_bytes, sha256) = match measured {
                        Ok(body) => body,
                        Err(CacheError::Io(error))
                            if error.kind() == std::io::ErrorKind::NotFound =>
                        {
                            // Only an exact selected file's actual nofollow
                            // NotFound, after validating the retained root.
                            self.cp
                                .report_missing_artifact(&lease, selected.version_id)
                                .await?;
                            continue;
                        }
                        Err(error) => {
                            let claim = self
                                .cp
                                .fail_artifacts(&lease, failure_reason(&error))
                                .await?;
                            return self.finish(&manifest, &status, claim).await;
                        }
                    };
                    let output = ReserveArtifactUpload {
                        request: lease.request.clone(),
                        publication_id: lease.publication_id,
                        lease_id: lease.lease_id,
                        generation: lease.generation,
                        version_id: selected.version_id,
                        size_bytes,
                        sha256,
                    };
                    let grant = self
                        .cp
                        .reserve_artifact_upload(&lease, &selected, &output)
                        .await?;
                    let body = match grant.disposition {
                        ArtifactUploadDisposition::Upload => Some(body),
                        ArtifactUploadDisposition::ReceiptOnly => None,
                    };
                    self.cp
                        .upload_artifact(&lease, &selected, &output, &grant, body)
                        .await?;
                }
                let completed = self.cp.complete_artifacts(&lease).await?;
                self.finish(&manifest, &status, completed).await
            }
            settled @ ClaimedArtifactPublication::Settled { .. } => {
                self.finish(&manifest, &status, settled).await
            }
            ClaimedArtifactPublication::NotReady { .. } => unreachable!(),
        }
    }

    async fn finish(
        &self,
        manifest: &LaunchManifest,
        status: &LaunchStatus,
        claim: ClaimedArtifactPublication,
    ) -> Result<ClaimedArtifactPublication> {
        if let ClaimedArtifactPublication::Settled { receipt } = &claim {
            let cache = self.cache.clone();
            let manifest = manifest.clone();
            let status = status.clone();
            let receipt = receipt.clone();
            tokio::task::spawn_blocking(move || {
                cache.cleanup_artifact_staging(&manifest, &status, &receipt)?;
                cache.finish_artifact_cleanup(&status.request, &receipt)
            })
            .await
            .map_err(|_| ActuatorError::FilesystemTask)??;
        }
        Ok(claim)
    }
}

fn failure_reason(error: &CacheError) -> ArtifactPublicationFailureReason {
    match error {
        CacheError::Quota => ArtifactPublicationFailureReason::LimitExceeded,
        CacheError::LostArtifactStaging => ArtifactPublicationFailureReason::LostStaging,
        CacheError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ArtifactPublicationFailureReason::LostStaging
        }
        CacheError::Io(_) => ArtifactPublicationFailureReason::Unreadable,
        _ => ArtifactPublicationFailureReason::InvalidTree,
    }
}
fn measure(mut body: File, maximum: u64) -> crate::cache::Result<(File, u64, Sha256Digest)> {
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let count = body.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size.checked_add(count as u64).ok_or(CacheError::Quota)?;
        if size > maximum {
            return Err(CacheError::Quota);
        }
        hash.update(&buffer[..count]);
    }
    let digest = Sha256Digest::parse(
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
    .map_err(|_| CacheError::PinMismatch)?;
    Ok((body, size, digest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Bytes,
        extract::{OriginalUri, State},
        http::{HeaderMap, StatusCode},
        routing::post,
        Json, Router,
    };
    use std::collections::BTreeSet;
    use tokio::sync::Mutex;

    struct Fixture {
        claim: ClaimedArtifactPublication,
        status: LaunchStatus,
        lease: ArtifactPublicationLease,
        written: BTreeSet<i64>,
        receipt_reads: usize,
        missing: usize,
        lose_ack: bool,
        pending_once: bool,
        failure: Option<ArtifactPublicationFailureReason>,
        queue: std::path::PathBuf,
        status_calls: usize,
        lose_claim_ack: bool,
    }
    impl Fixture {
        fn settled(&self, state: ArtifactPublicationState) -> ClaimedArtifactPublication {
            ClaimedArtifactPublication::Settled {
                receipt: ArtifactPublicationReceipt {
                    request: self.lease.request.clone(),
                    publication_id: self.lease.publication_id,
                    state,
                    cleanup_receipt_id: 44,
                },
            }
        }
    }
    async fn respond(
        State(fixture): State<Arc<Mutex<Fixture>>>,
        OriginalUri(uri): OriginalUri,
        headers: HeaderMap,
        body: Bytes,
    ) -> (StatusCode, Json<serde_json::Value>) {
        // This is a transport/removal-state filesystem fixture, not evidence
        // of actual CP source authorization or native object settlement.
        assert_eq!(headers["authorization"], "Bearer owned-controller");
        assert!(uri.query().is_none());
        let operation = uri
            .path()
            .strip_prefix("/api/v1/workload-launches/1/")
            .unwrap();
        let mut fixture = fixture.lock().await;
        let value = match operation {
            "artifacts/claim" => {
                assert_eq!(
                    std::fs::read_dir(&fixture.queue).unwrap().count(),
                    1,
                    "durable public cleanup evidence must precede the FIRST claim"
                );
                if fixture.lose_claim_ack {
                    fixture.lose_claim_ack = false;
                    return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({})));
                }
                serde_json::to_value(&fixture.claim).unwrap()
            }
            "status" => {
                assert_eq!(headers[LAUNCH_DISPATCH_HEADER], "owned-dispatch");
                fixture.status_calls += 1;
                serde_json::to_value(&fixture.status).unwrap()
            }
            "artifacts/reserve-upload" => {
                assert_eq!(headers[ARTIFACT_PUBLICATION_HEADER], "owned-publication");
                let output: ReserveArtifactUpload = serde_json::from_slice(&body).unwrap();
                let bytes = if output.version_id == 1 {
                    &b""[..]
                } else {
                    &b"owned-file"[..]
                };
                assert_eq!(output.size_bytes, bytes.len() as u64);
                assert_eq!(output.sha256, Sha256Digest::of_bytes(bytes));
                serde_json::to_value(ArtifactUploadGrant {
                    operation_id: output.version_id + 100,
                    version_id: output.version_id,
                    size_bytes: output.size_bytes,
                    sha256: output.sha256,
                    expires_at: fixture.lease.expires_at,
                    token: LaunchToken::new("owned-upload".into()).unwrap(),
                    disposition: if fixture.written.contains(&output.version_id) {
                        ArtifactUploadDisposition::ReceiptOnly
                    } else {
                        ArtifactUploadDisposition::Upload
                    },
                })
                .unwrap()
            }
            "artifacts/101/body" | "artifacts/102/body" => {
                assert_eq!(headers[ARTIFACT_UPLOAD_HEADER], "owned-upload");
                assert!(!headers.contains_key(ARTIFACT_PUBLICATION_HEADER));
                let version = if operation == "artifacts/101/body" {
                    1
                } else {
                    2
                };
                let bytes = if version == 1 {
                    &b""[..]
                } else {
                    &b"owned-file"[..]
                };
                if fixture.written.insert(version) {
                    assert_eq!(&body[..], bytes);
                    if version == 2 && fixture.lose_ack {
                        fixture.lose_ack = false;
                        return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({})));
                    }
                } else {
                    assert!(body.is_empty(), "receipt replay must not upload again");
                    fixture.receipt_reads += 1;
                }
                serde_json::to_value(ArtifactUploadReceipt {
                    operation_id: version + 100,
                    version_id: version,
                    size_bytes: bytes.len() as u64,
                    sha256: Sha256Digest::of_bytes(bytes),
                    publication_receipt_id: version + 200,
                })
                .unwrap()
            }
            "artifacts/missing" => {
                let missing: MissingArtifactFile = serde_json::from_slice(&body).unwrap();
                assert_eq!(missing.version_id, 3);
                fixture.missing += 1;
                serde_json::to_value(ArtifactFileDispositionReceipt {
                    request: missing.request,
                    publication_id: missing.publication_id,
                    version_id: 3,
                    disposition: ArtifactFileDisposition::Missing,
                })
                .unwrap()
            }
            "artifacts/complete" | "artifacts/failure" => {
                if operation.ends_with("failure") {
                    let failure: FailArtifactPublication = serde_json::from_slice(&body).unwrap();
                    assert_eq!(Some(failure.reason), fixture.failure);
                    assert_eq!(fixture.missing, 0, "whole-tree failure is never Missing");
                }
                if fixture.pending_once {
                    fixture.pending_once = false;
                    serde_json::to_value(ClaimedArtifactPublication::NotReady {
                        request: fixture.lease.request.clone(),
                        state: ArtifactPublicationState::Publishing,
                    })
                    .unwrap()
                } else {
                    let settled = fixture.settled(if fixture.failure.is_some() {
                        ArtifactPublicationState::Incomplete
                    } else {
                        ArtifactPublicationState::Complete
                    });
                    fixture.claim = settled.clone();
                    serde_json::to_value(settled).unwrap()
                }
            }
            _ => panic!("unexpected effect; no resolve/Run/Engine fallback allowed"),
        };
        (StatusCode::OK, Json(value))
    }

    async fn fixture() -> (
        tempfile::TempDir,
        ArtifactPublisher,
        ReservedLaunch,
        std::path::PathBuf,
        Arc<Mutex<Fixture>>,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<std::io::Result<()>>,
    ) {
        let root = tempfile::tempdir().unwrap();
        let cache = Arc::new(
            CacheStore::open(
                root.path().into(),
                crate::cache::CacheLimits {
                    compressed_bytes: 4096,
                    expanded_bytes: 4096,
                    files: 16,
                },
            )
            .unwrap()
            .for_current_test_user(),
        );
        let spec = crate::test_support::spec(format!("sha256:{}", "a".repeat(64)));
        cache.prepare_private(&spec.manifest).unwrap();
        cache.prepare_artifact_staging(&spec.manifest).unwrap();
        let directory = root.path().join(format!(
            "private/{}/artifacts",
            spec.manifest.engine_name_nonce.simple()
        ));
        std::fs::write(directory.join("empty"), []).unwrap();
        std::fs::write(directory.join("full"), b"owned-file").unwrap();
        let request = LaunchRequest {
            launch_id: 1,
            admitted_digest: spec.manifest.admitted_digest().unwrap(),
        };
        let lease = ArtifactPublicationLease {
            request: request.clone(),
            publication_id: 2,
            generation: 1,
            lease_id: uuid::Uuid::new_v4(),
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(60),
            token: LaunchToken::new("owned-publication".into()).unwrap(),
        };
        let status = LaunchStatus {
            request: request.clone(),
            state: LaunchState::Stopped,
            runtime_state: LaunchRuntimeState::Unstarted,
            exit_outcome: None,
            resource: None,
            publication: LaunchPublicationState::NotRequired,
            capture: LaunchCaptureState::Complete,
            capture_deadline: None,
            stop_reason: Some(LaunchStopReason::Cancelled),
        };
        let state = Arc::new(Mutex::new(Fixture {
            claim: ClaimedArtifactPublication::Transfer {
                lease: lease.clone(),
                files: ["empty", "full", "missing"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, path)| PreparedArtifactFile {
                        version_id: index as i64 + 1,
                        relative_path: path.into(),
                        max_bytes: 64,
                    })
                    .collect(),
                limits: ArtifactLimits {
                    max_total_bytes: 256,
                    max_entries: 16,
                },
                deadline: lease.expires_at,
            },
            status,
            lease,
            written: BTreeSet::new(),
            receipt_reads: 0,
            missing: 0,
            lose_ack: true,
            pending_once: true,
            failure: None,
            queue: root.path().join("artifact-cleanup"),
            status_calls: 0,
            lose_claim_ack: false,
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let app = Router::new()
            .fallback(post(respond))
            .with_state(state.clone());
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        let cp = ControlPlane::new(
            origin.parse().unwrap(),
            LaunchToken::new("owned-controller".into()).unwrap(),
        )
        .unwrap();
        (
            root,
            ArtifactPublisher::new(cp, cache),
            ReservedLaunch {
                request,
                dispatch_token: LaunchToken::new("owned-dispatch".into()).unwrap(),
            },
            directory,
            state,
            stop,
            server,
        )
    }

    #[tokio::test]
    async fn lost_body_ack_replays_receipts_and_pending_keeps_staging_until_durable_settlement() {
        let (_root, publisher, request, directory, state, stop, server) = fixture().await;
        assert!(publisher.reconcile(&request).await.is_err());
        assert!(directory.join("full").exists());
        assert_eq!(
            publisher.cache.pending_artifact_cleanups().unwrap(),
            vec![request.request.clone()]
        );
        assert!(matches!(
            publisher.reconcile_queued(&request.request).await.unwrap(),
            ClaimedArtifactPublication::NotReady { .. }
        ));
        assert!(directory.join("empty").exists());
        assert!(matches!(
            publisher.reconcile_queued(&request.request).await.unwrap(),
            ClaimedArtifactPublication::Settled { .. }
        ));
        assert!(!directory.exists());
        assert!(publisher
            .cache
            .pending_artifact_cleanups()
            .unwrap()
            .is_empty());
        assert!(matches!(
            publisher.reconcile(&request).await.unwrap(),
            ClaimedArtifactPublication::Settled { .. }
        ));
        let state = state.lock().await;
        assert_eq!(state.written, BTreeSet::from([1, 2]));
        assert_eq!(state.receipt_reads, 4);
        assert_eq!(state.missing, 2);
        assert_eq!(state.status_calls, 2);
        drop(state);
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn invalid_tree_and_lost_root_are_explicit_failures_retained_while_native_ownership_pending(
    ) {
        for lost_root in [false, true] {
            let (root, publisher, request, directory, state, stop, server) = fixture().await;
            if lost_root {
                std::fs::remove_dir_all(&directory).unwrap();
                state.lock().await.failure = Some(ArtifactPublicationFailureReason::LostStaging);
            } else {
                let sentinel = root.path().join("owned-sentinel");
                std::fs::write(&sentinel, b"untouched").unwrap();
                std::os::unix::fs::symlink(&sentinel, directory.join("escape")).unwrap();
                state.lock().await.failure = Some(ArtifactPublicationFailureReason::InvalidTree);
            }
            let parent = directory.parent().unwrap();
            assert!(matches!(
                publisher.reconcile(&request).await.unwrap(),
                ClaimedArtifactPublication::NotReady { .. }
            ));
            assert!(parent.exists());
            assert!(matches!(
                publisher.reconcile(&request).await.unwrap(),
                ClaimedArtifactPublication::Settled { .. }
            ));
            assert!(!parent.exists());
            if !lost_root {
                assert_eq!(
                    std::fs::read(root.path().join("owned-sentinel")).unwrap(),
                    b"untouched"
                );
            }
            assert!(state.lock().await.written.is_empty());
            stop.send(()).unwrap();
            server.await.unwrap().unwrap();
        }
    }

    #[tokio::test]
    async fn missing_index_and_unsettled_status_cannot_resolve_or_publish_private_context() {
        let (root, publisher, request, directory, state, stop, server) = fixture().await;
        state.lock().await.status.state = LaunchState::Created;
        assert!(matches!(
            publisher.reconcile(&request).await,
            Err(ActuatorError::Authority)
        ));
        assert!(state.lock().await.written.is_empty());
        assert!(directory.exists());
        state.lock().await.status.state = LaunchState::Stopped;
        state.lock().await.failure = Some(ArtifactPublicationFailureReason::LostStaging);
        let manifests = root.path().join("manifests");
        let index = std::fs::read_dir(&manifests)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::remove_file(index).unwrap();
        assert!(publisher.reconcile(&request).await.is_err());
        assert!(
            directory.exists(),
            "missing index never authorizes guessing/deleting an owned nonce"
        );
        assert!(state.lock().await.written.is_empty());
        assert_eq!(state.lock().await.missing, 0);
        assert!(publisher
            .cache
            .pending_artifact_cleanups()
            .unwrap()
            .is_empty());
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn terminal_empty_inventory_ack_loss_recovers_from_public_queue_without_dispatch() {
        let (root, publisher, request, directory, state, stop, server) = fixture().await;
        // Empty inventory may settle at its first claim, so queue publication
        // must precede even that claim. Simulate a response lost after commit.
        let mut fixture = state.lock().await;
        fixture.claim = fixture.settled(ArtifactPublicationState::Complete);
        fixture.lose_claim_ack = true;
        drop(fixture);
        assert!(publisher.reconcile(&request).await.is_err());
        let cp = publisher.cp.clone();
        // No private context or dispatch credential is retained for recovery.
        drop(publisher);
        let cache = Arc::new(
            CacheStore::open(
                root.path().into(),
                crate::cache::CacheLimits {
                    compressed_bytes: 4096,
                    expanded_bytes: 4096,
                    files: 16,
                },
            )
            .unwrap()
            .for_current_test_user(),
        );
        let pending = cache.pending_artifact_cleanups().unwrap();
        assert_eq!(pending, vec![request.request.clone()]);
        let entry = std::fs::read_dir(root.path().join("artifact-cleanup"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let public = std::fs::read_to_string(entry.path()).unwrap();
        for private in [
            "owned-controller",
            "owned-dispatch",
            "owned-publication",
            "argv",
            "/bin/true",
        ] {
            assert!(!public.contains(private));
        }
        assert!(directory.exists());
        state.lock().await.status.state = LaunchState::Created;
        let restarted = ArtifactPublisher::new(cp, cache);
        assert!(matches!(
            restarted.reconcile_queued(&pending[0]).await.unwrap(),
            ClaimedArtifactPublication::Settled { .. }
        ));
        assert!(!directory.exists());
        assert!(restarted
            .cache
            .pending_artifact_cleanups()
            .unwrap()
            .is_empty());
        assert_eq!(state.lock().await.status_calls, 1);
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn aliased_or_noncanonical_public_queue_cannot_publish_or_delete_staging() {
        use std::os::unix::fs::PermissionsExt;
        let (root, publisher, request, directory, state, stop, server) = fixture().await;
        assert!(publisher.reconcile(&request).await.is_err());
        let path = std::fs::read_dir(root.path().join("artifact-cleanup"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let canonical = std::fs::read(&path).unwrap();
        let alias = root.path().join("aliased-evidence");
        std::fs::hard_link(&path, &alias).unwrap();
        assert!(publisher.reconcile_queued(&request.request).await.is_err());
        assert!(publisher.cache.pending_artifact_cleanups().is_err());
        std::fs::remove_file(alias).unwrap();
        let mut altered = canonical;
        altered.push(b'\n');
        std::fs::write(&path, altered).unwrap();
        assert!(publisher.reconcile_queued(&request.request).await.is_err());
        let sentinel = root.path().join("owned-sentinel");
        std::fs::write(&sentinel, b"untouched-private-marker").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&sentinel, &path).unwrap();
        assert!(publisher.reconcile_queued(&request.request).await.is_err());
        assert_eq!(
            std::fs::read(sentinel).unwrap(),
            b"untouched-private-marker"
        );
        assert!(directory.exists());
        assert_eq!(state.lock().await.status_calls, 1);
        assert_eq!(state.lock().await.written, BTreeSet::from([1, 2]));
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    }
}
