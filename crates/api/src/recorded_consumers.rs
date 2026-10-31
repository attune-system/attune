//! Typed recorded consumers. Never turn an ownership envelope into IAM grants.

use attune_common::{
    models::{ArtifactVisibility, OwnerType},
    rbac::Action,
    repositories::{
        action::ActionRepository,
        artifact::ArtifactRepository,
        pack::PackRepository,
        runtime_authority::ValidatedRuntimeAuthority,
        runtime_target::{
            ProspectiveDataOwner, ProspectiveRuntimeTarget, RuntimeTarget, RuntimeTargetRepository,
        },
        trigger::SensorRepository,
        FindById, FindByRef,
    },
};
use sqlx::{PgConnection, PgPool};

use crate::{
    auth::middleware::AuthenticatedUser,
    middleware::{ApiError, ApiResult},
    recorded_effect::EffectTransaction,
};

pub(crate) fn is_recorded(user: &AuthenticatedUser) -> ApiResult<bool> {
    Ok(user
        .claims
        .admitted_execution_credential()
        .map_err(|_| ApiError::Unauthorized("Invalid recorded credential".into()))?
        .is_some())
}

/// Exported for execution/queue integration. The caller must retain this
/// transaction through the protected write, not validate and then reacquire.
pub async fn authorize_target(
    conn: &mut PgConnection,
    user: &AuthenticatedUser,
    target: RuntimeTarget,
    action: Action,
) -> ApiResult<ValidatedRuntimeAuthority> {
    let authority = user
        .recorded_runtime_authority(conn)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded execution authority required".into()))?;
    let target = RuntimeTargetRepository::resolve(
        conn,
        target,
        authority.credential_binding().sponsor_identity_id,
    )
    .await?
    .ok_or_else(|| ApiError::Forbidden("Runtime target unavailable".into()))?;
    let allowed = if matches!(action, Action::Read | Action::Decrypt) {
        authority.allows_target(&target, action)
    } else {
        authority.allows_new_effect(&target, action)
    };
    if !allowed {
        return Err(ApiError::Forbidden(
            "Recorded authority does not permit this operation".into(),
        ));
    }
    authority.ensure_fresh_on(conn).await?;
    Ok(authority)
}

/// Execution-route hook. Read/decrypt authority alone does not disclose copied
/// input material; every retained origin must also be authorized. The caller
/// retains the transaction until it has prepared the response.
pub async fn authorize_execution_content(
    conn: &mut PgConnection,
    user: &AuthenticatedUser,
    execution_id: i64,
) -> ApiResult<ValidatedRuntimeAuthority> {
    authorize_execution_content_with_key(conn, user, execution_id, None).await
}

pub async fn authorize_execution_content_with_key(
    conn: &mut PgConnection,
    user: &AuthenticatedUser,
    execution_id: i64,
    encryption_key: Option<&str>,
) -> ApiResult<ValidatedRuntimeAuthority> {
    let prepared =
        authorize_execution_content_prepared(conn, user, execution_id, encryption_key).await?;
    prepared.ensure_fresh()?;
    Ok(prepared.authority)
}

/// Holds the source proofs and exact encrypted buffers through response reads
/// and the caller's capture commit. No personal IAM or API selection fallback.
#[must_use]
pub struct PreparedExecutionContent {
    execution: i64,
    authority: ValidatedRuntimeAuthority,
    budget: crate::recorded_disclosure::OriginBudget,
    delivery: attune_common::repositories::immutable_input_origin::ReceiptFreshness,
    fields: std::collections::BTreeMap<
        &'static str,
        Vec<attune_common::secret_values::StoredSecretValue>,
    >,
    encryption_key: Option<String>,
    jwt_exp: i64,
}

