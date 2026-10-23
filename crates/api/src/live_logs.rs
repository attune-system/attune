//! Typed live-log acquisition. Each emitted frame acquires an immutable current
//! origin-set revision after I/O; no mutable execution-wide union is a permit.
use crate::{
    auth::middleware::AuthenticatedUser,
    middleware::{ApiError, ApiResult},
    state::AppState,
};
use attune_common::{
    models::{
        enums::LogStreamBackend, ArtifactBodyState, ArtifactClassification, ArtifactVersion, Id,
    },
    rbac::Action,
    repositories::{
        artifact::ArtifactRepository,
        log_stream::LogStreamRepository,
        runtime_read::{
            RuntimeReadOrigin, RuntimeReadOwner, RuntimeReadRepository as Reads, RuntimeReadTarget,
        },
        runtime_target::RuntimeTarget,
        FindById,
    },
    runtime_read_log::ENCODING,
};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct LogPin {
    pub artifact: Id,
    pub version: Id,
    pub execution: Id,
    pub stream: Id,
    pub backend: LogStreamBackend,
    pub file_path: Option<String>,
    owner: Option<RuntimeReadOwner>,
}

fn denied() -> ApiError {
    ApiError::Forbidden("Live log source is unavailable".into())
}

impl LogPin {
    pub(crate) async fn select(
        state: &Arc<AppState>,
        user: &AuthenticatedUser,
        version: ArtifactVersion,
    ) -> ApiResult<Self> {
        let artifact = ArtifactRepository::find_by_id(&state.db, version.artifact)
            .await?
            .ok_or_else(denied)?;
        let stream = LogStreamRepository::find_by_artifact_version(&state.db, version.id)
            .await?
            .ok_or_else(denied)?;
        if artifact.classification != ArtifactClassification::RuntimeLog
            || version
                .meta
                .as_ref()
                .and_then(|meta| meta.get("log_encoding"))
                .and_then(serde_json::Value::as_str)
                != Some(ENCODING)
            || !matches!(
                version.body_state,
                Some(ArtifactBodyState::Pending | ArtifactBodyState::Ready)
            )
        {
            return Err(denied());
        }
        let mut pin = Self {
            artifact: artifact.id,
            version: version.id,
            execution: version.execution.ok_or_else(denied)?,
            stream: stream.id,
            backend: stream.backend,
            file_path: version.file_path,
            owner: None,
        };
        pin.owner = Some(pin.authorize_chunk(state, user).await?);
        Ok(pin)
    }

    pub(crate) async fn capture(
        &self,
        state: &Arc<AppState>,
        user: &AuthenticatedUser,
    ) -> ApiResult<RuntimeReadOwner> {
        if chrono::Utc::now().timestamp() >= user.claims.exp {
            return Err(denied());
        }
        let credential = user
            .claims
            .admitted_execution_credential()
            .map_err(|_| denied())?
            .ok_or_else(denied)?;
        let mut tx = state.db.begin().await?;
        attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(&mut tx)
            .await?;
        let authority = user
            .validate_recorded_runtime_authority(&mut tx)
            .await?
            .ok_or_else(denied)?;
        let rows = crate::recorded_consumers::execution_content_origins(
            &mut tx,
            self.execution,
            (authority.credential_binding().execution_id == self.execution).then_some(self.stream),
        )
        .await?;
        let mut prepared = crate::recorded_disclosure::OriginBudget::with_encryption_key(
            state.config.security.encryption_key.as_deref(),
        );
        prepared.prepare_receipts(&mut tx, &rows).await?;
        let source =
            Reads::lock_log_frontier(&mut tx, credential.admission_id, self.execution).await?;
        authority.ensure_fresh_on(&mut tx).await?;
        let (origins, budget) = crate::recorded_consumers::execution_content_frontier(
            &mut tx,
            &authority,
            self.execution,
            Some(self.stream),
            prepared,
        )
        .await?;
        let target = crate::recorded_disclosure::require_target(
            &mut tx,
            &authority,
            RuntimeTarget::Artifact(self.artifact),
            &[Action::Read, Action::Decrypt],
        )
        .await?;
        let owner = RuntimeReadOwner::of(&target);
        if self
            .owner
            .as_ref()
            .is_some_and(|expected| expected != &owner)
        {
            return Err(denied());
        }
        Reads::validate_log_target(
            &mut tx,
            self.artifact,
            self.version,
            self.execution,
            self.stream,
        )
        .await?;
        Reads::record(
            &mut tx,
            &authority,
            RuntimeReadOrigin {
                target: RuntimeReadTarget::LiveLog {
                    artifact: self.artifact,
                    version: self.version,
                    execution: self.execution,
                    stream: self.stream,
                    owner: owner.clone(),
                },
                provenance: crate::recorded_disclosure::snapshot(&origins)?,
                parents: budget.retained_parents(),
            },
        )
        .await?;
        if Reads::source_admission(&mut *tx, self.execution).await? != source {
            return Err(denied());
        }
        budget.ensure_receipts_fresh_on(&mut tx).await?;
        if chrono::Utc::now().timestamp() >= user.claims.exp {
            return Err(denied());
        }
        authority.ensure_fresh_on(&mut tx).await?;
        tx.commit().await?;
        budget.ensure_receipts_fresh()?;
        authority.ensure_fresh()?;
        if chrono::Utc::now().timestamp() >= user.claims.exp {
            return Err(denied());
        }
        Ok(owner)
    }

    async fn authorize_chunk(
        &self,
        state: &Arc<AppState>,
        user: &AuthenticatedUser,
    ) -> ApiResult<RuntimeReadOwner> {
        if crate::recorded_consumers::is_recorded(user)? {
            return self.capture(state, user).await;
        }
        crate::routes::artifacts::authorize_human_live_log(state, user, self, self.owner.as_ref())
            .await
    }

    pub(crate) async fn response(
        self,
        state: Arc<AppState>,
        user: AuthenticatedUser,
    ) -> ApiResult<axum::response::Response> {
        let reader = crate::log_decoder::live_ciphertext(state.clone(), self.clone());
        let key = state
            .config
            .security
            .encryption_key
            .clone()
            .ok_or_else(denied)?;
        let plaintext =
            crate::log_decoder::verified_live_plaintext(reader, key, self.execution, self.version);
        let plaintext = crate::log_decoder::text_boundaries(plaintext);
        let stream = crate::recorded_reads::guarded_chunks(plaintext, move || {
            let (state, user, pin) = (state.clone(), user.clone(), self.clone());
            async move {
                pin.authorize_chunk(&state, &user)
                    .await
                    .map(|_| ())
                    .map_err(|_| {
                        std::io::Error::other(
                            "Live log authority expired or its frontier is not permitted",
                        )
                    })
            }
        });
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
}
