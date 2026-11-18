//! Finite file-publication transport. No frontend route accepts file paths or
//! these controller-only grants. CP effects remain authoritative; transport
//! success is accepted only with the exact typed durable receipt.
use super::*;
use attune_common::workload_artifact::*;
use chrono::Utc;

impl ControlPlane {
    pub async fn claim_artifacts(
        &self,
        request: &LaunchRequest,
    ) -> Result<ClaimedArtifactPublication> {
        let response = self
            .client
            .post(self.url(request.launch_id, &["artifacts", "claim"])?)
            .bearer_auth(self.credential().await.as_str())
            .json(request)
            .send()
            .await
            .map_err(|_| ControlError::Transport)?;
        let claim = decode(response).await?;
        validate_claim(&claim, request)?;
        Ok(claim)
    }

    pub async fn reserve_artifact_upload(
        &self,
        lease: &ArtifactPublicationLease,
        selected: &PreparedArtifactFile,
        output: &ReserveArtifactUpload,
    ) -> Result<ArtifactUploadGrant> {
        validate_output(lease, selected, output)?;
        let grant: ArtifactUploadGrant =
            self.artifact_post(lease, "reserve-upload", output).await?;
        validate_upload(lease, output, &grant)?;
        Ok(grant)
    }

    /// ReceiptOnly deliberately takes None and sends an empty body, even when
    /// the historical receipt describes a nonempty file. It grants no new PUT.
    /// Upload takes the already-opened, settled regular file, including size0.
    pub async fn upload_artifact(
        &self,
        lease: &ArtifactPublicationLease,
        selected: &PreparedArtifactFile,
        output: &ReserveArtifactUpload,
        grant: &ArtifactUploadGrant,
        body: Option<std::fs::File>,
    ) -> Result<ArtifactUploadReceipt> {
        validate_output(lease, selected, output)?;
        validate_upload(lease, output, grant)?;
        let (body, length) = match (grant.disposition, body) {
            (ArtifactUploadDisposition::ReceiptOnly, None) => (reqwest::Body::from(Vec::new()), 0),
            (ArtifactUploadDisposition::Upload, Some(body)) => {
                use std::os::unix::fs::MetadataExt;
                let metadata = body.metadata()?;
                if !metadata.is_file()
                    || metadata.nlink() != 1
                    || metadata.len() != output.size_bytes
                {
                    return Err(ControlError::Response);
                }
                let mut file = tokio::fs::File::from_std(body);
                file.seek(std::io::SeekFrom::Start(0)).await?;
                let deadline = grant.expires_at;
                let stream = futures::stream::try_unfold(
                    (file, output.size_bytes),
                    move |(mut file, remaining)| async move {
                        if Utc::now() >= deadline {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "artifact upload expired",
                            ));
                        }
                        if remaining == 0 {
                            return Ok(None);
                        }
                        let mut bytes = vec![0u8; remaining.min(65536) as usize];
                        let count = file.read(&mut bytes).await?;
                        if count == 0 {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "artifact file shortened",
                            ));
                        }
                        bytes.truncate(count);
                        Ok(Some((bytes, (file, remaining - count as u64))))
                    },
                );
                (reqwest::Body::wrap_stream(stream), output.size_bytes)
            }
            _ => return Err(ControlError::Response),
        };
        let remaining = remaining(grant.expires_at)?;
        let response = self
            .client
            .post(self.url(
                lease.request.launch_id,
                &["artifacts", &grant.operation_id.to_string(), "body"],
            )?)
            .bearer_auth(self.credential().await.as_str())
            // No publication token, lease token, dispatch or query credentials.
            .header(ARTIFACT_UPLOAD_HEADER, secret(&grant.token)?)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(reqwest::header::CONTENT_LENGTH, length)
            .timeout(remaining)
            .body(body)
            .send()
            .await
            .map_err(|_| ControlError::Transport)?;
        let receipt: ArtifactUploadReceipt = decode(response).await?;
        if receipt.operation_id != grant.operation_id
            || receipt.version_id != output.version_id
            || receipt.size_bytes != output.size_bytes
            || receipt.sha256 != output.sha256
            || receipt.publication_receipt_id <= 0
        {
            return Err(ControlError::Response);
        }
        Ok(receipt)
    }

    /// Called only for an actual nofollow NotFound from the exact inventory.
    /// Zero bytes, 403/404, timeout and unsettled native PUTs are not missing.
    pub async fn report_missing_artifact(
        &self,
        lease: &ArtifactPublicationLease,
        version_id: i64,
    ) -> Result<ArtifactFileDispositionReceipt> {
        if version_id <= 0 {
            return Err(ControlError::Response);
        }
        let receipt: ArtifactFileDispositionReceipt = self
            .artifact_post(
                lease,
                "missing",
                &MissingArtifactFile {
                    request: lease.request.clone(),
                    publication_id: lease.publication_id,
                    generation: lease.generation,
                    lease_id: lease.lease_id,
                    version_id,
                },
            )
            .await?;
        if receipt.request != lease.request
            || receipt.publication_id != lease.publication_id
            || receipt.version_id != version_id
            || receipt.disposition != ArtifactFileDisposition::Missing
        {
            return Err(ControlError::Response);
        }
        Ok(receipt)
    }

    pub async fn complete_artifacts(
        &self,
        lease: &ArtifactPublicationLease,
    ) -> Result<ClaimedArtifactPublication> {
        let claim: ClaimedArtifactPublication = self
            .artifact_post(lease, "complete", &lease.request)
            .await?;
        validate_claim(&claim, &lease.request)?;
        if matches!(claim, ClaimedArtifactPublication::Transfer { .. }) {
            return Err(ControlError::Response);
        }
        Ok(claim)
    }

    /// Whole-tree failure is not Missing and does not prove native absence.
    /// Only the CP's settled Incomplete receipt authorizes eventual cleanup.
    pub async fn fail_artifacts(
        &self,
        lease: &ArtifactPublicationLease,
        reason: ArtifactPublicationFailureReason,
    ) -> Result<ClaimedArtifactPublication> {
        let claim: ClaimedArtifactPublication = self
            .artifact_post(
                lease,
                "failure",
                &FailArtifactPublication {
                    request: lease.request.clone(),
                    publication_id: lease.publication_id,
                    lease_id: lease.lease_id,
                    generation: lease.generation,
                    reason,
                },
            )
            .await?;
        validate_claim(&claim, &lease.request)?;
        match &claim {
            ClaimedArtifactPublication::NotReady {
                state: ArtifactPublicationState::Pending | ArtifactPublicationState::Publishing,
                ..
            } => {}
            ClaimedArtifactPublication::Settled { receipt }
                if receipt.state == ArtifactPublicationState::Incomplete => {}
            _ => return Err(ControlError::Response),
        }
        Ok(claim)
    }

    async fn artifact_post<T: Serialize + ?Sized, O: DeserializeOwned>(
        &self,
        lease: &ArtifactPublicationLease,
        suffix: &str,
        body: &T,
    ) -> Result<O> {
        validate_publication_lease(lease)?;
        let response = self
            .client
            .post(self.url(lease.request.launch_id, &["artifacts", suffix])?)
            .bearer_auth(self.credential().await.as_str())
            .header(ARTIFACT_PUBLICATION_HEADER, secret(&lease.token)?)
            .json(body)
            .timeout(remaining(lease.expires_at)?)
            .send()
            .await
            .map_err(|_| ControlError::Transport)?;
        decode(response).await
    }
}

