//! CP action acquisition. Claim, attempt and worker-incarnation reservation
//! commit together before any pack preparation or external launcher request.
use super::*;
use crate::{
    execution_authority::RecordedConsumerReadiness,
    models::{Execution, ExecutionStatus},
    repositories::{
        delegated_execution::DelegatedExecutionRepository,
        execution::ExecutionRepository,
        execution_runtime_admission::{ExecutionAdmissionRepository, ExistingExecutionAuthority},
        file_evidence_retention::FileEvidenceRetentionRepository,
        runtime_authority::{RuntimeAuthorityRepository, RuntimeOwner},
        schema_type::SchemaTypeRepository,
        workload_launch::WorkloadLaunchRepository,
        FindById,
    },
    workload_launch::LaunchTarget,
};

/// Server metadata only: deliberately not an external request/deserializable
/// authority grant. The launcher receives the eventual ReservedLaunch instead.
pub struct ClaimedActionSource {
    pub execution: Execution,
    pub target: LaunchTarget,
    pub owner: i64,
    pub admission: i64,
    pub image: i64,
    pub profile: HostExecutionProfile,
    pub api_origin: reqwest::Url,
    pub http_policy: crate::workload_launch_io::LaunchHttpPolicy,
}
impl WorkloadHostRepository {
    pub async fn claim_action(
        pool: &PgPool,
        actor: HostActor,
        known: &[i64],
        recorded: RecordedConsumerReadiness,
    ) -> Result<Option<ClaimedActionSource>> {
        if known.len() > 1024 || known.iter().any(|id| *id <= 0) {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        FileEvidenceRetentionRepository::lock_capture(&mut tx).await?;
        SchemaTypeRepository::lock_selection(&mut tx).await?;
        Self::admit(&mut tx, actor).await?;
        let (profile, _) = Self::enrollment(
            &mut tx,
            EnrollmentActor {
                identity: actor.identity,
                enrollment: actor.enrollment,
                profile: actor.profile,
                generation: actor.generation,
                expires_at: actor.expires_at,
            },
        )
        .await?;
        if profile.role != "action" {
            return Err(denied());
        }
        // Recover only a durable reservation for this exact incarnation. A
        // replacement manager cannot adopt an old still-running attempt.
        let recovering: Option<(i64, Json<LaunchTarget>)> = sqlx::query_as(
            "SELECT e.id,p.source_target FROM execution e JOIN workload_launch_prerequisite p \
             ON p.purpose='action' AND (p.source_target->>'execution')::bigint=e.id \
             WHERE e.worker=$1 AND e.status='running' AND p.source_target->>'worker_instance'=$2 \
             AND NOT(e.id=ANY($3)) ORDER BY e.started_at,e.id LIMIT 1",
        )
        .bind(actor.worker)
        .bind(actor.instance.to_string())
        .bind(known)
        .fetch_optional(&mut *tx)
        .await?;
        let (execution, target, owner) = if let Some((id, target)) = recovering {
            let execution = ExecutionRepository::find_by_id(&mut *tx, id)
                .await?
                .ok_or_else(denied)?;
            let owner = Self::action_owner(&mut tx, id, recorded).await?;
            (execution, target.0, owner)
        } else {
            let running:i64=sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE worker=$1 AND status IN ('running','canceling')")
                .bind(actor.worker).fetch_one(&mut *tx).await?;
            if running >= i64::from(profile.max_concurrent) {
                tx.commit().await?;
                return Ok(None);
            }
            let id:Option<i64>=sqlx::query_scalar("SELECT id FROM execution WHERE worker=$1 AND status='scheduled' AND NOT(id=ANY($2)) ORDER BY created,id LIMIT 1")
                .bind(actor.worker).bind(known).fetch_optional(&mut *tx).await?;
            let Some(id) = id else {
                tx.commit().await?;
                return Ok(None);
            };
            let owner = Self::action_owner(&mut tx, id, recorded).await?;
            let authority = ExecutionAdmissionRepository::selected_authority(&mut tx, id).await?;
            let attempt = match authority {
                ExistingExecutionAuthority::Delegated => {
                    DelegatedExecutionRepository::start(&mut tx, id, actor.worker).await?
                }
                ExistingExecutionAuthority::Recorded => uuid::Uuid::new_v4(),
            };
            let execution = ExecutionRepository::transition_worker(
                &mut *tx,
                id,
                Some(actor.worker),
                ExecutionStatus::Running,
                None,
            )
            .await?
            .execution;
            if authority == ExistingExecutionAuthority::Recorded {
                RuntimeAuthorityRepository::begin_execution_attempt(
                    &mut tx,
                    id,
                    actor.worker,
                    attempt,
                )
                .await?;
            }
            (
                execution,
                LaunchTarget::Execution {
                    execution: id,
                    worker: actor.worker,
                    worker_instance: actor.instance,
                    attempt,
                },
                owner,
            )
        };
        let description = Self::profile(&mut tx, &profile).await?;
        // This immutable CP profile, never a request Host/X-Forwarded header or
        // a caller-supplied origin, selects where action API credentials go.
        let http_policy = match profile.capabilities.get("development_plain_http") {
            None | Some(serde_json::Value::Bool(false)) => {
                crate::workload_launch_io::LaunchHttpPolicy::TlsOrLoopback
            }
            Some(serde_json::Value::Bool(true)) => {
                crate::workload_launch_io::LaunchHttpPolicy::DevelopmentPlainHttp
            }
            _ => return Err(denied()),
        };
        let api_origin = profile
            .capabilities
            .get("api_origin")
            .and_then(|value| value.as_str())
            .and_then(|value| reqwest::Url::parse(value).ok())
            .ok_or_else(denied)?;
        if !http_policy.allows_origin(&api_origin)
            || !(api_origin.scheme() == "https"
                || matches!(
                    http_policy,
                    crate::workload_launch_io::LaunchHttpPolicy::DevelopmentPlainHttp
                ))
            || api_origin.host_str().is_none()
            || api_origin.path() != "/"
            || !api_origin.username().is_empty()
            || api_origin.password().is_some()
            || api_origin.query().is_some()
            || api_origin.fragment().is_some()
        {
            return Err(denied());
        }
        let artifact_limits: crate::workload_artifact::ArtifactLimits = profile
            .capabilities
            .get("artifact_limits")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()?
            .unwrap_or_default();
        let admission = WorkloadLaunchRepository::reserve_action_admission_in(
            &mut tx,
            &target,
            owner,
            profile.image,
            artifact_limits,
        )
        .await?;
        if actor.expires_at <= Self::now(&mut tx).await?.timestamp() {
            return Err(denied());
        }
        let source = ClaimedActionSource {
            execution,
            target,
            owner,
            admission,
            image: profile.image,
            profile: description,
            api_origin,
            http_policy,
        };
        tx.commit().await?;
        if actor.expires_at <= Utc::now().timestamp() {
            return Err(denied());
        }
        Ok(Some(source))
    }
    async fn action_owner(
        tx: &mut Transaction<'_, Postgres>,
        execution: i64,
        recorded: RecordedConsumerReadiness,
    ) -> Result<i64> {
        match ExecutionAdmissionRepository::selected_authority(tx, execution).await? {
            ExistingExecutionAuthority::Delegated => {
                let selected = DelegatedExecutionRepository::validate(tx, execution).await?;
                if !selected.explicit {
                    return Err(denied());
                }
                selected.owner.ok_or_else(denied)
            }
            ExistingExecutionAuthority::Recorded => {
                recorded.require_ready()?;
                let (admission, fence) = ExecutionAdmissionRepository::require_with_fence(
                    tx,
                    RuntimeOwner::Execution(execution),
                )
                .await?;
                fence.ensure_fresh_on(tx).await?;
                Ok(admission.authority.owner.sponsor_identity_id)
            }
        }
    }
}