impl PreparedExecutionContent {
    pub async fn restore_entity(
        &self,
        conn: &mut PgConnection,
        entity: &str,
        value: serde_json::Value,
    ) -> ApiResult<serde_json::Value> {
        use attune_common::repositories::{
            execution_secret_value::ExecutionSecretValueRepository,
            immutable_input_origin::bind_input_receipt_buffers,
        };
        let frozen = self
            .fields
            .get(entity)
            .ok_or_else(|| ApiError::Forbidden("Execution content unavailable".into()))?;
        let current = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn,
            entity,
            self.execution,
        )
        .await?;
        if &current != frozen {
            return Err(ApiError::Forbidden("Execution content unavailable".into()));
        }
        bind_input_receipt_buffers(conn, entity, self.execution, frozen).await?;
        if frozen.is_empty() {
            return Ok(value);
        }
        Ok(attune_common::secret_values::restore_secret_values(
            value,
            frozen,
            self.encryption_key
                .as_deref()
                .ok_or_else(|| ApiError::Forbidden("Execution encryption unavailable".into()))?,
        )?)
    }

    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> ApiResult<()> {
        self.budget.ensure_receipts_fresh_on(conn).await?;
        self.authority.ensure_fresh_on(conn).await?;
        self.ensure_fresh()
    }

    /// Run after the final commit, with no subsequent application await before
    /// returning the response. This includes the token deadline, not just A's.
    pub fn ensure_fresh(&self) -> ApiResult<()> {
        self.budget.ensure_receipts_fresh()?;
        self.delivery.ensure_fresh()?;
        self.authority.ensure_fresh()?;
        if chrono::Utc::now().timestamp() >= self.jwt_exp {
            return Err(ApiError::Forbidden("Execution content unavailable".into()));
        }
        Ok(())
    }
}

pub async fn authorize_execution_content_prepared(
    conn: &mut PgConnection,
    user: &AuthenticatedUser,
    execution_id: i64,
    encryption_key: Option<&str>,
) -> ApiResult<PreparedExecutionContent> {
    let authority = user
        .validate_recorded_runtime_authority(conn)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority required".into()))?;
    let rows = crate::recorded_disclosure::execution_origins(conn, execution_id).await?;
    let mut prepared =
        crate::recorded_disclosure::OriginBudget::with_encryption_key(encryption_key);
    let mut fields = std::collections::BTreeMap::new();
    let mut delivery =
        attune_common::repositories::immutable_input_origin::ReceiptFreshness::default();
    let mut field_bytes = 0usize;
    for entity in [
        attune_common::secret_values::ENTITY_EXECUTION_CONFIG,
        attune_common::secret_values::ENTITY_EXECUTION_RESULT,
    ] {
        let frozen = attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(&mut *conn, entity, execution_id).await?;
        for field in &frozen {
            let charge = serde_json::to_vec(&(
                &field.json_path,
                &field.encrypted_value,
                &field.encryption_key_hash,
                &field.source_kind,
                &field.source_ref,
            ))
            .map_err(|_| ApiError::Forbidden("Execution content unavailable".into()))?
            .len();
            field_bytes = field_bytes
                .checked_add(charge)
                .ok_or_else(|| ApiError::Forbidden("Execution content unavailable".into()))?;
        }
        if frozen.len() > 256 || field_bytes > 1024 * 1024 {
            return Err(ApiError::Forbidden("Execution content unavailable".into()));
        }
        let mut has_receipt = false;
        for field in &frozen {
            if field.source_kind != "provenance" {
                return Err(ApiError::Forbidden("Execution content unavailable".into()));
            }
            let provenance: attune_common::secret_provenance::SecretProvenance =
                serde_json::from_str(
                    field.source_ref.as_deref().ok_or_else(|| {
                        ApiError::Forbidden("Execution content unavailable".into())
                    })?,
                )
                .map_err(|_| ApiError::Forbidden("Execution content unavailable".into()))?;
            has_receipt |= provenance
                .origins
                .iter()
                .any(attune_common::repositories::immutable_input_origin::is_input_receipt);
        }
        if has_receipt {
            let (_, contexts) =
                attune_common::repositories::immutable_input_origin::prepare_input_receipts(
                    conn,
                    entity,
                    execution_id,
                    &frozen,
                    encryption_key.ok_or_else(|| {
                        ApiError::Forbidden("Execution encryption unavailable".into())
                    })?,
                )
                .await?;
            for context in contexts {
                delivery.retain(context)?;
            }
        }
        fields.insert(entity, frozen);
    }
    prepared.prepare_receipts(conn, &rows).await?;
    for (entity, frozen) in &fields {
        attune_common::repositories::immutable_input_origin::bind_input_receipt_buffers(
            conn,
            entity,
            execution_id,
            frozen,
        )
        .await?;
    }
    attune_common::repositories::runtime_read::RuntimeReadRepository::lock_open(
        conn,
        authority.credential_binding().admission_id,
    )
    .await?;
    let (origins, budget) =
        execution_content_frontier(conn, &authority, execution_id, None, prepared).await?;
    attune_common::repositories::runtime_read::RuntimeReadRepository::record(
        conn,
        &authority,
        attune_common::repositories::runtime_read::RuntimeReadOrigin {
            target: attune_common::repositories::runtime_read::RuntimeReadTarget::Execution {
                execution: execution_id,
            },
            provenance: crate::recorded_disclosure::snapshot(&origins)?,
            parents: budget.retained_parents(),
        },
    )
    .await?;
    budget.ensure_receipts_fresh_on(conn).await?;
    authority.ensure_fresh_on(conn).await?;
    Ok(PreparedExecutionContent {
        execution: execution_id,
        authority,
        budget,
        delivery,
        fields,
        encryption_key: encryption_key.map(str::to_owned),
        jwt_exp: user.claims.exp,
    })
}

