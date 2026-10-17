//! Worker issuance is opt-in only after the coordinated consumer conversion.

use attune_common::{
    auth::jwt::{generate_admitted_execution_token, JwtConfig},
    models::Execution,
    repositories::runtime_authority::RuntimeAuthorityRepository,
    Error, Result,
};
use sqlx::PgPool;
use uuid::Uuid;

/// Supplied by coordinated service startup, not pack metadata or environment.
/// Default is deliberately not ready. Do not enable individual consumers by
/// treating a denied operation as a reason to issue an older broad credential.
#[derive(Clone, Copy, Default)]
pub struct RecordedConsumerReadiness {
    pub execution_admission: bool,
    pub api_consumers: bool,
    pub artifact_disclosure: bool,
    pub runtime_read_provenance: bool,
    pub notifier_delivery: bool,
    pub worker_completion: bool,
    pub native_executor_consumers: bool,
    pub candidate_isolation: bool,
}

impl RecordedConsumerReadiness {
    pub fn require_ready(self) -> Result<()> {
        if self.execution_admission
            && self.api_consumers
            && self.artifact_disclosure
            && self.runtime_read_provenance
            && self.notifier_delivery
            && self.worker_completion
            && self.native_executor_consumers
            && self.candidate_isolation
        {
            Ok(())
        } else {
            Err(Error::PermissionDenied(
                "Recorded execution credentials require coordinated admission, consumers and completion readiness".into(),
            ))
        }
    }
}

/// The attempt is generated once by the worker for this launch. Retries of this
/// preparation reuse it; another execution must never reuse its credential.
/// The database validates worker assignment, sponsor, policy and approvals.
pub async fn mint_recorded_execution_token(
    pool: &PgPool,
    execution: &Execution,
    worker_id: i64,
    attempt: Uuid,
    ttl_seconds: i64,
    config: &JwtConfig,
    readiness: RecordedConsumerReadiness,
) -> Result<Option<String>> {
    if execution.permission_set_refs.is_empty() {
        return Ok(None);
    }
    readiness.require_ready()?;
    if ttl_seconds <= 0 {
        return Err(Error::validation(
            "Execution token lifetime must be positive",
        ));
    }
    let mut tx = pool.begin().await?;
    let admission = RuntimeAuthorityRepository::execution_admission(&mut tx, execution.id)
        .await?
        .ok_or_else(|| {
            Error::PermissionDenied("Execution runtime admission is unavailable".into())
        })?;
    if admission.authority.selected_refs != execution.permission_set_refs
        || admission.authority.binding.action_ref() != execution.action_ref
    {
        return Err(Error::PermissionDenied(
            "Execution selection does not match admission".into(),
        ));
    }
    let binding = RuntimeAuthorityRepository::begin_execution_attempt(
        &mut tx,
        execution.id,
        worker_id,
        attempt,
    )
    .await?;
    // Validate pinned standard owner bindings too, before giving arbitrary code
    // a token that the server would immediately reject.
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &binding).await?;
    let token = generate_admitted_execution_token(
        &binding,
        &admission.authority.selected_refs,
        config,
        Some(ttl_seconds),
    )
    .map_err(|_| Error::PermissionDenied("Unable to sign execution credential".into()))?;
    authority.ensure_fresh_on(&mut tx).await?;
    tx.commit().await?;
    Ok(Some(token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_conversion_never_enables_worker_issuance() {
        assert!(RecordedConsumerReadiness::default()
            .require_ready()
            .is_err());
        let ready = RecordedConsumerReadiness {
            execution_admission: true,
            api_consumers: true,
            artifact_disclosure: true,
            runtime_read_provenance: true,
            notifier_delivery: true,
            worker_completion: true,
            native_executor_consumers: true,
            candidate_isolation: true,
        };
        assert!(ready.require_ready().is_ok());
        assert!(RecordedConsumerReadiness {
            runtime_read_provenance: false,
            ..ready
        }
        .require_ready()
        .is_err());
        assert!(RecordedConsumerReadiness {
            worker_completion: false,
            ..ready
        }
        .require_ready()
        .is_err());
        assert!(RecordedConsumerReadiness {
            api_consumers: false,
            ..ready
        }
        .require_ready()
        .is_err());
    }
}
