//! Transaction-scoped file publication checks. External uploads are not effects
//! until a second, freshly authorized transaction publishes their metadata.
use attune_common::repositories::{
    runtime_authority::ValidatedRuntimeAuthority,
    worker_delivery::{DeliveryContext, DeliveryEffectFence, DeliveryPermit},
};
use attune_common::{
    rbac::Action,
    repositories::{artifact::ArtifactRepository, runtime_target::RuntimeTarget},
};
use sqlx::PgConnection;

pub(crate) struct FileMutationProof {
    pub credential_deadline: i64,
    pub process: Option<ValidatedRuntimeAuthority>,
    pub process_delivery: Option<DeliveryEffectFence>,
    pub worker: Option<(DeliveryContext, DeliveryPermit, DeliveryEffectFence)>,
    pub file_origin_budget: Option<crate::recorded_disclosure::OriginBudget>,
}
impl FileMutationProof {
    pub(crate) async fn commit(
        &self,
        transaction: sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> ApiResult<()> {
        transaction.commit().await?;
        self.ensure_fresh()
    }
    pub(crate) fn ensure_fresh(&self) -> ApiResult<()> {
        if let Some(authority) = &self.process {
            authority.ensure_fresh()?;
        }
        if let Some(effect) = &self.process_delivery {
            effect.ensure_fresh()?;
        }
        if let Some((_, _, effect)) = &self.worker {
            effect.ensure_fresh()?;
        }
        if let Some(budget) = &self.file_origin_budget {
            budget.ensure_receipts_fresh()?;
        }
        require_credential_fresh(self.credential_deadline)
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> ApiResult<()> {
        if let Some(authority) = &self.process {
            authority.ensure_fresh_on(conn).await?;
        }
        if let Some(effect) = &self.process_delivery {
            effect.ensure_fresh_on(conn).await?;
        }
        if let Some((_, _, effect)) = &self.worker {
            effect.ensure_fresh_on(conn).await?;
        }
        if let Some(budget) = &self.file_origin_budget {
            budget.ensure_receipts_fresh_on(conn).await?;
        }
        self.ensure_fresh()
    }
}

pub(crate) struct FileSourceProof {
    credential_deadline: i64,
    authority: ValidatedRuntimeAuthority,
    origins: crate::file_origins::PreparedFileOrigins,
}
impl std::ops::Deref for FileSourceProof {
    type Target = ValidatedRuntimeAuthority;
    fn deref(&self) -> &Self::Target {
        &self.authority
    }
}
impl FileSourceProof {
    pub(crate) fn ensure_fresh(&self) -> ApiResult<()> {
        self.authority.ensure_fresh()?;
        self.origins.ensure_fresh()?;
        require_credential_fresh(self.credential_deadline)
    }
    pub(crate) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> ApiResult<()> {
        self.authority.ensure_fresh_on(conn).await?;
        self.origins.ensure_fresh_on(conn).await?;
        self.ensure_fresh()
    }
}
pub(crate) async fn prepare_file_source(
    conn: &mut PgConnection,
    user: &AuthenticatedUser,
    key: Option<&str>,
) -> ApiResult<Option<FileSourceProof>> {
    if !crate::recorded_consumers::is_recorded(user)? {
        return Ok(None);
    }
    let authority = user
        .validate_recorded_runtime_authority(conn)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded file source unavailable".into()))?;
    let origins = crate::file_origins::prepare_reservation(conn, &authority, key).await?;
    Ok(Some(FileSourceProof {
        credential_deadline: user.claims.exp,
        authority,
        origins,
    }))
}

fn require_credential_fresh(deadline: i64) -> ApiResult<()> {
    if deadline <= chrono::Utc::now().timestamp() {
        return Err(ApiError::Forbidden(
            "File credential expired before effect or delivery".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod freshness_tests {
    #[test]
    fn final_file_handoff_does_not_add_credential_expiry_grace() {
        assert!(super::require_credential_fresh(0).is_err());
        assert!(super::require_credential_fresh(chrono::Utc::now().timestamp() - 1).is_err());
        assert!(super::require_credential_fresh(i64::MAX).is_ok());
    }
}
pub(crate) async fn authorize_body_mutation(
    conn: &mut PgConnection,
    user: &AuthenticatedUser,
    artifact: i64,
    key: Option<&str>,
) -> ApiResult<Option<FileSourceProof>> {
    let source = prepare_file_source(conn, user, key).await?;
    if let Some(source) = &source {
        authorize_target(conn, source, artifact, Action::Update).await?;
    }
    Ok(source)
}

use crate::{
    auth::middleware::AuthenticatedUser,
    middleware::{ApiError, ApiResult},
};

/// Trusted marker on append/seal/complete endpoints only, never reads/deletes.
#[derive(Clone, Copy)]
pub(crate) struct WorkerDeliveryConsumer;

/// Arbitrary version metadata is a second unstructured payload channel. Until
/// it has a distinct encrypted/provenance-bound projection, admitted file APIs
/// accept body bytes only, not caller-supplied JSON or creator labels to echo.
pub(crate) fn validate_file_descriptors(
    user: &AuthenticatedUser,
    meta: &Option<serde_json::Value>,
    created_by: &Option<String>,
) -> ApiResult<()> {
    if crate::recorded_consumers::is_recorded(user)? && (meta.is_some() || created_by.is_some()) {
        return Err(ApiError::BadRequest("Recorded file payloads must use the encrypted body; arbitrary version meta/created_by are not a protected file channel".into()));
    }
    Ok(())
}

pub(crate) fn execution_attribution(
    user: &AuthenticatedUser,
    requested: Option<i64>,
) -> ApiResult<Option<i64>> {
    if crate::recorded_consumers::is_recorded(user)? {
        let own = user
            .execution_id()
            .ok_or_else(|| ApiError::Forbidden("Execution attribution unavailable".into()))?;
        if requested.is_some_and(|id| id != own) {
            return Err(ApiError::Forbidden(
                "Recorded file versions cannot impersonate another execution".into(),
            ));
        }
        Ok(Some(own))
    } else {
        Ok(requested.or_else(|| user.execution_id()))
    }
}

/// Source fence first, then artifact-version advisory lock and structural owner.
/// Keep this connection's transaction through the reserve/publication commit.
pub(crate) async fn authorize_mutation(
    conn: &mut PgConnection,
    user: &AuthenticatedUser,
    artifact: i64,
) -> ApiResult<Option<ValidatedRuntimeAuthority>> {
    authorize_operation(conn, user, artifact, Action::Update).await
}

/// File metadata is not a new body. Keep the target mutation proof without
/// loading source origins or requiring that their read context is still open.
pub(crate) async fn metadata_mutation(
    pool: &sqlx::PgPool,
    user: &AuthenticatedUser,
    artifact: i64,
) -> ApiResult<Option<crate::recorded_effect::EffectTransaction<'static>>> {
    if !crate::recorded_consumers::is_recorded(user)? {
        return Ok(None);
    }
    let mut tx = pool.begin().await?;
    let authority = authorize_mutation(&mut tx, user, artifact)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded file authority unavailable".into()))?;
    let selected = ArtifactRepository::find_by_id_for_update(&mut tx, artifact)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Artifact unavailable".into()))?;
    if attune_common::repositories::artifact::is_file_backed_type(selected.r#type) {
        Ok(Some(
            crate::recorded_effect::EffectTransaction::from_authority(tx, Some(authority)),
        ))
    } else {
        tx.rollback().await?;
        crate::recorded_consumers::artifact_mutation(pool, user, artifact, Action::Update).await
    }
}

async fn authorize_operation(
    conn: &mut PgConnection,
    user: &AuthenticatedUser,
    artifact: i64,
    action: Action,
) -> ApiResult<Option<ValidatedRuntimeAuthority>> {
    if !crate::recorded_consumers::is_recorded(user)? {
        return Ok(None);
    }
    let authority = user
        .recorded_runtime_authority(conn)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority unavailable".into()))?;
    authorize_target(conn, &authority, artifact, action).await?;
    Ok(Some(authority))
}

pub(crate) async fn authorize_target(
    conn: &mut PgConnection,
    authority: &ValidatedRuntimeAuthority,
    artifact: i64,
    action: Action,
) -> ApiResult<()> {
    ArtifactRepository::find_by_id_for_update(conn, artifact)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Artifact unavailable".into()))?;
    let target = attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve(
        conn,
        RuntimeTarget::Artifact(artifact),
        authority.credential_binding().sponsor_identity_id,
    )
    .await?
    .ok_or_else(|| ApiError::Forbidden("Artifact owner unavailable".into()))?;
    if !authority.allows_new_effect(&target, action) {
        return Err(ApiError::Forbidden(
            "Recorded authority does not permit file publication".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn authorize_publication(
    conn: &mut PgConnection,
    authority: Option<&ValidatedRuntimeAuthority>,
    version: i64,
    artifact: i64,
) -> ApiResult<()> {
    let Some(authority) = authority else {
        return Ok(());
    };
    use attune_common::repositories::worker_delivery::WorkerDeliveryRepository;
    let permit = WorkerDeliveryRepository::for_version(conn, version)
        .await?
        .ok_or_else(|| ApiError::Forbidden("File publication binding unavailable".into()))?;
    let context =
        WorkerDeliveryRepository::for_execution(conn, authority.credential_binding().execution_id)
            .await?
            .ok_or_else(|| ApiError::Forbidden("File delivery context unavailable".into()))?;
    if permit.context != context.id
        || context.admission != authority.credential_binding().admission_id
        || context.attempt != authority.credential_binding().attempt
    {
        return Err(ApiError::Forbidden(
            "File publication binding mismatch".into(),
        ));
    }
    let action = match permit.publication_action.as_str() {
        "create" => Action::Create,
        "update" => Action::Update,
        _ => {
            return Err(ApiError::Forbidden(
                "Unknown file publication operation".into(),
            ))
        }
    };
    authorize_target(conn, authority, artifact, action).await?;
    WorkerDeliveryRepository::require_owner_binding(conn, permit.id, version).await?;
    Ok(())
}

/// A running action may allocate a file only under its prelaunch context. The
/// worker later receives this exact version's permit, not an execution-wide one.
pub(crate) async fn bind_completion(
    conn: &mut PgConnection,
    user: &AuthenticatedUser,
    version: i64,
    max_bytes: u64,
    action: Action,
    authority: Option<&FileSourceProof>,
) -> ApiResult<()> {
    let Some(binding) = user
        .claims
        .admitted_execution_credential()
        .map_err(|_| ApiError::Forbidden("Invalid recorded credential".into()))?
    else {
        return Ok(());
    };
    use attune_common::repositories::worker_delivery::WorkerDeliveryRepository;
    let context = WorkerDeliveryRepository::for_execution(conn, binding.execution_id)
        .await?
        .ok_or_else(|| {
            ApiError::Forbidden("Prelaunch worker delivery context unavailable".into())
        })?;
    if context.admission != binding.admission_id || context.attempt != binding.attempt {
        return Err(ApiError::Forbidden(
            "Worker delivery context does not match current attempt".into(),
        ));
    }
    let authority = authority.ok_or_else(|| {
        ApiError::Forbidden("File allocation must retain its source authority".into())
    })?;
    let effect = WorkerDeliveryRepository::process_effect(authority, &context)?;
    WorkerDeliveryRepository::bind(
        conn,
        &context,
        version,
        None,
        i64::try_from(max_bytes)
            .map_err(|_| ApiError::BadRequest("File delivery byte cap overflow".into()))?,
        0,
        action,
        &effect,
    )
    .await?;
    crate::file_origins::reserve(conn, authority, version, &authority.origins).await?;
    effect.ensure_fresh_on(conn).await?;
    authority.ensure_fresh_on(conn).await?;
    Ok(())
}