fn remaining(deadline: chrono::DateTime<Utc>) -> Result<Duration> {
    let value = (deadline - Utc::now())
        .to_std()
        .map_err(|_| ControlError::Response)?;
    if value.is_zero() {
        return Err(ControlError::Response);
    }
    Ok(value.min(Duration::from_secs(30)))
}
fn validate_publication_lease(lease: &ArtifactPublicationLease) -> Result<()> {
    if lease.request.launch_id <= 0
        || lease.publication_id <= 0
        || lease.generation <= 0
        || lease.lease_id.is_nil()
    {
        return Err(ControlError::Response);
    }
    remaining(lease.expires_at)?;
    Ok(())
}
fn validate_output(
    lease: &ArtifactPublicationLease,
    selected: &PreparedArtifactFile,
    output: &ReserveArtifactUpload,
) -> Result<()> {
    validate_publication_lease(lease)?;
    if output.request != lease.request
        || output.publication_id != lease.publication_id
        || output.generation != lease.generation
        || output.lease_id != lease.lease_id
        || selected.version_id <= 0
        || output.version_id != selected.version_id
        || selected.max_bytes == 0
        || output.size_bytes > selected.max_bytes
    {
        return Err(ControlError::Response);
    }
    Ok(())
}
fn validate_upload(
    lease: &ArtifactPublicationLease,
    output: &ReserveArtifactUpload,
    grant: &ArtifactUploadGrant,
) -> Result<()> {
    if grant.operation_id <= 0
        || grant.version_id != output.version_id
        || grant.size_bytes != output.size_bytes
        || grant.sha256 != output.sha256
        || grant.expires_at > lease.expires_at
    {
        return Err(ControlError::Response);
    }
    remaining(grant.expires_at)?;
    Ok(())
}
fn validate_claim(claim: &ClaimedArtifactPublication, expected: &LaunchRequest) -> Result<()> {
    match claim {
        ClaimedArtifactPublication::NotReady { request, state } => {
            if request != expected
                || matches!(
                    state,
                    ArtifactPublicationState::Complete | ArtifactPublicationState::Incomplete
                )
            {
                return Err(ControlError::Response);
            }
        }
        ClaimedArtifactPublication::Transfer {
            lease,
            files,
            limits,
            deadline,
        } => {
            validate_publication_lease(lease)?;
            if &lease.request != expected || lease.expires_at > *deadline {
                return Err(ControlError::Response);
            }
            validate_inventory(files, *limits).map_err(|_| ControlError::Response)?;
        }
        ClaimedArtifactPublication::Settled { receipt } => {
            if &receipt.request != expected
                || receipt.publication_id <= 0
                || receipt.cleanup_receipt_id <= 0
                || !matches!(
                    receipt.state,
                    ArtifactPublicationState::Complete | ArtifactPublicationState::Incomplete
                )
            {
                return Err(ControlError::Response);
            }
        }
    }
    Ok(())
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
    use std::{collections::VecDeque, io::Write};

    struct Step {
        path: String,
        grant: Option<(&'static str, &'static str)>,
        body: Vec<u8>,
        status: StatusCode,
        response: serde_json::Value,
    }
    async fn respond(
        State(steps): State<Arc<tokio::sync::Mutex<VecDeque<Step>>>>,
        OriginalUri(uri): OriginalUri,
        headers: HeaderMap,
        body: Bytes,
    ) -> (StatusCode, Json<serde_json::Value>) {
        let step = steps
            .lock()
            .await
            .pop_front()
            .expect("unexpected wire effect");
        assert_eq!(uri.path(), step.path);
        assert!(uri.query().is_none());
        assert_eq!(
            headers[reqwest::header::AUTHORIZATION],
            "Bearer owned-controller"
        );
        for header in [
            ARTIFACT_UPLOAD_HEADER,
            ARTIFACT_PUBLICATION_HEADER,
            LAUNCH_LEASE_HEADER,
            LAUNCH_DISPATCH_HEADER,
            LAUNCH_IO_HEADER,
        ] {
            if let Some((expected, value)) = step.grant.filter(|(name, _)| *name == header) {
                assert_eq!(headers[expected], value);
            } else {
                assert!(
                    !headers.contains_key(header),
                    "unrelated credential on effect"
                );
            }
        }
        assert_eq!(&body[..], &step.body);
        (step.status, Json(step.response))
    }
    #[tokio::test]
    async fn exact_artifact_wire_preserves_empty_files_receipt_only_and_nonterminal_denials() {
        // This server checks the frozen HTTP contract, not CP authorization or
        // native provider settlement. Those need the actual API integration.
        let request = LaunchRequest {
            launch_id: 1,
            admitted_digest: Sha256Digest::of_bytes(b"manifest"),
        };
        let lease = ArtifactPublicationLease {
            request: request.clone(),
            publication_id: 2,
            generation: 1,
            lease_id: uuid::Uuid::new_v4(),
            expires_at: Utc::now() + chrono::Duration::seconds(60),
            token: LaunchToken::new("owned-publication".into()).unwrap(),
        };
        let files = vec![
            PreparedArtifactFile {
                version_id: 3,
                relative_path: "files/empty".into(),
                max_bytes: 64,
            },
            PreparedArtifactFile {
                version_id: 4,
                relative_path: "files/nonempty".into(),
                max_bytes: 64,
            },
        ];
        let transfer = ClaimedArtifactPublication::Transfer {
            lease: lease.clone(),
            files: files.clone(),
            limits: ArtifactLimits {
                max_total_bytes: 128,
                max_entries: 3,
            },
            deadline: lease.expires_at + chrono::Duration::seconds(60),
        };
        let make_step = |suffix: &str, grant, body, response, status| Step {
            path: format!("/api/v1/workload-launches/1/artifacts/{suffix}"),
            grant,
            body,
            response,
            status,
        };
        let mut steps = VecDeque::from([make_step(
            "claim",
            None,
            serde_json::to_vec(&request).unwrap(),
            serde_json::to_value(&transfer).unwrap(),
            StatusCode::OK,
        )]);
        let mut cases = Vec::new();
        for (index, (bytes, disposition)) in [
            (b"".as_slice(), ArtifactUploadDisposition::Upload),
            (b"hello".as_slice(), ArtifactUploadDisposition::ReceiptOnly),
        ]
        .into_iter()
        .enumerate()
        {
            let file = files[index].clone();
            let output = ReserveArtifactUpload {
                request: request.clone(),
                publication_id: 2,
                lease_id: lease.lease_id,
                generation: 1,
                version_id: file.version_id,
                size_bytes: bytes.len() as u64,
                sha256: Sha256Digest::of_bytes(bytes),
            };
            let grant = ArtifactUploadGrant {
                operation_id: 7 + index as i64,
                version_id: file.version_id,
                size_bytes: output.size_bytes,
                sha256: output.sha256.clone(),
                expires_at: lease.expires_at,
                token: LaunchToken::new("owned-upload".into()).unwrap(),
                disposition,
            };
            steps.push_back(make_step(
                "reserve-upload",
                Some((ARTIFACT_PUBLICATION_HEADER, "owned-publication")),
                serde_json::to_vec(&output).unwrap(),
                serde_json::to_value(&grant).unwrap(),
                StatusCode::OK,
            ));
            let receipt = ArtifactUploadReceipt {
                operation_id: grant.operation_id,
                version_id: file.version_id,
                size_bytes: output.size_bytes,
                sha256: output.sha256.clone(),
                publication_receipt_id: 20 + index as i64,
            };
            steps.push_back(make_step(
                &format!("{}/body", grant.operation_id),
                Some((ARTIFACT_UPLOAD_HEADER, "owned-upload")),
                Vec::new(),
                serde_json::to_value(&receipt).unwrap(),
                StatusCode::OK,
            ));
            cases.push((file, output, grant, receipt));
        }
        let missing = MissingArtifactFile {
            request: request.clone(),
            publication_id: 2,
            lease_id: lease.lease_id,
            generation: 1,
            version_id: 3,
        };
        steps.push_back(make_step(
            "missing",
            Some((ARTIFACT_PUBLICATION_HEADER, "owned-publication")),
            serde_json::to_vec(&missing).unwrap(),
            serde_json::json!({}),
            StatusCode::CONFLICT,
        ));
        let not_ready = ClaimedArtifactPublication::NotReady {
            request: request.clone(),
            state: ArtifactPublicationState::Publishing,
        };
        steps.push_back(make_step(
            "complete",
            Some((ARTIFACT_PUBLICATION_HEADER, "owned-publication")),
            serde_json::to_vec(&request).unwrap(),
            serde_json::to_value(&not_ready).unwrap(),
            StatusCode::OK,
        ));
        let settled = ClaimedArtifactPublication::Settled {
            receipt: ArtifactPublicationReceipt {
                request: request.clone(),
                publication_id: 2,
                state: ArtifactPublicationState::Complete,
                cleanup_receipt_id: 30,
            },
        };
        steps.push_back(make_step(
            "complete",
            Some((ARTIFACT_PUBLICATION_HEADER, "owned-publication")),
            serde_json::to_vec(&request).unwrap(),
            serde_json::to_value(&settled).unwrap(),
            StatusCode::OK,
        ));
        steps.push_back(make_step(
            "claim",
            None,
            serde_json::to_vec(&request).unwrap(),
            serde_json::json!({}),
            StatusCode::FORBIDDEN,
        ));
        let steps = Arc::new(tokio::sync::Mutex::new(steps));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let app = Router::new()
            .fallback(post(respond))
            .with_state(steps.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        let cp = ControlPlane::new(
            format!("http://{address}/").parse().unwrap(),
            LaunchToken::new("owned-controller".into()).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            cp.claim_artifacts(&request).await.unwrap(),
            ClaimedArtifactPublication::Transfer { .. }
        ));
        for (selected, output, expected, receipt) in cases {
            let grant = cp
                .reserve_artifact_upload(&lease, &selected, &output)
                .await
                .unwrap();
            assert_eq!(grant.operation_id, expected.operation_id);
            let mut empty = tempfile::NamedTempFile::new().unwrap();
            empty.write_all(b"").unwrap();
            let body = match grant.disposition {
                ArtifactUploadDisposition::Upload => {
                    assert!(cp
                        .upload_artifact(&lease, &selected, &output, &grant, None)
                        .await
                        .is_err());
                    Some(empty.as_file().try_clone().unwrap())
                }
                ArtifactUploadDisposition::ReceiptOnly => {
                    assert!(cp
                        .upload_artifact(
                            &lease,
                            &selected,
                            &output,
                            &grant,
                            Some(empty.as_file().try_clone().unwrap())
                        )
                        .await
                        .is_err());
                    None
                }
            };
            assert_eq!(
                cp.upload_artifact(&lease, &selected, &output, &grant, body)
                    .await
                    .unwrap(),
                receipt
            );
        }
        assert!(matches!(
            cp.report_missing_artifact(&lease, 3).await,
            Err(ControlError::Rejected(409))
        ));
        assert!(matches!(
            cp.complete_artifacts(&lease).await.unwrap(),
            ClaimedArtifactPublication::NotReady { .. }
        ));
        assert!(matches!(
            cp.complete_artifacts(&lease).await.unwrap(),
            ClaimedArtifactPublication::Settled { .. }
        ));
        assert!(matches!(
            cp.claim_artifacts(&request).await,
            Err(ControlError::Rejected(403))
        ));
        assert!(steps.lock().await.is_empty());
        stop.send(()).unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn tree_failure_is_not_missing_or_cleanup_before_native_settlement() {
        let request = LaunchRequest {
            launch_id: 1,
            admitted_digest: Sha256Digest::of_bytes(b"manifest"),
        };
        let lease = ArtifactPublicationLease {
            request: request.clone(),
            publication_id: 2,
            generation: 1,
            lease_id: uuid::Uuid::new_v4(),
            expires_at: Utc::now() + chrono::Duration::seconds(60),
            token: LaunchToken::new("owned-publication".into()).unwrap(),
        };
        let failure = FailArtifactPublication {
            request: request.clone(),
            publication_id: 2,
            lease_id: lease.lease_id,
            generation: 1,
            reason: ArtifactPublicationFailureReason::LimitExceeded,
        };
        let step = |response, status| Step {
            path: "/api/v1/workload-launches/1/artifacts/failure".into(),
            grant: Some((ARTIFACT_PUBLICATION_HEADER, "owned-publication")),
            body: serde_json::to_vec(&failure).unwrap(),
            response,
            status,
        };
        let receipt = ArtifactPublicationReceipt {
            request: request.clone(),
            publication_id: 2,
            state: ArtifactPublicationState::Incomplete,
            cleanup_receipt_id: 31,
        };
        let steps = Arc::new(tokio::sync::Mutex::new(VecDeque::from([
            step(
                serde_json::to_value(ClaimedArtifactPublication::NotReady {
                    request: request.clone(),
                    state: ArtifactPublicationState::Publishing,
                })
                .unwrap(),
                StatusCode::OK,
            ),
            step(
                serde_json::to_value(ClaimedArtifactPublication::Settled {
                    receipt: receipt.clone(),
                })
                .unwrap(),
                StatusCode::OK,
            ),
            step(
                serde_json::to_value(ClaimedArtifactPublication::Settled {
                    receipt: ArtifactPublicationReceipt {
                        state: ArtifactPublicationState::Complete,
                        ..receipt
                    },
                })
                .unwrap(),
                StatusCode::OK,
            ),
            step(serde_json::json!({}), StatusCode::FORBIDDEN),
        ])));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let app = Router::new()
            .fallback(post(respond))
            .with_state(steps.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        let cp = ControlPlane::new(
            format!("http://{address}/").parse().unwrap(),
            LaunchToken::new("owned-controller".into()).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            cp.fail_artifacts(&lease, failure.reason).await.unwrap(),
            ClaimedArtifactPublication::NotReady { .. }
        ));
        assert!(matches!(
            cp.fail_artifacts(&lease, failure.reason).await.unwrap(),
            ClaimedArtifactPublication::Settled { .. }
        ));
        assert!(matches!(
            cp.fail_artifacts(&lease, failure.reason).await,
            Err(ControlError::Response)
        ));
        assert!(matches!(
            cp.fail_artifacts(&lease, failure.reason).await,
            Err(ControlError::Rejected(403))
        ));
        assert!(steps.lock().await.is_empty());
        stop.send(()).unwrap();
        server.await.unwrap();
    }
}