/// Content half shared with live log acquisition. Source validation is supplied
/// by the caller, never repeated behind its target locks. A live caller holds
/// the producer's capture fence through origin snapshotting and publication.
pub(crate) async fn execution_content_frontier(
    conn: &mut PgConnection,
    authority: &ValidatedRuntimeAuthority,
    execution_id: i64,
    self_log: Option<i64>,
    mut budget: crate::recorded_disclosure::OriginBudget,
) -> ApiResult<(
    Vec<attune_common::repositories::execution_secret_value::SecretProvenanceRow>,
    crate::recorded_disclosure::OriginBudget,
)> {
    crate::recorded_disclosure::require_target(
        conn,
        authority,
        RuntimeTarget::Execution(execution_id),
        &[Action::Read, Action::Decrypt],
    )
    .await?;
    let binding = attune_common::repositories::schema_contract::SchemaContractRepository::find(
        &mut *conn,
        attune_common::repositories::schema_contract::BindingOwner::Execution,
        execution_id,
    )
    .await?
    .ok_or_else(|| ApiError::Forbidden("Execution disclosure binding unavailable".into()))?;
    let origins = execution_content_origins(
        conn,
        execution_id,
        self_log.filter(|_| authority.credential_binding().execution_id == execution_id),
    )
    .await?;
    crate::recorded_disclosure::authorize_origins_bounded(conn, authority, &origins, &mut budget)
        .await?;
    crate::secret_disclosure::runtime_log_requires_disclosure(&binding, &origins)?;
    authority.ensure_fresh_on(conn).await?;
    Ok((origins, budget))
}

pub(crate) async fn execution_content_origins(
    conn: &mut PgConnection,
    execution_id: i64,
    self_log: Option<i64>,
) -> ApiResult<Vec<attune_common::repositories::execution_secret_value::SecretProvenanceRow>> {
    let mut origins = crate::recorded_disclosure::execution_origins(conn, execution_id).await?;
    if let Some(stream) = self_log {
        use attune_common::{
            repositories::runtime_read::{RuntimeReadRepository, ENTITY_RUNTIME_READ},
            secret_provenance::{SecretOrigin, SecretProvenance},
        };
        let carriers = RuntimeReadRepository::self_log_carriers(conn, execution_id, stream).await?;
        origins.retain(|row| {
            if row.entity_type != ENTITY_RUNTIME_READ { return true; }
            let Some(provenance) = row.source_ref.as_deref().and_then(|reference| serde_json::from_str::<SecretProvenance>(reference).ok()) else { return true; };
            !matches!(provenance.origins.as_slice(), [SecretOrigin::Entity { entity_type, entity_id, path }] if entity_type == ENTITY_RUNTIME_READ && path.is_empty() && carriers.contains(entity_id))
        });
    }
    Ok(origins)
}

pub(crate) async fn artifact_mutation(
    pool: &PgPool,
    user: &AuthenticatedUser,
    id: i64,
    action: Action,
) -> ApiResult<Option<EffectTransaction<'static>>> {
    if !is_recorded(user)? {
        return Ok(None);
    }
    let mut tx = pool.begin().await?;
    if action == Action::Delete {
        attune_common::repositories::file_evidence_retention::FileEvidenceRetentionRepository::lock_reclamation(&mut tx).await?;
    }
    let authority = user
        .recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority required".into()))?;
    ArtifactRepository::find_by_id_for_update(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Artifact unavailable".into()))?;
    let target = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Artifact(id),
        authority.credential_binding().sponsor_identity_id,
    )
    .await?
    .ok_or_else(|| ApiError::Forbidden("Artifact unavailable".into()))?;
    if !authority.allows_new_effect(&target, action) {
        return Err(ApiError::Forbidden(
            "Recorded authority does not permit artifact mutation".into(),
        ));
    }
    attune_common::repositories::runtime_read::RuntimeReadRepository::require_untainted(
        &mut tx,
        authority.credential_binding().admission_id,
    )
    .await?;
    Ok(Some(EffectTransaction::from_authority(tx, Some(authority))))
}

