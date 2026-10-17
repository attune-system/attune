//! Receipt source preparation for human transports. This is not an IAM permit:
//! G's current source/target matcher must still authorize every disclosed origin.
use crate::{
    auth::middleware::AuthenticatedUser,
    middleware::{ApiError, ApiResult},
    state::AppState,
};
use attune_common::{
    auth::jwt::TokenType,
    repositories::{
        execution_secret_value::SecretProvenanceRow, immutable_input_origin::InputReceiptFrontier,
        schema_type::SchemaTypeRepository,
    },
};
use std::{collections::BTreeSet, sync::Arc};

pub(crate) type SharedHumanReceiptGuard = Arc<tokio::sync::Mutex<HumanReceiptGuard>>;
mod iam;

pub(crate) struct HumanReceiptGuard {
    transaction: sqlx::Transaction<'static, sqlx::Postgres>,
    frontier: InputReceiptFrontier,
    sources_closed: bool,
    sensor_versions: BTreeSet<i64>,
    iam: Option<iam::HeldIam>,
}

impl HumanReceiptGuard {
    async fn begin(state: &AppState, user: &AuthenticatedUser) -> ApiResult<Self> {
        let mut transaction = state.db.begin().await?;
        attune_common::repositories::file_evidence_retention::FileEvidenceRetentionRepository::lock_capture(
            &mut transaction,
        ).await?;
        SchemaTypeRepository::lock_selection(&mut transaction).await?;
        let iam = if user.claims.token_type == TokenType::Access {
            Some(
                iam::HeldIam::retain(
                    &mut transaction,
                    user.identity_id()
                        .map_err(|_| ApiError::Unauthorized("Identity unavailable".into()))?,
                )
                .await?,
            )
        } else {
            None
        };
        Ok(Self {
            transaction,
            frontier: InputReceiptFrontier::default(),
            sources_closed: false,
            sensor_versions: BTreeSet::new(),
            iam,
        })
    }

    pub(crate) async fn for_rows(
        state: &AppState,
        user: &AuthenticatedUser,
        rows: &[SecretProvenanceRow],
    ) -> ApiResult<Self> {
        let mut guard = Self::begin(state, user).await?;
        guard
            .frontier
            .prepare(
                &mut guard.transaction,
                rows,
                state.config.security.encryption_key.as_deref(),
            )
            .await?;
        Ok(guard)
    }

    pub(crate) async fn for_executions(
        state: &AppState,
        user: &AuthenticatedUser,
        executions: &[i64],
    ) -> ApiResult<Self> {
        let mut executions = executions.to_vec();
        executions.sort_unstable();
        executions.dedup();
        if executions.len() > 256 {
            return Err(ApiError::Forbidden(
                "Receipt graph exceeds its bound".into(),
            ));
        }
        let mut guard = Self::begin(state, user).await?;
        let mut rows = Vec::new();
        for execution in executions {
            rows.extend(
                crate::recorded_disclosure::execution_origins(&mut guard.transaction, execution)
                    .await?,
            );
            if rows.len() > 256 {
                return Err(ApiError::Forbidden(
                    "Receipt graph exceeds its bound".into(),
                ));
            }
        }
        guard
            .frontier
            .prepare(
                &mut guard.transaction,
                &rows,
                state.config.security.encryption_key.as_deref(),
            )
            .await?;
        Ok(guard)
    }

    pub(crate) fn frontier(&self) -> &InputReceiptFrontier {
        &self.frontier
    }
    pub(crate) fn matches_iam(
        &self,
        caller: &attune_common::rbac::AuthorizationContext,
        grants: &[attune_common::rbac::Grant],
    ) -> bool {
        self.iam
            .as_ref()
            .is_none_or(|iam| iam.matches(caller, grants))
    }

