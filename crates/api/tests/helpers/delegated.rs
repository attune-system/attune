//! Real assigned attempt authority for middleware/stream boundary fixtures.
use attune_common::{
    models::{ExecutionStatus, WorkerStatus, WorkerType},
    repositories::{
        delegated_execution::DelegatedExecutionRepository,
        execution::{ExecutionRepository, UpdateExecutionInput},
        runtime::{CreateWorkerInput, WorkerRepository},
        Create, Update,
    },
    Result,
};
use sqlx::PgConnection;
use uuid::Uuid;

pub async fn start(conn: &mut PgConnection, execution: i64) -> Result<Uuid> {
    let worker = WorkerRepository::create(
        &mut *conn,
        CreateWorkerInput {
            name: format!("delegated-fixture-{}", Uuid::new_v4()),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: None,
            meta: None,
        },
    )
    .await?;
    ExecutionRepository::update(
        &mut *conn,
        execution,
        UpdateExecutionInput {
            worker: Some(worker.id),
            status: Some(ExecutionStatus::Scheduled),
            ..Default::default()
        },
    )
    .await?;
    let attempt = DelegatedExecutionRepository::start(conn, execution, worker.id).await?;
    ExecutionRepository::transition_worker(
        &mut *conn,
        execution,
        Some(worker.id),
        ExecutionStatus::Running,
        None,
    )
    .await?;
    Ok(attempt)
}
