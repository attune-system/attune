//! Program metadata is not authority. Persist it under the existing producer
//! attempt and assigned worker before starting a CLI process.
use crate::{action_cli::SelectedProgram, Error, Result};
use sqlx::PgPool;

pub struct ExecutionProgramRepository;
impl ExecutionProgramRepository {
    pub async fn record(
        pool: &PgPool,
        execution: i64,
        worker: i64,
        release: i64,
        selection: &SelectedProgram,
    ) -> Result<()> {
        let selection = serde_json::to_value(selection)
            .map_err(|_| Error::validation("Invalid program metadata"))?;
        let mut transaction = pool.begin().await?;
        super::execution_runtime_admission::ExecutionAdmissionRepository::existing_authority(
            &mut transaction,
            execution,
        )
        .await?;
        let attempt: Option<uuid::Uuid> = sqlx::query_scalar(
            "SELECT CASE WHEN s.mode='delegated' THEN d.attempt WHEN s.mode='recorded' AND a.id=s.admission AND a.owner_kind='execution' AND a.owner_id=e.id AND a.revoked_at IS NULL THEN f.attempt END FROM execution e JOIN execution_authority_selection s ON s.execution=e.id \
             LEFT JOIN delegated_execution_attempt d ON d.execution=e.id AND d.worker=e.worker AND NOT d.closed AND d.producer_kind='worker' \
             LEFT JOIN runtime_authority_execution_fence f ON f.execution=e.id LEFT JOIN runtime_authority_admission a ON a.id=f.admission \
             WHERE e.id=$1 AND e.worker=$2 AND e.status='running' FOR SHARE OF e"
        ).bind(execution).bind(worker).fetch_optional(&mut *transaction).await?.flatten();
        let attempt = attempt.ok_or_else(|| {
            Error::PermissionDenied("Program launch attempt is unavailable".into())
        })?;
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO execution_program_selection(execution,attempt,worker,release,selection) VALUES($1,$2,$3,$4,$5) \
             ON CONFLICT(execution,attempt) DO UPDATE SET selection=EXCLUDED.selection \
             WHERE execution_program_selection.selection=EXCLUDED.selection AND execution_program_selection.worker=EXCLUDED.worker \
             AND execution_program_selection.release=EXCLUDED.release RETURNING id"
        ).bind(execution).bind(attempt).bind(worker).bind(release).bind(selection)
            .fetch_optional(&mut *transaction).await?;
        if id.is_none() {
            return Err(Error::PermissionDenied(
                "Program launch metadata changed within an attempt".into(),
            ));
        }
        transaction.commit().await?;
        Ok(())
    }
}
