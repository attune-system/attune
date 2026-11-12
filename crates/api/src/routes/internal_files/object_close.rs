//! Durable object close. Only immutable selected data crosses the storage phase.
use super::log_source::{HistoricalBinding, ProofBinding, Source};
use super::*;
use attune_common::db::SessionLease;
use attune_common::repositories::log_stream::SelectedLogClose;
use sqlx::Connection;

type Failure = (StatusCode, String);
fn conflict() -> Failure {
    (
        StatusCode::CONFLICT,
        "Selected log close changed or expired".into(),
    )
}

fn selection_error(error: attune_common::Error) -> Failure {
    match error {
        attune_common::Error::InvalidState(_) => conflict(),
        attune_common::Error::Database(sqlx::Error::Database(ref db))
            if db.code().as_deref() == Some("P0001") =>
        {
            conflict()
        }
        error => map_repository_error(error),
    }
}

struct Prepared {
    selected: SelectedLogClose,
    history: HistoricalBinding,
    stream: LogStream,
    segments: Vec<attune_common::models::log_stream::LogSegment>,
}
struct Verified {
    prepared: Prepared,
    bytes: attune_common::object_log_verification::VerifiedBytes,
}

async fn targets(
    source: &mut Source<'_>,
    user: &AuthenticatedUser,
    version: &attune_common::models::artifact_version::ArtifactVersion,
    stream: i64,
) -> Result<
    (
        attune_common::models::artifact_version::ArtifactVersion,
        LogStream,
    ),
    Failure,
> {
    source.authorize_target(user, version).await?;
    let version =
        ArtifactVersionRepository::find_by_id_for_update(source.transaction(), version.id)
            .await
            .map_err(map_repository_error)?
            .ok_or_else(conflict)?;
    let stream = LogStreamRepository::lock(source.transaction(), stream)
        .await
        .map_err(map_repository_error)?;
    source.recheck().await?;
    Ok((version, stream))
}

async fn prepare(
    state: &AppState,
    user: &AuthenticatedUser,
    version: &attune_common::models::artifact_version::ArtifactVersion,
    stream_id: i64,
    headers: &HeaderMap,
    truncated: bool,
) -> Result<Prepared, Failure> {
    // Retain cancellation-safe ownership, but reuse acknowledged clean phases.
    let mut connection = SessionLease::acquire(&state.db)
        .await
        .map_err(map_sqlx_error)?;
    let mut tx = connection.begin().await.map_err(map_sqlx_error)?;
    if let Err(error) = LogStreamRepository::configure_close_phase(&mut tx).await {
        tx.rollback().await.map_err(map_sqlx_error)?;
        connection.release_clean();
        return Err(map_repository_error(error));
    }
    let mut source = log_source::acquire(state, tx, user, version).await?;
    let result = async {
        let (version, stream) = targets(&mut source, user, version, stream_id).await?;
        let history = source
            .bind(state, user, &version, &stream, headers, None)
            .await?;
        let revision = LogStreamRepository::close_revision(source.transaction(), stream.id)
            .await
            .map_err(map_repository_error)?;
        let selected = LogStreamRepository::select_close(
            source.transaction(),
            stream.id,
            revision,
            &serde_json::to_value(history.producer()).map_err(|_| conflict())?,
            &serde_json::to_value(history.proof()).map_err(|_| conflict())?,
            truncated,
            chrono::Utc::now() + chrono::Duration::minutes(5),
        )
        .await
        .map_err(selection_error)?;
        if selected.published()
            && (!stream.sealed
                || stream.truncated != truncated
                || version.body_state != Some(ArtifactBodyState::Ready)
                || version.size_bytes != Some(stream.total_bytes)
                || version.sha256.is_none())
        {
            return Err(conflict());
        }
        let segments = LogStreamRepository::segments_with_conn(source.transaction(), stream.id)
            .await
            .map_err(map_repository_error)?;
        source.recheck().await?;
        Ok(Prepared {
            selected,
            history,
            stream,
            segments,
        })
    }
    .await;
    match result {
        Ok(prepared) => {
            source.commit().await?;
            connection.release_clean();
            if !prepared.selected.published() && prepared.selected.deadline() <= chrono::Utc::now()
            {
                return Err(conflict());
            }
            Ok(prepared)
        }
        Err(error) => {
            source.rollback().await?;
            connection.release_clean();
            Err(error)
        }
    }
}

// No AppState, source, pool, transaction, or mutation proof can enter this phase.
async fn verify(
    storage: &dyn attune_common::blob_store::BlobStore,
    prepared: Prepared,
    key: Option<&str>,
) -> Result<Verified, Failure> {
    let encrypted = match prepared.history.proof() {
        ProofBinding::Plain => None,
        ProofBinding::Encrypted {
            manifest,
            plaintext_cap,
            ciphertext_cap,
            ..
        } => Some(attune_common::object_log_verification::EncryptedInput {
            key: key.ok_or_else(conflict)?,
            manifest,
            plaintext_cap: *plaintext_cap,
            ciphertext_cap: *ciphertext_cap,
        }),
    };
    let bytes = attune_common::object_log_verification::verify(
        storage,
        &prepared.stream,
        &prepared.segments,
        encrypted,
    )
    .await
    .map_err(|error| match error {
        attune_common::object_log_verification::VerificationError::Storage(error) => {
            map_blob_error(error)
        }
        attune_common::object_log_verification::VerificationError::Encrypted(error) => {
            map_transport_error(error)
        }
        error @ attune_common::object_log_verification::VerificationError::Manifest => {
            (StatusCode::FORBIDDEN, error.to_string())
        }
        error => (StatusCode::CONFLICT, error.to_string()),
    })?;
    Ok(Verified { prepared, bytes })
}