pub(crate) async fn artifact_create(
    pool: &PgPool,
    user: &AuthenticatedUser,
    reference: &str,
    scope: OwnerType,
    owner_ref: &str,
    visibility: ArtifactVisibility,
) -> ApiResult<Option<EffectTransaction<'static>>> {
    if !is_recorded(user)? {
        return Ok(None);
    }
    let mut tx = pool.begin().await?;
    let authority = user
        .recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority required".into()))?;
    authorize_artifact_owner(&mut tx, &authority, reference, scope, owner_ref, visibility).await?;
    attune_common::repositories::runtime_read::RuntimeReadRepository::require_untainted(
        &mut tx,
        authority.credential_binding().admission_id,
    )
    .await?;
    Ok(Some(EffectTransaction::from_authority(tx, Some(authority))))
}

pub(crate) async fn authorize_artifact_owner(
    conn: &mut PgConnection,
    authority: &ValidatedRuntimeAuthority,
    reference: &str,
    scope: OwnerType,
    owner_ref: &str,
    visibility: ArtifactVisibility,
) -> ApiResult<()> {
    let missing = || ApiError::Forbidden("Artifact owner unavailable".into());
    let owner = match scope {
        OwnerType::System => ProspectiveDataOwner::System,
        OwnerType::Identity => {
            ProspectiveDataOwner::Identity(owner_ref.parse().map_err(|_| missing())?)
        }
        OwnerType::Pack => ProspectiveDataOwner::Pack(
            PackRepository::find_by_ref(&mut *conn, owner_ref)
                .await?
                .ok_or_else(missing)?
                .id,
        ),
        OwnerType::Action => ProspectiveDataOwner::Action(
            ActionRepository::find_by_ref(&mut *conn, owner_ref)
                .await?
                .ok_or_else(missing)?
                .id,
        ),
        OwnerType::Sensor => ProspectiveDataOwner::Sensor(
            SensorRepository::find_by_ref(&mut *conn, owner_ref)
                .await?
                .ok_or_else(missing)?
                .id,
        ),
    };
    let target = RuntimeTargetRepository::resolve_creation_target(
        conn,
        owner,
        ProspectiveRuntimeTarget::Artifact {
            reference,
            visibility,
        },
        authority.credential_binding().sponsor_identity_id,
    )
    .await?
    .ok_or_else(missing)?;
    // The ref resolved before locking must still name that same structural ID.
    if target.context().owner_ref.as_deref() != Some(owner_ref) && scope != OwnerType::System
        || !authority.allows_new_effect(&target, Action::Create)
    {
        return Err(ApiError::Forbidden(
            "Recorded authority does not permit artifact creation".into(),
        ));
    }
    authority.ensure_fresh_on(conn).await?;
    Ok(())
}

/// An update-only permit must not return the previous data payload. Runtime
/// logs always use the dedicated disclosure path, never a mutation response.
pub(crate) fn redact_mutation(artifact: &mut attune_common::models::artifact::Artifact) {
    artifact.data = None;
}

