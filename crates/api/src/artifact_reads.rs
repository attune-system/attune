//! Request-local artifact visibility and recorded runtime-log disclosure.
//! Collection reads apply visibility in SQL. Global summaries deliberately do
//! not apply the detail/execution-list disclosure gate.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use attune_common::{
    models::{artifact::Artifact, ArtifactClassification},
    repositories::{
        artifact::{
            ArtifactReadContext, ArtifactRepository, ArtifactSearchFilters, ArtifactSearchResult,
            ArtifactSummaryRow, ArtifactVersionRepository, ArtifactVersionSummaryRow,
            ARTIFACT_READ_BATCH_SIZE,
        },
        execution::ExecutionRepository,
        execution_secret_value::{ExecutionSecretValueRepository, SecretProvenanceRow},
        schema_contract::{BindingOwner, SchemaContractRepository},
    },
    secret_values::{ENTITY_EXECUTION_CONFIG, ENTITY_EXECUTION_RESULT},
};

use crate::{
    auth::middleware::AuthenticatedUser,
    authz::AuthorizationSnapshot,
    disclosure_records::DisclosureRecords,
    middleware::{ApiError, ApiResult},
    secret_disclosure::{authorize_runtime_log_cached, runtime_log_requires_disclosure},
    state::AppState,
};

// Schema bundles may include static workflow snapshots. Chunk these separately
// from lightweight artifact summaries so one read does not retain 256 bundles.
const EXECUTION_DISCLOSURE_BATCH_SIZE: usize = 64;

pub(crate) struct ArtifactReads<'a> {
    state: &'a Arc<AppState>,
    user: &'a AuthenticatedUser,
    context: Option<ArtifactReadContext>,
    disclosure_snapshot: Option<AuthorizationSnapshot>,
    execution_disclosure: HashMap<i64, bool>,
    source_records: DisclosureRecords,
    receipt_guard: Option<crate::receipt_disclosure::SharedHumanReceiptGuard>,
}

impl<'a> ArtifactReads<'a> {
    pub(crate) async fn new(
        state: &'a Arc<AppState>,
        user: &'a AuthenticatedUser,
    ) -> ApiResult<Self> {
        Ok(Self {
            state,
            user,
            context: crate::routes::artifacts::artifact_read_context_for_user(state, user).await?,
            disclosure_snapshot: None,
            execution_disclosure: HashMap::new(),
            source_records: DisclosureRecords::default(),
            receipt_guard: None,
        })
    }

    pub(crate) async fn global_summaries(
        &self,
        filters: &ArtifactSearchFilters,
    ) -> ApiResult<ArtifactSearchResult<ArtifactSummaryRow>> {
        if crate::recorded_consumers::is_recorded(self.user)? {
            return crate::recorded_consumers::artifact_summaries(
                &self.state.db,
                self.user,
                filters,
            )
            .await;
        }
        Ok(
            ArtifactRepository::search_summaries(&self.state.db, filters, self.context.as_ref())
                .await?,
        )
    }

    pub(crate) async fn authorize_detail(&mut self, artifact: &Artifact) -> ApiResult<()> {
        self.authorize_header(artifact.id, artifact.classification)
            .await
    }

    /// Consume this request-local cache so a later transport wait retains every
    /// receipt deadline, rather than reusing a cached bool without its fence.
    pub(crate) async fn authorize_detail_with_receipts(
        self,
        artifact: &Artifact,
    ) -> ApiResult<crate::receipt_disclosure::SharedHumanReceiptGuard> {
        let executions =
            ArtifactRepository::linked_execution_ids_bounded(&self.state.db, artifact.id, 257)
                .await?;
        let guard = crate::receipt_disclosure::HumanReceiptGuard::for_executions(
            self.state,
            self.user,
            &executions,
        )
        .await?
        .shared();
        self.authorize_detail_with_prepared_receipts(artifact, guard)
            .await
    }

    pub(crate) async fn authorize_detail_with_prepared_receipts(
        mut self,
        artifact: &Artifact,
        guard: crate::receipt_disclosure::SharedHumanReceiptGuard,
    ) -> ApiResult<crate::receipt_disclosure::SharedHumanReceiptGuard> {
        self.disclosure_snapshot = guard.lock().await.iam_snapshot();
        self.receipt_guard = Some(guard.clone());
        self.authorize_header(artifact.id, artifact.classification)
            .await?;
        let freshness = self.source_records.take_receipt_freshness();
        freshness.ensure_fresh()?;
        Ok(guard)
    }

