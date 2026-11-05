use attune_common::{
    config::Config,
    models::{
        enums::{WorkerStatus, WorkerType},
        ExecutionStatus,
    },
    repositories::{
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        runtime::{CreateWorkerInput, WorkerRepository},
        Create, FindById, Update,
    },
    test_database::TestDatabase,
};
use serde_json::json;

async fn fixture() -> (TestDatabase, i64) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config.test.yaml");
    let config = Config::load_from_file(path.to_str().unwrap()).unwrap();
    let db = TestDatabase::create(&config.database)
        .await
        .unwrap()
        .with_cleanup_on_drop();
    let worker = WorkerRepository::create(
        db.pool(),
        CreateWorkerInput {
            name: "transition-worker".into(),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: None,
            meta: None,
        },
    )
    .await
    .unwrap();
    (db, worker.id)
}

#[tokio::test]
async fn worker_transitions_return_the_locked_predecessor_and_reject_stale_assignment() {
    let (db, worker) = fixture().await;
    for terminal in [
        ExecutionStatus::Completed,
        ExecutionStatus::Failed,
        ExecutionStatus::Timeout,
        ExecutionStatus::Cancelled,
    ] {
        let execution = ExecutionRepository::create(
            db.pool(),
            CreateExecutionInput {
                action_ref: "test.transition".into(),
                worker: Some(worker),
                status: ExecutionStatus::Scheduled,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(ExecutionRepository::transition_worker(
            db.pool(),
            execution.id,
            Some(worker + 1),
            ExecutionStatus::Running,
            None
        )
        .await
        .is_err());
        let running = ExecutionRepository::transition_worker(
            db.pool(),
            execution.id,
            Some(worker),
            ExecutionStatus::Running,
            None,
        )
        .await
        .unwrap();
        assert_eq!(running.previous_status, ExecutionStatus::Scheduled);
        assert!(running.execution.started_at.is_some());
        assert!(
            ExecutionRepository::transition_worker(
                db.pool(),
                execution.id,
                Some(worker),
                ExecutionStatus::Running,
                None
            )
            .await
            .is_err(),
            "a duplicate delivery must not start the process twice"
        );
        ExecutionRepository::update(
            db.pool(),
            execution.id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Canceling),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(ExecutionRepository::transition_worker(
            db.pool(),
            execution.id,
            Some(worker),
            ExecutionStatus::Running,
            None
        )
        .await
        .is_err());
        let result = json!({"stdout":"[REDACTED]", "queue_ack":{"value":1}});
        let finished = ExecutionRepository::transition_worker(
            db.pool(),
            execution.id,
            Some(worker),
            terminal,
            Some(result.clone()),
        )
        .await
        .unwrap();
        assert_eq!(finished.previous_status, ExecutionStatus::Canceling);
        assert_eq!(finished.execution.status, terminal);
        assert_eq!(finished.execution.result, Some(result.clone()));
        assert_eq!(finished.execution.started_at, running.execution.started_at);
        assert!(ExecutionRepository::transition_worker(
            db.pool(),
            execution.id,
            Some(worker),
            ExecutionStatus::Failed,
            Some(json!({"overwrite":true}))
        )
        .await
        .is_err());
        assert!(ExecutionRepository::transition_worker(
            db.pool(),
            execution.id,
            Some(worker),
            ExecutionStatus::Running,
            None
        )
        .await
        .is_err());
        assert_eq!(
            ExecutionRepository::find_by_id(db.pool(), execution.id)
                .await
                .unwrap()
                .unwrap()
                .result,
            Some(result)
        );
    }
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn racing_terminal_transitions_have_one_winner() {
    let (db, worker) = fixture().await;
    let execution = ExecutionRepository::create(
        db.pool(),
        CreateExecutionInput {
            action_ref: "test.race".into(),
            worker: Some(worker),
            status: ExecutionStatus::Running,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (left, right) = tokio::join!(
        ExecutionRepository::transition_worker(
            db.pool(),
            execution.id,
            Some(worker),
            ExecutionStatus::Completed,
            Some(json!({"winner":"left"}))
        ),
        ExecutionRepository::transition_worker(
            db.pool(),
            execution.id,
            Some(worker),
            ExecutionStatus::Failed,
            Some(json!({"winner":"right"}))
        ),
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let winner = left.or(right).unwrap();
    assert_eq!(winner.previous_status, ExecutionStatus::Running);
    let stored = ExecutionRepository::find_by_id(db.pool(), execution.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, winner.execution.status);
    assert_eq!(stored.result, winner.execution.result);
    db.cleanup().await.unwrap();
}
