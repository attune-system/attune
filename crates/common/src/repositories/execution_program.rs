//! Program metadata is not authority. Persist it under the existing producer
//! attempt and assigned worker before starting a CLI process.
use crate::{action_cli::SelectedProgram, Error, Result};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

pub struct ExecutionProgramRepository;
impl ExecutionProgramRepository {
    pub async fn record(
        pool: &PgPool,
        execution: i64,
        worker: i64,
        release: i64,
        selection: &SelectedProgram,
    ) -> Result<()> {
        let mut transaction = pool.begin().await?;
        let selection = serde_json::to_value(selection)
            .map_err(|_| Error::validation("Invalid program metadata"))?;
        super::execution_runtime_admission::ExecutionAdmissionRepository::existing_authority(
            &mut transaction,
            execution,
        )
        .await?;
        let attempt: Option<Uuid> = sqlx::query_scalar(
            "SELECT CASE WHEN s.mode='delegated' THEN d.attempt WHEN s.mode='recorded' AND a.id=s.admission AND a.owner_kind='execution' AND a.owner_id=e.id AND a.revoked_at IS NULL THEN f.attempt END FROM execution e JOIN execution_authority_selection s ON s.execution=e.id \
             LEFT JOIN delegated_execution_attempt d ON d.execution=e.id AND d.worker=e.worker AND NOT d.closed AND d.producer_kind='worker' \
             LEFT JOIN runtime_authority_execution_fence f ON f.execution=e.id LEFT JOIN runtime_authority_admission a ON a.id=f.admission \
             WHERE e.id=$1 AND e.worker=$2 AND e.status='running' FOR SHARE OF e"
        ).bind(execution).bind(worker).fetch_optional(&mut *transaction).await?.flatten();
        let attempt = attempt.ok_or_else(|| {
            Error::PermissionDenied("Program launch attempt is unavailable".into())
        })?;
        Self::record_inspected_on(
            &mut transaction,
            execution,
            worker,
            attempt,
            release,
            selection,
            None,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Caller holds the existing source and native inspection effect fences.
    pub(crate) async fn record_inspected_on(
        connection: &mut PgConnection,
        execution: i64,
        worker: i64,
        attempt: Uuid,
        release: i64,
        selection: serde_json::Value,
        native_probe: Option<Uuid>,
    ) -> Result<()> {
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO execution_program_selection(execution,attempt,worker,release,selection,native_probe) VALUES($1,$2,$3,$4,$5,$6) \
             ON CONFLICT(execution,attempt) DO UPDATE SET selection=EXCLUDED.selection \
             WHERE execution_program_selection.selection=EXCLUDED.selection AND execution_program_selection.worker=EXCLUDED.worker \
             AND execution_program_selection.release=EXCLUDED.release AND execution_program_selection.native_probe IS NOT DISTINCT FROM EXCLUDED.native_probe RETURNING id"
        ).bind(execution).bind(attempt).bind(worker).bind(release).bind(selection).bind(native_probe)
            .fetch_optional(connection).await?;
        if id.is_none() {
            return Err(Error::PermissionDenied(
                "Program launch metadata changed within an attempt".into(),
            ));
        }
        Ok(())
    }
}