async fn finish(
    state: &AppState,
    user: &AuthenticatedUser,
    initial: &attune_common::models::artifact_version::ArtifactVersion,
    headers: &HeaderMap,
    verified: &Verified,
) -> Result<(), Failure> {
    // Retain cancellation-safe ownership, but reuse acknowledged clean phases.
    let mut connection = SessionLease::acquire(&state.db)
        .await
        .map_err(map_sqlx_error)?;
    let mut tx = connection.begin().await.map_err(map_sqlx_error)?;
    if let Err(error) = LogStreamRepository::configure_close_phase(&mut tx).await {
        tx.rollback().await.map_err(map_sqlx_error)?;
        connection.release_clean();
        return Err(map_repository_error(error));
    }
    let mut source = log_source::acquire(state, tx, user, initial).await?;
    let result = async {
        let p = &verified.prepared;
        let (version, stream) = targets(&mut source, user, initial, p.stream.id).await?;
        source
            .bind(state, user, &version, &stream, headers, Some(&p.history))
            .await?;
        let selected = LogStreamRepository::select_close(
            source.transaction(),
            stream.id,
            p.selected.revision(),
            &serde_json::to_value(p.history.producer()).map_err(|_| conflict())?,
            &serde_json::to_value(p.history.proof()).map_err(|_| conflict())?,
            p.selected.input()["truncated"]
                .as_bool()
                .ok_or_else(conflict)?,
            // Finalization never takes over an expired revision.
            chrono::Utc::now() - chrono::Duration::seconds(1),
        )
        .await
        .map_err(selection_error)?;
        if selected.revision() != p.selected.revision()
            || selected.input() != p.selected.input()
            || selected.input_sha256() != p.selected.input_sha256()
        {
            return Err(conflict());
        }
        if selected.published() {
            if version.body_state != Some(ArtifactBodyState::Ready)
                || version.size_bytes != Some(verified.bytes.size())
                || version.sha256.as_deref() != Some(verified.bytes.sha256())
            {
                return Err(conflict());
            }
        } else {
            source
                .publish_evidence(
                    state,
                    user,
                    &version,
                    &stream,
                    headers,
                    p.selected.input()["truncated"]
                        .as_bool()
                        .ok_or_else(conflict)?,
                )
                .await?;
            LogStreamRepository::publish_close(source.transaction(), stream.id, &selected)
                .await
                .map_err(selection_error)?;
            ArtifactVersionRepository::mark_body_ready_in_transaction(
                source.transaction(),
                version.id,
                &format!("segments:{}", stream.next_sequence),
                verified.bytes.size(),
                verified.bytes.sha256(),
            )
            .await
            .map_err(map_repository_error)?
            .ok_or_else(conflict)?;
        }
        source.recheck().await?;
        if !selected.published() && p.selected.deadline() <= chrono::Utc::now() {
            return Err(conflict());
        }
        Ok(selected.published())
    }
    .await;
    match result {
        Ok(already_published) => {
            source.commit().await?;
            connection.release_clean();
            // Acknowledged commit followed by pure deadline failure is never
            // relabeled CommitUnknown or blindly retried.
            if !already_published && verified.prepared.selected.deadline() <= chrono::Utc::now() {
                return Err(conflict());
            }
            Ok(())
        }
        Err(error) => {
            source.rollback().await?;
            connection.release_clean();
            Err(error)
        }
    }
}

async fn bounded<T>(
    work: impl std::future::Future<Output = Result<T, Failure>>,
) -> Result<T, Failure> {
    tokio::time::timeout(std::time::Duration::from_secs(15), work)
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Log close phase timed out; retry same input".into(),
            )
        })?
}

pub(super) async fn seal(
    state: Arc<AppState>,
    user: AuthenticatedUser,
    version: attune_common::models::artifact_version::ArtifactVersion,
    stream: i64,
    headers: HeaderMap,
    truncated: bool,
) -> Result<StatusCode, Failure> {
    // The owned task finishes transaction cleanup even if the HTTP waiter leaves.
    tokio::spawn(async move {
        let prepared = match bounded(prepare(
            &state, &user, &version, stream, &headers, truncated,
        ))
        .await
        {
            Err((StatusCode::SERVICE_UNAVAILABLE, _)) => {
                bounded(prepare(
                    &state, &user, &version, stream, &headers, truncated,
                ))
                .await?
            }
            result => result?,
        };
        // This phase freshly authorized and matched the exact durable input.
        // A published retry uses the earlier verified publication, without GET.
        if prepared.selected.published() {
            return Ok(StatusCode::OK);
        }
        let verified = verify(
            state.blob_store.as_ref(),
            prepared,
            state.config.security.encryption_key.as_deref(),
        )
        .await?;
        match bounded(finish(&state, &user, &version, &headers, &verified)).await {
            Err((StatusCode::SERVICE_UNAVAILABLE, _)) => {
                bounded(finish(&state, &user, &version, &headers, &verified)).await?
            }
            result => result?,
        };
        Ok(StatusCode::OK)
    })
    .await
    .map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Log close task unavailable; retry with the same input".into(),
        )
    })?
}
