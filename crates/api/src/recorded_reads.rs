//! Selected-version recorded reads. Never authorize one version then refetch a
//! different latest version after releasing the transaction.
use crate::{
    auth::middleware::AuthenticatedUser,
    middleware::{ApiError, ApiResult},
};
use attune_common::{
    models::{Artifact, ArtifactBodyState, ArtifactClassification, ArtifactVersion},
    rbac::Action,
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository},
        runtime_read::{RuntimeReadOrigin, RuntimeReadRepository, RuntimeReadTarget},
        runtime_target::RuntimeTarget,
        FindById,
    },
};
use sqlx::PgPool;

fn denied() -> ApiError {
    ApiError::Forbidden("Selected artifact content is unavailable".into())
}

#[derive(Clone, Copy, Debug)]
enum ReadAcquisitionStage {
    Candidate,
    ArtifactOwner,
    SensorSource,
    SelectedVersion,
    SelectionChanged,
    BodyNotReady,
    LogBinding,
    UnexpectedLogManifest,
    CaptureRegistry,
    SourceFreshness,
    CaptureCommit,
    FinalSourceFreshness,
    FinalCredentialFreshness,
    TokenDeadline,
}

// Emit only a fixed enum. Do not log the underlying error, request, source
// identities, paths, ciphertext, provenance or plaintext.
fn read_stage_error(stage: ReadAcquisitionStage, error: ApiError) -> ApiError {
    tracing::warn!(recorded_read_stage = ?stage, "Recorded version acquisition denied");
    error
}

fn read_stage_denied(stage: ReadAcquisitionStage) -> ApiError {
    read_stage_error(stage, denied())
}