    /// Append selected sensor versions while collecting bytes, before ANY
    /// target checks. All source locks belong to this same transaction.
    pub(crate) async fn prepare_sensor_logs(
        &mut self,
        state: &AppState,
        versions: &[i64],
    ) -> ApiResult<()> {
        use attune_common::repositories::{
            artifact::{ArtifactRepository, ArtifactVersionRepository},
            execution_secret_value::ExecutionSecretValueRepository,
            sensor_schema::{
                SensorSchemaRepository, ENTITY_SENSOR_GENERATION_CONFIG,
                ENTITY_SENSOR_GENERATION_RULE_CONFIG, ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
            },
        };
        if self.sources_closed {
            return Err(ApiError::Forbidden(
                "Receipt sources must precede target locks".into(),
            ));
        }
        let mut rows = Vec::new();
        let mut executions = BTreeSet::new();
        for version in versions {
            if !self.sensor_versions.insert(*version) {
                continue;
            }
            if self.sensor_versions.len() > 256 {
                return Err(ApiError::Forbidden(
                    "Sensor receipt graph exceeds its bound".into(),
                ));
            }
            let (generation, _) =
                SensorSchemaRepository::find_log_generation(&mut *self.transaction, *version)
                    .await?
                    .ok_or_else(|| {
                        ApiError::Forbidden("Sensor log generation unavailable".into())
                    })?;
            rows.extend(
                ExecutionSecretValueRepository::find_provenance_by_entities(
                    &mut *self.transaction,
                    &[
                        ENTITY_SENSOR_GENERATION_CONFIG,
                        ENTITY_SENSOR_GENERATION_RULE_CONFIG,
                    ],
                    &[generation],
                )
                .await?,
            );
            let deliveries =
                SensorSchemaRepository::log_delivery_origins(&mut *self.transaction, *version)
                    .await?;
            if deliveries.len() > 256 {
                return Err(ApiError::Forbidden(
                    "Sensor delivery graph exceeds its bound".into(),
                ));
            }
            for batch in deliveries.chunks(64) {
                rows.extend(
                    ExecutionSecretValueRepository::find_provenance_by_entities(
                        &mut *self.transaction,
                        &[ENTITY_SENSOR_RULE_DELIVERY_CONFIG],
                        batch,
                    )
                    .await?,
                );
            }
            let selected = ArtifactVersionRepository::find_by_id(&mut *self.transaction, *version)
                .await?
                .ok_or_else(|| ApiError::Forbidden("Sensor log version unavailable".into()))?;
            executions.extend(
                ArtifactRepository::linked_execution_ids_bounded(
                    &mut *self.transaction,
                    selected.artifact,
                    257,
                )
                .await?,
            );
            if rows.len() > 256 || executions.len() > 256 {
                return Err(ApiError::Forbidden(
                    "Sensor receipt graph exceeds its bound".into(),
                ));
            }
        }
        for execution in executions {
            rows.extend(
                crate::recorded_disclosure::execution_origins(&mut self.transaction, execution)
                    .await?,
            );
            if rows.len() > 256 {
                return Err(ApiError::Forbidden(
                    "Sensor receipt graph exceeds its bound".into(),
                ));
            }
        }
        self.frontier
            .prepare(
                &mut self.transaction,
                &rows,
                state.config.security.encryption_key.as_deref(),
            )
            .await?;
        Ok(())
    }

    pub(crate) fn close_sources(&mut self) {
        self.sources_closed = true;
    }

    pub(crate) fn connection(&mut self) -> &mut sqlx::PgConnection {
        self.close_sources();
        &mut self.transaction
    }

    pub(crate) fn shared(self) -> SharedHumanReceiptGuard {
        Arc::new(tokio::sync::Mutex::new(self))
    }

    pub(crate) async fn finish_shared(
        guard: SharedHumanReceiptGuard,
    ) -> ApiResult<InputReceiptFrontier> {
        let guard = Arc::try_unwrap(guard)
            .map_err(|_| ApiError::Forbidden("Receipt guard is still in use".into()))?
            .into_inner();
        guard.finish().await
    }

    /// The caller performs all other application waits first. Source locks and
    /// current Key targets survive until this final commit; only synchronous
    /// deadline/JWT checks may follow it before returning a protected frame.
    pub(crate) async fn finish(self) -> ApiResult<InputReceiptFrontier> {
        let Self {
            mut transaction,
            frontier,
            ..
        } = self;
        frontier.ensure_fresh_on(&mut transaction).await?;
        transaction.commit().await?;
        frontier.ensure_fresh()?;
        Ok(frontier)
    }
}