pub(crate) async fn artifact_read(
    pool: &PgPool,
    user: &AuthenticatedUser,
    artifact_id: i64,
) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    let authority = user
        .recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority required".into()))?;
    let links =
        ArtifactRepository::linked_execution_ids_bounded(&mut *tx, artifact_id, 257).await?;
    let artifact = ArtifactRepository::find_by_id(&mut *tx, artifact_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Artifact unavailable".into()))?;
    if links.is_empty()
        && artifact.classification == attune_common::models::ArtifactClassification::RuntimeLog
    {
        return Err(ApiError::Forbidden(
            "Runtime log has no retained execution disclosure binding".into(),
        ));
    }
    // All linked executions must be visible. Being one batch contributor does
    // not make the whole artifact readable. Own-execution SDK access is allowed.
    if links.len() > 256 {
        return Err(ApiError::Forbidden(
            "Artifact disclosure graph exceeds its bound".into(),
        ));
    }
    let mut origin_budget = crate::recorded_disclosure::OriginBudget::default();
    // Worker completion locks executions before artifacts. Resolve linked
    // execution/origin scope before taking the artifact row, then recheck the
    // linkage projection. Never upgrade a permit from a changed graph.
    for execution in links.iter().copied() {
        if execution != authority.credential_binding().execution_id {
            crate::recorded_disclosure::require_target(
                &mut tx,
                &authority,
                RuntimeTarget::Execution(execution),
                &[Action::Read],
            )
            .await?;
        }
        // Missing retained bindings never become evidence of non-sensitivity.
        let binding = attune_common::repositories::schema_contract::SchemaContractRepository::find(
            &mut tx,
            attune_common::repositories::schema_contract::BindingOwner::Execution,
            execution,
        )
        .await?
        .ok_or_else(|| ApiError::Forbidden("Execution disclosure binding unavailable".into()))?;
        let origins = crate::recorded_disclosure::execution_origins(&mut tx, execution).await?;
        if crate::secret_disclosure::runtime_log_requires_disclosure(&binding, &origins)? {
            crate::recorded_disclosure::require_target(
                &mut tx,
                &authority,
                RuntimeTarget::Execution(execution),
                &[Action::Read, Action::Decrypt],
            )
            .await?;
            crate::recorded_disclosure::authorize_origins_bounded(
                &mut tx,
                &authority,
                &origins,
                &mut origin_budget,
            )
            .await?;
            // A read permit is not provenance for later outputs/events. The
            // existing SDK registry retains Keys only, so it cannot preserve
            // arbitrary source execution/event/queue visibility. Do not expose
            // protected bytes until generalized runtime-read origins are stored
            // and consumed by output/log/event disclosure in the same fence.
            return Err(ApiError::Forbidden(
                "Runtime plaintext delivery requires retained runtime-read provenance".into(),
            ));
        }
    }
    let target = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Artifact(artifact_id),
        authority.credential_binding().sponsor_identity_id,
    )
    .await?
    .ok_or_else(|| ApiError::Forbidden("Artifact owner binding unavailable".into()))?;
    if !authority.allows_target(&target, Action::Read) {
        return Err(ApiError::Forbidden(
            "Recorded authority does not permit artifact read".into(),
        ));
    }
    if ArtifactRepository::linked_execution_ids_bounded(&mut *tx, artifact_id, 257).await? != links
    {
        return Err(ApiError::Forbidden(
            "Artifact disclosure graph changed".into(),
        ));
    }
    authority.ensure_fresh_on(&mut tx).await?;
    Ok(())
}

pub(crate) async fn artifact_summaries(
    pool: &PgPool,
    user: &AuthenticatedUser,
    filters: &attune_common::repositories::artifact::ArtifactSearchFilters,
    cursor: &crate::artifact_cursor::Cursor,
    execution_scoped: bool,
) -> ApiResult<crate::dto::artifact::ArtifactCursorPage<crate::dto::artifact::ArtifactSummary>> {
    use crate::{artifact_cursor::Position, dto::artifact::ArtifactSummary};
    let mut source = pool.begin().await?;
    let authority = user
        .recorded_runtime_authority(&mut source)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority required".into()))?;
    let fence = authority.effect_fence().clone();
    if execution_scoped {
        let execution = filters.execution.ok_or_else(|| {
            ApiError::BadRequest("Execution artifact selection requires an execution".into())
        })?;
        // Keep the existing SDK own-execution exemption; standard artifact
        // access does not itself grant a general execution-read permission.
        if execution != authority.credential_binding().execution_id {
            crate::recorded_disclosure::require_target(
                &mut source,
                &authority,
                RuntimeTarget::Execution(execution),
                &[Action::Read],
            )
            .await?;
        }
    }
    source.commit().await?;
    // One bounded candidate page, not an unbounded search for readable rows.
    // Empty pages retain a cursor when denied candidates precede more history.
    let mut candidate_filters = filters.clone();
    candidate_filters.limit = filters.limit + 1;
    let mut candidates: Vec<ArtifactSummary> = if execution_scoped {
        ArtifactRepository::scan_execution_summaries(
            pool,
            &candidate_filters,
            None,
            cursor.artifact_after(),
        )
        .await?
        .into_iter()
        .map(ArtifactSummary::from)
        .collect()
    } else {
        ArtifactRepository::scan_summaries(pool, &candidate_filters, None, cursor.artifact_after())
            .await?
            .into_iter()
            .map(ArtifactSummary::from)
            .collect()
    };
    let has_more = candidates.len() > filters.limit as usize;
    candidates.truncate(filters.limit as usize);
    let after = candidates.last().map(|row| Position::Artifact {
        created: row.created,
        id: row.id,
    });
    let mut rows = Vec::new();
    for candidate in candidates {
        match artifact_read(pool, user, candidate.id).await {
            Ok(()) => rows.push(candidate),
            Err(ApiError::Forbidden(_) | ApiError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }
    let mut connection = pool.acquire().await?;
    fence.ensure_fresh_on(&mut connection).await?;
    cursor.scanned_page(rows, has_more, after)
}