pub(crate) async fn artifact(
    pool: &PgPool,
    user: &AuthenticatedUser,
    id: i64,
    reference: Option<&str>,
    encryption_key: Option<&str>,
) -> ApiResult<Artifact> {
    let mut tx = pool.begin().await?;
    let authority = user
        .validate_recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(denied)?;
    let links = ArtifactRepository::linked_execution_ids_bounded(&mut *tx, id, 257).await?;
    if links.len() > 256 {
        return Err(denied());
    }
    let artifact = ArtifactRepository::find_by_id(&mut *tx, id)
        .await?
        .ok_or_else(denied)?;
    // A root projection is not a selected version. Creator links alone do not
    // turn metadata into execution content. Root data and runtime-log roots
    // still retain the linked source checks; versions keep their own checks.
    let content_links = if artifact.data.as_ref().is_some_and(|data| !data.is_null())
        || artifact.classification == ArtifactClassification::RuntimeLog
    {
        links.clone()
    } else {
        Vec::new()
    };
    let mut provenance = Vec::new();
    let mut budget = crate::recorded_disclosure::OriginBudget::with_encryption_key(encryption_key);
    for execution in &content_links {
        let rows = crate::recorded_disclosure::execution_origins(&mut tx, *execution).await?;
        budget.prepare_receipts(&mut tx, &rows).await?;
    }
    RuntimeReadRepository::lock_open(&mut tx, authority.credential_binding().admission_id).await?;
    let target = crate::recorded_disclosure::require_target(
        &mut tx,
        &authority,
        RuntimeTarget::Artifact(id),
        &[Action::Read],
    )
    .await?;
    let current = ArtifactRepository::find_by_id(&mut *tx, id)
        .await?
        .ok_or_else(denied)?;
    if current.data != artifact.data
        || current.classification != artifact.classification
        || current.scope != artifact.scope
    {
        return Err(denied());
    }
    let artifact = current;
    for execution in &content_links {
        crate::recorded_disclosure::require_target(
            &mut tx,
            &authority,
            RuntimeTarget::Execution(*execution),
            &[Action::Read, Action::Decrypt],
        )
        .await?;
        let binding = attune_common::repositories::schema_contract::SchemaContractRepository::find(
            &mut tx,
            attune_common::repositories::schema_contract::BindingOwner::Execution,
            *execution,
        )
        .await?
        .ok_or_else(denied)?;
        let origins = crate::recorded_disclosure::execution_origins(&mut tx, *execution).await?;
        crate::secret_disclosure::runtime_log_requires_disclosure(&binding, &origins)?;
        crate::recorded_disclosure::authorize_origins_bounded(
            &mut tx,
            &authority,
            &origins,
            &mut budget,
        )
        .await?;
        provenance.extend(crate::recorded_disclosure::snapshot(&origins)?);
    }
    if artifact.scope == attune_common::models::OwnerType::Sensor {
        return Err(denied());
    }
    if reference.is_some_and(|reference| artifact.r#ref != reference)
        || ArtifactRepository::linked_execution_ids_bounded(&mut *tx, id, 257).await? != links
        || (artifact.classification == ArtifactClassification::RuntimeLog && links.is_empty())
    {
        return Err(denied());
    }
    RuntimeReadRepository::record(
        &mut tx,
        &authority,
        RuntimeReadOrigin {
            target: RuntimeReadTarget::Artifact {
                artifact: id,
                owner: attune_common::repositories::runtime_read::RuntimeReadOwner::of(&target),
                executions: content_links,
            },
            provenance,
            parents: budget.retained_parents(),
        },
    )
    .await?;
    budget.ensure_receipts_fresh_on(&mut tx).await?;
    tx.commit().await?;
    budget.ensure_receipts_fresh()?;
    authority.ensure_fresh()?;
    if chrono::Utc::now().timestamp() >= user.claims.exp {
        return Err(denied());
    }
    Ok(artifact)
}

pub(crate) async fn history(
    pool: &PgPool,
    user: &AuthenticatedUser,
    id: i64,
    encryption_key: Option<&str>,
) -> ApiResult<Vec<attune_common::repositories::artifact::ArtifactVersionSummaryRow>> {
    use attune_common::repositories::artifact::ArtifactVersionSummaryRow;
    let mut tx = pool.begin().await?;
    crate::recorded_consumers::authorize_target(
        &mut tx,
        user,
        RuntimeTarget::Artifact(id),
        Action::Read,
    )
    .await?;
    tx.commit().await?;
    let candidates = ArtifactVersionRepository::bounded_summaries(pool, id).await?;
    if candidates.len() > 256 {
        return Err(denied());
    }
    let mut visible = Vec::new();
    let mut captured_reads = Vec::new();
    for candidate in candidates {
        let (selected, captured) =
            match version(pool, user, id, Some(candidate.version), encryption_key).await {
                Ok((_, selected, captured)) => (selected, captured),
                Err(ApiError::Forbidden(_) | ApiError::NotFound(_)) => continue,
                Err(error) => return Err(error),
            };
        captured_reads.push(captured);
        visible.push(ArtifactVersionSummaryRow {
            id: selected.id,
            version: selected.version,
            execution: selected.execution,
            content_type: selected.content_type,
            size_bytes: selected.size_bytes,
            file_path: selected.file_path,
            body_state: selected.body_state,
            object_key: selected.object_key,
            provider_version: selected.provider_version,
            sha256: selected.sha256,
            created_by: selected.created_by,
            created: selected.created,
        });
    }
    // Reauthorize the exact selected snapshots together. A prior per-version
    // decision is not a permit across the later collection/query waits.
    let mut tx = pool.begin().await?;
    let authority = user
        .validate_recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(denied)?;
    let rows = captured_reads
        .into_iter()
        .map(|origin| {
            Ok(
                attune_common::repositories::execution_secret_value::SecretProvenanceRow {
                    entity_type: attune_common::repositories::runtime_read::ENTITY_RUNTIME_READ
                        .into(),
                    entity_id: authority.credential_binding().execution_id,
                    json_path: String::new(),
                    source_kind: "provenance".into(),
                    source_ref: Some(
                        serde_json::to_string(
                            &attune_common::secret_provenance::SecretProvenance {
                                origins: vec![origin],
                                templates: vec![],
                            },
                        )
                        .map_err(|_| denied())?,
                    ),
                },
            )
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let budget = crate::recorded_disclosure::authorize_origins_with_key(
        &mut tx,
        &authority,
        &rows,
        encryption_key,
    )
    .await?;
    crate::recorded_disclosure::require_target(
        &mut tx,
        &authority,
        RuntimeTarget::Artifact(id),
        &[Action::Read],
    )
    .await?;
    authority.ensure_fresh_on(&mut tx).await?;
    budget.ensure_receipts_fresh_on(&mut tx).await?;
    tx.commit().await?;
    budget.ensure_receipts_fresh()?;
    authority.ensure_fresh()?;
    if chrono::Utc::now().timestamp() >= user.claims.exp {
        return Err(denied());
    }
    Ok(visible)
}

pub(crate) async fn version(
    pool: &PgPool,
    user: &AuthenticatedUser,
    artifact: i64,
    number: Option<i32>,
    encryption_key: Option<&str>,
) -> ApiResult<(
    Artifact,
    ArtifactVersion,
    attune_common::secret_provenance::SecretOrigin,
)> {
    let mut tx = pool.begin().await?;
    let authority = user
        .validate_recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(denied)?;
    let candidate = match number {
        Some(number) => {
            ArtifactVersionRepository::find_by_version(&mut *tx, artifact, number).await?
        }
        None => ArtifactVersionRepository::find_latest(&mut *tx, artifact).await?,
    }
    .ok_or_else(|| read_stage_denied(ReadAcquisitionStage::Candidate))?;
    let mut origins = Vec::new();
    let mut budget = crate::recorded_disclosure::OriginBudget::with_encryption_key(encryption_key);
    let file_origin = crate::file_origins::reference(&mut tx, candidate.id).await?;
    let file_rows = file_origin
        .map(|file_origin| -> ApiResult<_> {
            Ok(vec![
                attune_common::repositories::execution_secret_value::SecretProvenanceRow {
                    entity_type: "artifact_file_body".into(),
                    entity_id: candidate.id,
                    json_path: String::new(),
                    source_kind: "provenance".into(),
                    source_ref: Some(serde_json::to_string(&file_origin).map_err(|_| denied())?),
                },
            ])
        })
        .transpose()?;
    if let Some(rows) = &file_rows {
        budget.prepare_receipts(&mut tx, rows).await?;
    } else if let Some(execution) = candidate.execution {
        let rows = crate::recorded_disclosure::execution_origins(&mut tx, execution).await?;
        budget.prepare_receipts(&mut tx, &rows).await?;
    }
    RuntimeReadRepository::lock_open(&mut tx, authority.credential_binding().admission_id).await?;
    if let Some(rows) = file_rows {
        crate::recorded_disclosure::authorize_origins_bounded(
            &mut tx,
            &authority,
            &rows,
            &mut budget,
        )
        .await?;
        origins.extend(rows);
    } else if let Some(execution) = candidate.execution {
        crate::recorded_disclosure::require_target(
            &mut tx,
            &authority,
            RuntimeTarget::Execution(execution),
            &[Action::Read, Action::Decrypt],
        )
        .await?;
        let binding = attune_common::repositories::schema_contract::SchemaContractRepository::find(
            &mut tx,
            attune_common::repositories::schema_contract::BindingOwner::Execution,
            execution,
        )
        .await?
        .ok_or_else(denied)?;
        origins = crate::recorded_disclosure::execution_origins(&mut tx, execution).await?;
        crate::secret_disclosure::runtime_log_requires_disclosure(&binding, &origins)?;
        crate::recorded_disclosure::authorize_origins_bounded(
            &mut tx,
            &authority,
            &origins,
            &mut budget,
        )
        .await?;
    }
    let target = crate::recorded_disclosure::require_target(
        &mut tx,
        &authority,
        RuntimeTarget::Artifact(artifact),
        &[Action::Read, Action::Decrypt],
    )
    .await?;
    let artifact = ArtifactRepository::find_by_id(&mut *tx, artifact)
        .await?
        .ok_or_else(|| read_stage_denied(ReadAcquisitionStage::ArtifactOwner))?;
    // Sensor logs need generation/stream-specific encryption and delivery. Do
    // not infer their source from mutable root metadata or an action log shape.
    if artifact.scope == attune_common::models::OwnerType::Sensor {
        return Err(read_stage_denied(ReadAcquisitionStage::SensorSource));
    }
    let selected = ArtifactVersionRepository::find_selected_for_share(&mut tx, artifact.id, number)
        .await?
        .ok_or_else(|| read_stage_denied(ReadAcquisitionStage::SelectedVersion))?;
    if selected.id != candidate.id || selected.execution != candidate.execution {
        return Err(read_stage_denied(ReadAcquisitionStage::SelectionChanged));
    }
    if (selected.file_path.is_some() && selected.body_state != Some(ArtifactBodyState::Ready))
        || matches!(
            selected.body_state,
            Some(
                ArtifactBodyState::Pending
                    | ArtifactBodyState::Deleting
                    | ArtifactBodyState::CleanupClaimed
            )
        )
    {
        return Err(read_stage_denied(ReadAcquisitionStage::BodyNotReady));
    }
    let encrypted_manifest = selected
        .meta
        .as_ref()
        .and_then(|meta| meta.get("encrypted_log_manifest"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let is_log = artifact.classification == ArtifactClassification::RuntimeLog
        || ArtifactVersionRepository::has_runtime_log_binding(&mut tx, selected.id).await?;
    if is_log
        && (selected.execution.is_none()
            || selected.body_state != Some(ArtifactBodyState::Ready)
            || selected
                .meta
                .as_ref()
                .and_then(|meta| meta.get("log_encoding"))
                .and_then(serde_json::Value::as_str)
                != Some(attune_common::runtime_read_log::ENCODING)
            || encrypted_manifest.as_ref().is_none_or(|manifest| {
                manifest.len() > attune_common::runtime_read_log::MAX_MANIFEST_BYTES
            })
            || !ArtifactVersionRepository::log_is_sealed(&mut tx, selected.id).await?)
    {
        return Err(read_stage_denied(ReadAcquisitionStage::LogBinding));
    }
    if !is_log && encrypted_manifest.is_some() {
        return Err(read_stage_denied(
            ReadAcquisitionStage::UnexpectedLogManifest,
        ));
    }
    let captured = RuntimeReadRepository::record(
        &mut tx,
        &authority,
        RuntimeReadOrigin {
            target: RuntimeReadTarget::ArtifactVersion {
                artifact: artifact.id,
                version: selected.id,
                execution: selected.execution,
                owner: attune_common::repositories::runtime_read::RuntimeReadOwner::of(&target),
                encrypted_manifest,
            },
            provenance: crate::recorded_disclosure::snapshot(&origins)?,
            parents: budget.retained_parents(),
        },
    )
    .await
    .map_err(|error| read_stage_error(ReadAcquisitionStage::CaptureRegistry, error.into()))?;
    budget
        .ensure_receipts_fresh_on(&mut tx)
        .await
        .map_err(|error| read_stage_error(ReadAcquisitionStage::SourceFreshness, error))?;
    tx.commit()
        .await
        .map_err(|error| read_stage_error(ReadAcquisitionStage::CaptureCommit, error.into()))?;
    budget
        .ensure_receipts_fresh()
        .map_err(|error| read_stage_error(ReadAcquisitionStage::FinalSourceFreshness, error))?;
    authority.ensure_fresh().map_err(|error| {
        read_stage_error(ReadAcquisitionStage::FinalCredentialFreshness, error.into())
    })?;
    if chrono::Utc::now().timestamp() >= user.claims.exp {
        return Err(read_stage_denied(ReadAcquisitionStage::TokenDeadline));
    }
    Ok((artifact, selected, captured))
}

/// Stream delivery rechecks the same immutable version before each chunk. A
/// revoked source or changed selection terminates the body, not a metadata-only
/// check followed by unrestricted streaming.
pub(crate) async fn guard_download(
    state: std::sync::Arc<crate::state::AppState>,
    user: AuthenticatedUser,
    captured: attune_common::secret_provenance::SecretOrigin,
    response: axum::response::Response,
) -> ApiResult<axum::response::Response> {
    let provenance = serde_json::to_string(&attune_common::secret_provenance::SecretProvenance {
        origins: vec![captured],
        templates: vec![],
    })
    .map_err(|_| denied())?;
    let (parts, body) = response.into_parts();
    let authorize = move || {
        let (state, user, provenance) = (state.clone(), user.clone(), provenance.clone());
        async move {
            let mut tx = state
                .db
                .begin()
                .await
                .map_err(|_| std::io::Error::other("Artifact delivery interrupted"))?;
            let authority = user
                .validate_recorded_runtime_authority(&mut tx)
                .await
                .map_err(|_| std::io::Error::other("Artifact delivery authority expired"))?
                .ok_or_else(|| std::io::Error::other("Artifact delivery authority expired"))?;
            let row = attune_common::repositories::execution_secret_value::SecretProvenanceRow {
                entity_type: attune_common::repositories::runtime_read::ENTITY_RUNTIME_READ.into(),
                entity_id: authority.credential_binding().execution_id,
                json_path: String::new(),
                source_kind: "provenance".into(),
                source_ref: Some(provenance.clone()),
            };
            let budget = crate::recorded_disclosure::authorize_origins_with_key(
                &mut tx,
                &authority,
                &[row],
                state.config.security.encryption_key.as_deref(),
            )
            .await
            .map_err(|_| std::io::Error::other("Artifact delivery source expired"))?;
            authority
                .ensure_fresh_on(&mut tx)
                .await
                .map_err(|_| std::io::Error::other("Artifact delivery authority expired"))?;
            budget
                .ensure_receipts_fresh_on(&mut tx)
                .await
                .map_err(|_| std::io::Error::other("Artifact delivery receipt expired"))?;
            tx.commit()
                .await
                .map_err(|_| std::io::Error::other("Artifact delivery interrupted"))?;
            budget
                .ensure_receipts_fresh()
                .map_err(|_| std::io::Error::other("Artifact delivery receipt expired"))?;
            // No application wait follows these handoff-time deadline checks.
            authority
                .ensure_fresh()
                .map_err(|_| std::io::Error::other("Artifact delivery authority expired"))?;
            if chrono::Utc::now().timestamp() >= user.claims.exp {
                return Err(std::io::Error::other(
                    "Artifact delivery credential expired",
                ));
            }
            Ok(())
        }
    };
    // Hyper may suppress the body entirely for HEAD. Headers have a separate
    // checked handoff after response/blob preparation, never a body-poll check.
    authorize().await.map_err(|_| denied())?;
    let stream = guarded_chunks(body.into_data_stream(), authorize);
    Ok(axum::response::Response::from_parts(
        parts,
        axum::body::Body::from_stream(stream),
    ))
}

/// Fetch a chunk first, then check authority immediately before exposing it.
/// The check cannot be cached across chunks or run before a blocking source read.
/// The disclosure boundary is returning Ready(Some(frame)) to Hyper's Body
/// consumer. Hyper may subsequently queue it or wait on a socket. This adapter
/// owns no socket-readiness hook and cannot revoke frames already handed off.
pub(crate) fn guarded_chunks<S, E, F, A>(
    chunks: S,
    authorize: F,
) -> impl futures::Stream<Item = std::io::Result<axum::body::Bytes>>
where
    S: futures::Stream<Item = Result<axum::body::Bytes, E>>,
    F: FnMut() -> A,
    A: std::future::Future<Output = std::io::Result<()>>,
{
    use futures::TryStreamExt;
    futures::stream::try_unfold(
        (Box::pin(chunks), authorize),
        |(mut chunks, mut authorize)| async move {
            let Some(chunk) = chunks
                .try_next()
                .await
                .map_err(|_| std::io::Error::other("Artifact delivery interrupted"))?
            else {
                authorize().await?;
                return Ok(None);
            };
            authorize().await?;
            Ok(Some((chunk, (chunks, authorize))))
        },
    )
}

pub(crate) fn encrypted_manifest(version: &ArtifactVersion) -> Option<&str> {
    version
        .meta
        .as_ref()?
        .get("encrypted_log_manifest")?
        .as_str()
}

pub(crate) async fn sealed_log_response(
    state: &std::sync::Arc<crate::state::AppState>,
    version: &ArtifactVersion,
) -> ApiResult<axum::response::Response> {
    use attune_common::runtime_read_log::open_final_manifest;
    let key = state
        .config
        .security
        .encryption_key
        .as_deref()
        .ok_or_else(denied)?;
    let encrypted = encrypted_manifest(version).ok_or_else(denied)?;
    let execution = version.execution.ok_or_else(denied)?;
    let manifest =
        open_final_manifest(key, encrypted, execution, version.id).map_err(|_| denied())?;
    if version.body_state != Some(ArtifactBodyState::Ready)
        || version
            .meta
            .as_ref()
            .and_then(|meta| meta.get("log_encoding"))
            .and_then(serde_json::Value::as_str)
            != Some(attune_common::runtime_read_log::ENCODING)
        || version.size_bytes != i64::try_from(manifest.ciphertext_bytes).ok()
        || version.sha256.as_deref() != Some(manifest.ciphertext_sha256.as_str())
    {
        return Err(denied());
    }
    let mut connection = state.db.acquire().await?;
    if !ArtifactVersionRepository::log_is_sealed(&mut connection, version.id).await? {
        return Err(denied());
    }
    drop(connection);
    let reader = crate::log_decoder::sealed_ciphertext(state.clone(), version).await?;
    let stream = crate::log_decoder::verified_plaintext(reader, key.to_owned(), manifest);
    // A ciphertext range cannot establish complete-stream proof. Serve the full
    // plaintext with successful EOF gated on manifest verification, never an
    // early Content-Length/206 completion for an unchecked prefix.
    axum::response::Response::builder()
        .header(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )
        .header(axum::http::header::CACHE_CONTROL, "private, no-store")
        .header(axum::http::header::ACCEPT_RANGES, "none")
        .body(axum::body::Body::from_stream(stream))
        .map_err(|_| denied())
}

#[cfg(test)]
mod delivery_tests {
    use super::guarded_chunks;
    use futures::TryStreamExt;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[test]
    fn acquisition_stage_diagnostics_keep_public_denials_generic() {
        for stage in [
            super::ReadAcquisitionStage::BodyNotReady,
            super::ReadAcquisitionStage::LogBinding,
            super::ReadAcquisitionStage::CaptureRegistry,
        ] {
            let crate::middleware::ApiError::Forbidden(message) = super::read_stage_denied(stage)
            else {
                panic!("Wrong error class");
            };
            assert_eq!(message, "Selected artifact content is unavailable");
        }
    }

    #[tokio::test]
    async fn finite_plaintext_waiting_on_its_next_chunk_rechecks_revocation_before_exposure() {
        let active = Arc::new(AtomicBool::new(true));
        let release = Arc::new(tokio::sync::Notify::new());
        let chunks = futures::stream::unfold(0, {
            let release = release.clone();
            move |index| {
                let release = release.clone();
                async move {
                    if index == 2 {
                        return None;
                    }
                    if index == 1 {
                        release.notified().await;
                    }
                    Some((
                        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(if index == 0 {
                            b"first"
                        } else {
                            b"must not escape"
                        })),
                        index + 1,
                    ))
                }
            }
        });
        let guarded = guarded_chunks(chunks, {
            let active = active.clone();
            move || {
                futures::future::ready(if active.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err(std::io::Error::other("revoked"))
                })
            }
        });
        futures::pin_mut!(guarded);
        assert_eq!(
            guarded.try_next().await.unwrap().unwrap().as_ref(),
            b"first"
        );
        {
            let next = guarded.try_next();
            futures::pin_mut!(next);
            assert!(futures::poll!(next.as_mut()).is_pending());
            active.store(false, Ordering::SeqCst);
            release.notify_one();
            assert!(next.await.is_err());
        }
        assert!(guarded.try_next().await.unwrap().is_none());
    }
}