    pub(crate) async fn version_summaries(
        &mut self,
        artifact: &ArtifactSummaryRow,
        pagination: &crate::dto::common::PaginationParams,
    ) -> ApiResult<ArtifactSearchResult<ArtifactVersionSummaryRow>> {
        self.authorize_header(artifact.id, artifact.classification)
            .await?;
        Ok(ArtifactVersionRepository::page_summaries(
            &self.state.db,
            artifact.id,
            self.context.as_ref(),
            pagination.limit(),
            pagination.offset(),
        )
        .await?)
    }

    async fn authorize_header(
        &mut self,
        id: i64,
        classification: ArtifactClassification,
    ) -> ApiResult<()> {
        if crate::recorded_consumers::is_recorded(self.user)? {
            return crate::recorded_consumers::artifact_read(&self.state.db, self.user, id).await;
        }
        let Some(context) = self.context.as_ref() else {
            return Ok(());
        };
        if !ArtifactRepository::is_readable(&self.state.db, id, context).await? {
            return Err(ApiError::Forbidden("Artifact is not readable".into()));
        }
        if classification == ArtifactClassification::RuntimeLog
            && !self.readable_logs(&[id]).await?.contains(&id)
        {
            return Err(ApiError::Forbidden(
                "Runtime log disclosure is not authorized".into(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn execution_summaries(
        &mut self,
        filters: &ArtifactSearchFilters,
    ) -> ApiResult<ArtifactSearchResult<ArtifactSummaryRow>> {
        if crate::recorded_consumers::is_recorded(self.user)? {
            return crate::recorded_consumers::artifact_summaries(
                &self.state.db,
                self.user,
                filters,
            )
            .await;
        }
        let start = std::time::Instant::now();
        let mut candidates = 0usize;
        let mut after = None;
        let mut summaries = Vec::new();
        let mut total = 0i64;
        loop {
            let rows = ArtifactRepository::scan_summaries(
                &self.state.db,
                filters,
                self.context.as_ref(),
                after,
            )
            .await?;
            let count = rows.len();
            candidates += count;
            let Some(last) = rows.last() else {
                break;
            };
            after = Some((last.created, last.id));
            let log_ids = rows
                .iter()
                .filter(|row| row.classification == ArtifactClassification::RuntimeLog)
                .map(|row| row.id)
                .collect::<Vec<_>>();
            let readable_logs = self.readable_logs(&log_ids).await?;
            for row in rows.into_iter().filter(|row| {
                row.classification != ArtifactClassification::RuntimeLog
                    || readable_logs.contains(&row.id)
            }) {
                if total >= i64::from(filters.offset) && summaries.len() < filters.limit as usize {
                    summaries.push(row);
                }
                total += 1;
            }
            if count < ARTIFACT_READ_BATCH_SIZE {
                break;
            }
        }
        tracing::debug!(
            operation = "execution_artifact_eligible_scan",
            candidate_rows = candidates,
            eligible_rows = total,
            returned_rows = summaries.len(),
            elapsed_seconds = start.elapsed().as_secs_f64(),
            "Exact-total scan and eligible page selection completed"
        );
        Ok(ArtifactSearchResult {
            rows: summaries,
            total,
        })
    }

    async fn readable_logs(&mut self, artifact_ids: &[i64]) -> ApiResult<HashSet<i64>> {
        if self.context.is_none() {
            return Ok(artifact_ids.iter().copied().collect());
        }
        if artifact_ids.is_empty() {
            return Ok(HashSet::new());
        }
        let mut links = Vec::new();
        for batch in artifact_ids.chunks(ARTIFACT_READ_BATCH_SIZE) {
            links.extend(
                ArtifactRepository::linked_execution_ids_many(&self.state.db, batch).await?,
            );
        }
        let mut execution_ids = links
            .iter()
            .map(|(_, execution)| *execution)
            .filter(|id| !self.execution_disclosure.contains_key(id))
            .collect::<Vec<_>>();
        execution_ids.sort_unstable();
        execution_ids.dedup();
        for batch in execution_ids.chunks(EXECUTION_DISCLOSURE_BATCH_SIZE) {
            self.load_execution_disclosure(batch).await?;
        }
        let mut readable = artifact_ids.iter().copied().collect::<HashSet<_>>();
        for (artifact, execution) in links {
            if self.execution_disclosure.get(&execution) != Some(&true) {
                readable.remove(&artifact);
            }
        }
        Ok(readable)
    }

    #[tracing::instrument(name = "artifact_execution_disclosure", level = "debug", skip_all)]
    async fn load_execution_disclosure(&mut self, ids: &[i64]) -> ApiResult<()> {
        let executions = ExecutionRepository::find_authorization_rows(&self.state.db, ids)
            .await?
            .into_iter()
            .map(|row| (row.id, row))
            .collect::<HashMap<_, _>>();
        let bindings =
            SchemaContractRepository::find_many(&self.state.db, BindingOwner::Execution, ids)
                .await?
                .into_iter()
                .collect::<HashMap<_, _>>();
        let mut secrets = HashMap::<i64, Vec<SecretProvenanceRow>>::new();
        for row in ExecutionSecretValueRepository::find_provenance_by_entities(
            &self.state.db,
            &[
                ENTITY_EXECUTION_CONFIG,
                ENTITY_EXECUTION_RESULT,
                "workflow_variables",
            ],
            ids,
        )
        .await?
        {
            secrets.entry(row.entity_id).or_default().push(row);
        }
        for row in
            crate::secret_disclosure::runtime_key_read_provenance(&self.state.db, ids).await?
        {
            secrets.entry(row.entity_id).or_default().push(row);
        }
        self.source_records
            .seed_executions(executions.values().cloned());
        self.source_records
            .seed_execution_provenance(ids, secrets.values().flatten().cloned());
        let mut sensitive = HashSet::new();
        for id in ids {
            if let (Some(_), Some(binding)) = (executions.get(id), bindings.get(id)) {
                if runtime_log_requires_disclosure(
                    binding,
                    secrets.get(id).map(Vec::as_slice).unwrap_or(&[]),
                )? {
                    sensitive.insert(*id);
                }
            }
        }
        self.source_records
            .load_executions(
                &self.state.db,
                &sensitive.iter().copied().collect::<Vec<_>>(),
            )
            .await?;
        if !sensitive.is_empty() && self.disclosure_snapshot.is_none() {
            self.disclosure_snapshot = self
                .state
                .authorization_service()
                .load_snapshot(self.user)
                .await?;
        }
        self.source_records
            .prepare(
                &self.state.db,
                self.user,
                self.disclosure_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.grants.as_slice())
                    .unwrap_or(&[]),
                secrets
                    .iter()
                    .filter(|(id, _)| sensitive.contains(id))
                    .flat_map(|(_, rows)| rows)
                    .map(|row| (row.source_kind.as_str(), row.source_ref.as_deref())),
            )
            .await?;
        for id in ids {
            let allowed = match (executions.get(id), bindings.get(id)) {
                (Some(execution), Some(_)) => {
                    let secrets = secrets.get(id).map(Vec::as_slice).unwrap_or(&[]);
                    if !sensitive.contains(id) {
                        true
                    } else {
                        if self.disclosure_snapshot.is_none() {
                            self.disclosure_snapshot = self
                                .state
                                .authorization_service()
                                .load_snapshot(self.user)
                                .await?;
                        }
                        let snapshot = self.disclosure_snapshot.as_ref().ok_or_else(|| {
                            ApiError::Forbidden(
                                "Runtime log secret disclosure requires identity access".into(),
                            )
                        })?;
                        let result = if let Some(guard) = &self.receipt_guard {
                            crate::secret_disclosure::authorize_runtime_log_cached_guarded(
                                self.state,
                                self.user,
                                execution,
                                secrets,
                                snapshot,
                                &mut self.source_records,
                                guard.clone(),
                            )
                            .await
                        } else {
                            authorize_runtime_log_cached(
                                self.state,
                                self.user,
                                execution,
                                secrets,
                                snapshot,
                                &mut self.source_records,
                            )
                            .await
                        };
                        match result {
                            Ok(()) => true,
                            Err(ApiError::Forbidden(_) | ApiError::NotFound(_)) => false,
                            Err(error) => return Err(error),
                        }
                    }
                }
                _ => false,
            };
            self.execution_disclosure.insert(*id, allowed);
        }
        Ok(())
    }
}

/// Detail denial masking must not hide database or internal failures.
pub(crate) fn mask_detail_denial(error: ApiError, message: String) -> ApiError {
    match error {
        ApiError::Forbidden(_) | ApiError::NotFound(_) => ApiError::NotFound(message),
        error => error,
    }
}
