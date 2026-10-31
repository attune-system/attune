//! Request-local artifact visibility and recorded runtime-log disclosure.
//! Collection reads apply visibility in SQL. Global summaries deliberately do
//! not apply the detail/execution-list disclosure gate.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use attune_common::{
    models::{artifact::Artifact, ArtifactClassification},
    repositories::{
        artifact::{
            ArtifactMetadataRow, ArtifactReadContext, ArtifactRepository, ArtifactSearchFilters,
            ArtifactVersionRepository, ARTIFACT_READ_BATCH_SIZE,
        },
        execution::ExecutionRepository,
        execution_secret_value::{ExecutionSecretValueRepository, SecretProvenanceRow},
        schema_contract::{BindingOwner, SchemaContractRepository},
    },
    secret_values::{ENTITY_EXECUTION_CONFIG, ENTITY_EXECUTION_RESULT},
};

use crate::{
    artifact_cursor::{Cursor, Position},
    auth::middleware::AuthenticatedUser,
    authz::AuthorizationSnapshot,
    disclosure_records::DisclosureRecords,
    dto::artifact::{ArtifactCursorPage, ArtifactSummary, ArtifactVersionSummary},
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
        Ok(Self::with_context(
            state,
            user,
            crate::routes::artifacts::artifact_read_context_for_user(state, user).await?,
        ))
    }

    fn with_context(
        state: &'a Arc<AppState>,
        user: &'a AuthenticatedUser,
        context: Option<ArtifactReadContext>,
    ) -> Self {
        Self {
            state,
            user,
            context,
            disclosure_snapshot: None,
            execution_disclosure: HashMap::new(),
            source_records: DisclosureRecords::default(),
            receipt_guard: None,
        }
    }

    pub(crate) async fn global_summaries(
        &self,
        filters: &ArtifactSearchFilters,
        raw_cursor: Option<&str>,
    ) -> ApiResult<ArtifactCursorPage<ArtifactSummary>> {
        let cursor = Cursor::artifacts(
            raw_cursor,
            "global_artifacts",
            filters,
            Some(self.state.jwt_config.secret.as_str()),
        )?;
        if crate::recorded_consumers::is_recorded(self.user)? {
            return crate::recorded_consumers::artifact_summaries(
                &self.state.db,
                self.user,
                filters,
                &cursor,
                false,
            )
            .await;
        }
        let mut candidates = filters.clone();
        candidates.limit = filters.limit + 1;
        let rows = ArtifactRepository::scan_summaries(
            &self.state.db,
            &candidates,
            self.context.as_ref(),
            cursor.artifact_after(),
        )
        .await?;
        cursor.page(
            rows.into_iter().map(ArtifactSummary::from).collect(),
            filters.limit,
            |row| Position::Artifact {
                created: row.created,
                id: row.id,
            },
        )
    }

    pub(crate) async fn authorize_detail(
        state: &'a Arc<AppState>,
        user: &'a AuthenticatedUser,
        artifact: &Artifact,
    ) -> ApiResult<()> {
        let guard = Self::authorize_detail_with_receipts(state, user, artifact).await?;
        let freshness = crate::receipt_disclosure::HumanReceiptGuard::finish_shared(guard).await?;
        freshness.ensure_fresh()?;
        if user.claims.exp <= chrono::Utc::now().timestamp() {
            return Err(ApiError::Unauthorized("Artifact credential expired".into()));
        }
        Ok(())
    }

    /// Consume this request-local cache so a later transport wait retains every
    /// receipt deadline, rather than reusing a cached bool without its fence.
    pub(crate) async fn authorize_detail_with_receipts(
        state: &'a Arc<AppState>,
        user: &'a AuthenticatedUser,
        artifact: &Artifact,
    ) -> ApiResult<crate::receipt_disclosure::SharedHumanReceiptGuard> {
        let executions =
            ArtifactRepository::linked_execution_ids_bounded(&state.db, artifact.id, 257).await?;
        let guard =
            crate::receipt_disclosure::HumanReceiptGuard::for_executions(state, user, &executions)
                .await?
                .shared();
        Self::authorize_detail_with_prepared_receipts(state, user, artifact, guard).await
    }

    pub(crate) async fn authorize_detail_with_prepared_receipts(
        state: &'a Arc<AppState>,
        user: &'a AuthenticatedUser,
        artifact: &Artifact,
        guard: crate::receipt_disclosure::SharedHumanReceiptGuard,
    ) -> ApiResult<crate::receipt_disclosure::SharedHumanReceiptGuard> {
        // No pool-backed constructor may run while the prepared guard owns its
        // transaction. Derive identity-based context from its retained caps.
        let mut reads = Self::with_context(state, user, None);
        let (snapshot, visibility) = {
            let held = guard.lock().await;
            (held.credential_snapshot(), held.visibility_snapshot())
        };
        if let Some(snapshot) = snapshot {
            let visibility = visibility.ok_or_else(|| {
                ApiError::Forbidden("Retained credential visibility is unavailable".into())
            })?;
            reads.context = Some(ArtifactReadContext {
                identity_id: visibility.identity_id,
                identity_attributes: visibility.identity_attributes,
                grants: visibility.grants,
            });
            let mut caller = attune_common::rbac::AuthorizationContext::new(snapshot.identity_id);
            caller.identity_attributes = snapshot.identity_attributes.clone();
            reads.source_records.sensor_caller = Some(caller);
            reads.disclosure_snapshot = Some(snapshot);
        }
        reads.receipt_guard = Some(guard.clone());
        reads
            .authorize_header(artifact.id, artifact.classification)
            .await?;
        let freshness = reads.source_records.take_receipt_freshness();
        freshness.ensure_fresh()?;
        Ok(guard)
    }

    /// Metadata visibility is not permission to disclose a runtime-log body.
    /// Recorded credentials keep their typed consumer gate even on this path.
    pub(crate) async fn authorize_metadata_header(
        &mut self,
        id: i64,
        classification: ArtifactClassification,
    ) -> ApiResult<()> {
        if crate::recorded_consumers::is_recorded(self.user)? {
            return crate::recorded_consumers::artifact_read(&self.state.db, self.user, id).await;
        }
        if classification != ArtifactClassification::RuntimeLog
            || self.user.claims.token_type != attune_common::auth::jwt::TokenType::Access
        {
            return self.authorize_header(id, classification).await;
        }
        if let Some(context) = &self.context {
            if !ArtifactRepository::is_readable(&self.state.db, id, context).await? {
                return Err(ApiError::Forbidden("Artifact is not readable".into()));
            }
        }
        Ok(())
    }

    pub(crate) async fn authorize_selected_version(
        state: &'a Arc<AppState>,
        user: &'a AuthenticatedUser,
        artifact: &Artifact,
        selected: &attune_common::models::ArtifactVersion,
    ) -> ApiResult<()> {
        let guard =
            Self::authorize_selected_version_with_receipts(state, user, artifact, selected).await?;
        crate::receipt_disclosure::HumanReceiptGuard::finish_shared(guard)
            .await?
            .ensure_fresh()?;
        if user.claims.exp <= chrono::Utc::now().timestamp() {
            return Err(ApiError::Unauthorized("Artifact credential expired".into()));
        }
        Ok(())
    }

    /// Pin one version before preparing only its actual bounded source graph.
    /// This must never be used to route recorded credentials through human IAM.
    pub(crate) async fn authorize_selected_version_with_receipts(
        state: &'a Arc<AppState>,
        user: &'a AuthenticatedUser,
        artifact: &Artifact,
        selected: &attune_common::models::ArtifactVersion,
    ) -> ApiResult<crate::receipt_disclosure::SharedHumanReceiptGuard> {
        use attune_common::{
            auth::jwt::TokenType, repositories::sensor_schema::SensorSchemaRepository,
        };
        let denied = || ApiError::Forbidden("Selected artifact disclosure is unavailable".into());
        if selected.artifact != artifact.id {
            return Err(denied());
        }
        let generation =
            SensorSchemaRepository::find_log_generation(&state.db, selected.id).await?;
        if artifact.classification != ArtifactClassification::RuntimeLog && generation.is_none() {
            return Self::authorize_detail_with_receipts(state, user, artifact).await;
        }
        if user.claims.token_type != TokenType::Access {
            return Err(denied());
        }
        let sensor_ref = match generation {
            Some((binding, _)) => Some(
                SensorSchemaRepository::find_by_binding_id(&state.db, binding)
                    .await?
                    .ok_or_else(denied)?
                    .0
                    .executable
                    .sensor
                    .r#ref,
            ),
            None => {
                if artifact.scope == attune_common::models::OwnerType::Sensor
                    || crate::routes::artifacts::sensor_log_artifact_parts(&artifact.r#ref)
                        .is_some()
                    || selected.execution.is_none()
                {
                    return Err(denied());
                }
                None
            }
        };
        let executions = selected.execution.into_iter().collect::<Vec<_>>();
        let mut guard =
            crate::receipt_disclosure::HumanReceiptGuard::for_executions(state, user, &executions)
                .await?;
        if sensor_ref.is_some() {
            guard.prepare_sensor_logs(state, &[selected.id]).await?;
        }
        let guard = guard.shared();
        let snapshot = guard.lock().await.iam_snapshot().ok_or_else(denied)?;
        let mut reads = Self::with_context(
            state,
            user,
            Some(ArtifactReadContext {
                identity_id: snapshot.identity_id,
                identity_attributes: snapshot.identity_attributes.clone(),
                grants: snapshot.grants.clone(),
            }),
        );
        reads.disclosure_snapshot = Some(snapshot.clone());
        let mut caller = attune_common::rbac::AuthorizationContext::new(snapshot.identity_id);
        caller.identity_attributes = snapshot.identity_attributes.clone();
        reads.source_records.sensor_caller = Some(caller);
        reads.receipt_guard = Some(guard.clone());
        // Reapply current metadata visibility to the pinned version, not latest.
        let visible = {
            let mut held = guard.lock().await;
            ArtifactVersionRepository::find_readable_by_version(
                held.connection(),
                artifact.id,
                selected.version,
                reads.context.as_ref().ok_or_else(denied)?,
            )
            .await?
            .ok_or_else(denied)?
        };
        if visible.id != selected.id || visible.execution != selected.execution {
            return Err(denied());
        }
        if let Some(sensor_ref) = sensor_ref {
            crate::secret_disclosure::authorize_sensor_runtime_log_prepared(
                state,
                user,
                selected.id,
                &sensor_ref,
                &guard,
            )
            .await?;
        }
        if !executions.is_empty() {
            let execution = {
                let mut held = guard.lock().await;
                let execution =
                    ExecutionRepository::find_authorization_rows(held.connection(), &executions)
                        .await?
                        .into_iter()
                        .next()
                        .ok_or_else(denied)?;
                crate::routes::executions::authorize_execution_projection_on(
                    state,
                    user,
                    &execution,
                    attune_common::rbac::Action::Read,
                    &snapshot,
                    &mut reads.source_records.visibility,
                    held.connection(),
                )
                .await?;
                execution
            };
            reads.load_execution_disclosure(&executions).await?;
            if reads.execution_disclosure.get(&execution.id) != Some(&true) {
                return Err(denied());
            }
        }
        let mut held = guard.lock().await;
        // Artifact refs and version linkage alone cannot establish a surviving
        // structural owner. Resolve the retained owner binding under the same
        // source/IAM transaction, including for plaintext runtime logs.
        let target = attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve(
            held.connection(),
            attune_common::repositories::runtime_target::RuntimeTarget::Artifact(artifact.id),
            snapshot.identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        let context = target.context();
        if context.owner_type != Some(artifact.scope)
            || context.owner_ref.as_deref() != Some(artifact.owner.as_str())
            || context.target_ref.as_deref() != Some(artifact.r#ref.as_str())
            || match artifact.scope {
                attune_common::models::OwnerType::Pack
                | attune_common::models::OwnerType::Action
                | attune_common::models::OwnerType::Sensor => {
                    target.owner_id().is_none() || target.pack_id().is_none()
                }
                attune_common::models::OwnerType::Identity => target.owner_id().is_none(),
                attune_common::models::OwnerType::System => false,
            }
        {
            return Err(denied());
        }
        let pinned =
            ArtifactVersionRepository::find_pinned_for_share(held.connection(), selected.id)
                .await?
                .ok_or_else(denied)?;
        if pinned.artifact != selected.artifact
            || pinned.version != selected.version
            || pinned.execution != selected.execution
            || pinned.file_path != selected.file_path
            || pinned.object_key != selected.object_key
            || pinned.provider_version != selected.provider_version
            || (selected.body_state == Some(attune_common::models::ArtifactBodyState::Ready)
                && (pinned.body_state != selected.body_state
                    || pinned.sha256 != selected.sha256
                    || pinned.size_bytes != selected.size_bytes))
        {
            return Err(denied());
        }
        drop(held);
        reads
            .source_records
            .take_receipt_freshness()
            .ensure_fresh()?;
        drop(reads);
        Ok(guard)
    }

    pub(crate) async fn version_summaries(
        &mut self,
        artifact: &ArtifactMetadataRow,
        per_page: u32,
        raw_cursor: Option<&str>,
    ) -> ApiResult<ArtifactCursorPage<ArtifactVersionSummary>> {
        let cursor = Cursor::versions(
            raw_cursor,
            artifact.id,
            Some(self.state.jwt_config.secret.as_str()),
        )?;
        if crate::recorded_consumers::is_recorded(self.user)? {
            return crate::recorded_reads::history_page(
                &self.state.db,
                self.user,
                artifact.id,
                self.state.config.security.encryption_key.as_deref(),
                per_page,
                &cursor,
            )
            .await;
        }
        self.authorize_metadata_header(artifact.id, artifact.classification)
            .await?;
        let rows = ArtifactVersionRepository::scan_summaries(
            &self.state.db,
            artifact.id,
            self.context.as_ref(),
            per_page + 1,
            cursor.version_after(),
        )
        .await?;
        cursor.page(
            rows.into_iter().map(ArtifactVersionSummary::from).collect(),
            per_page,
            |row| Position::Version {
                version: row.version,
                id: row.id,
            },
        )
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
        let readable = if let Some(guard) = &self.receipt_guard {
            let mut held = guard.lock().await;
            ArtifactRepository::is_readable(held.connection(), id, context).await?
        } else {
            ArtifactRepository::is_readable(&self.state.db, id, context).await?
        };
        if !readable {
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
        raw_cursor: Option<&str>,
    ) -> ApiResult<ArtifactCursorPage<ArtifactSummary>> {
        let cursor = Cursor::artifacts(
            raw_cursor,
            "execution_artifacts",
            filters,
            Some(self.state.jwt_config.secret.as_str()),
        )?;
        if crate::recorded_consumers::is_recorded(self.user)? {
            return crate::recorded_consumers::artifact_summaries(
                &self.state.db,
                self.user,
                filters,
                &cursor,
                true,
            )
            .await;
        }
        let execution_id = filters.execution.ok_or_else(|| {
            ApiError::BadRequest("Execution artifact selection requires an execution".into())
        })?;
        let execution =
            ExecutionRepository::find_authorization_rows(&self.state.db, &[execution_id])
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| ApiError::NotFound("Execution not found".into()))?;
        crate::routes::executions::authorize_execution_projection(
            self.state,
            self.user,
            &execution,
            attune_common::rbac::Action::Read,
            None,
            &mut crate::routes::executions::ExecutionVisibilityCache::default(),
        )
        .await?;
        let mut candidates = filters.clone();
        candidates.limit = filters.limit + 1;
        let rows = ArtifactRepository::scan_execution_summaries(
            &self.state.db,
            &candidates,
            self.context.as_ref(),
            cursor.artifact_after(),
        )
        .await?;
        cursor.page(
            rows.into_iter().map(ArtifactSummary::from).collect(),
            filters.limit,
            |row| Position::Artifact {
                created: row.created,
                id: row.id,
            },
        )
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
            let rows = if let Some(guard) = &self.receipt_guard {
                let mut held = guard.lock().await;
                ArtifactRepository::linked_execution_ids_many(held.connection(), batch).await?
            } else {
                ArtifactRepository::linked_execution_ids_many(&self.state.db, batch).await?
            };
            links.extend(rows);
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
        if let Some(guard) = self.receipt_guard.clone() {
            return self.load_execution_disclosure_guarded(ids, guard).await;
        }
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
        let missing_origins = ids;
        if !missing_origins.is_empty() {
            for row in ExecutionSecretValueRepository::find_provenance_by_entities(
                &self.state.db,
                &[
                    ENTITY_EXECUTION_CONFIG,
                    ENTITY_EXECUTION_RESULT,
                    "workflow_variables",
                ],
                missing_origins,
            )
            .await?
            {
                secrets.entry(row.entity_id).or_default().push(row);
            }
            for row in crate::secret_disclosure::runtime_key_read_provenance(
                &self.state.db,
                missing_origins,
            )
            .await?
            {
                secrets.entry(row.entity_id).or_default().push(row);
            }
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
                        let result = authorize_runtime_log_cached(
                            self.state,
                            self.user,
                            execution,
                            secrets,
                            snapshot,
                            &mut self.source_records,
                        )
                        .await;
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

    /// All preparation uses the retained source/IAM transaction. Release its
    /// mutex before the recursive evaluator, which takes short guard borrows for
    /// its own target checks. Never reacquire the pool while that transaction lives.
    async fn load_execution_disclosure_guarded(
        &mut self,
        ids: &[i64],
        guard: crate::receipt_disclosure::SharedHumanReceiptGuard,
    ) -> ApiResult<()> {
        let denied = || ApiError::Forbidden("Runtime log disclosure evidence unavailable".into());
        let (executions, bindings, secrets, sensitive, snapshot) = {
            let mut held = guard.lock().await;
            let snapshot = held.credential_snapshot().ok_or_else(denied)?;
            let executions = ExecutionRepository::find_authorization_rows(held.connection(), ids)
                .await?
                .into_iter()
                .map(|row| (row.id, row))
                .collect::<HashMap<_, _>>();
            let bindings = SchemaContractRepository::find_many(
                held.connection(),
                BindingOwner::Execution,
                ids,
            )
            .await?
            .into_iter()
            .collect::<HashMap<_, _>>();
            let mut secrets = HashMap::new();
            for id in ids {
                secrets.insert(
                    *id,
                    held.execution_origins(*id).ok_or_else(denied)?.to_vec(),
                );
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
                .load_executions_on(
                    held.connection(),
                    &sensitive.iter().copied().collect::<Vec<_>>(),
                )
                .await?;
            self.source_records
                .prepare_on(
                    held.connection(),
                    self.user,
                    &snapshot.grants,
                    secrets
                        .iter()
                        .filter(|(id, _)| sensitive.contains(id))
                        .flat_map(|(_, rows)| rows)
                        .map(|row| (row.source_kind.as_str(), row.source_ref.as_deref())),
                )
                .await?;
            (executions, bindings, secrets, sensitive, snapshot)
        };
        self.disclosure_snapshot = Some(snapshot.clone());
        for id in ids {
            let allowed = match (executions.get(id), bindings.get(id)) {
                (Some(execution), Some(_)) if sensitive.contains(id) => {
                    match crate::secret_disclosure::authorize_runtime_log_cached_guarded(
                        self.state,
                        self.user,
                        execution,
                        secrets.get(id).map(Vec::as_slice).unwrap_or(&[]),
                        &snapshot,
                        &mut self.source_records,
                        guard.clone(),
                    )
                    .await
                    {
                        Ok(()) => true,
                        Err(ApiError::Forbidden(_) | ApiError::NotFound(_)) => false,
                        Err(error) => return Err(error),
                    }
                }
                (Some(_), Some(_)) => true,
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
