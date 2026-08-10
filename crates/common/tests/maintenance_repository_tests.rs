use attune_common::repositories::maintenance::MaintenanceRepository;
use serde_json::json;
use sqlx::PgPool;

mod helpers;
use helpers::create_test_pool;

async fn setup_db() -> attune_common::test_database::TestDatabase {
    create_test_pool()
        .await
        .expect("Failed to create test pool")
}

async fn insert_execution(pool: &PgPool, status: &str, age_seconds: i64) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO execution (action_ref, config, status, created, updated)
         VALUES ($1, $2, $3::execution_status_enum, NOW() - ($4 * INTERVAL '1 second'), NOW() - ($4 * INTERVAL '1 second'))
         RETURNING id",
    )
    .bind(format!("test.reschedule_{}", status))
    .bind(json!({"hello": "world"}))
    .bind(status)
    .bind(age_seconds)
    .fetch_one(pool)
    .await
    .expect("insert execution")
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn requested_execution_is_marked_before_reschedule() {
    let pool = setup_db().await;
    let execution_id = insert_execution(&pool, "requested", 600).await;

    let candidates =
        MaintenanceRepository::find_requested_executions_for_reschedule(&pool, 300, 3, 10)
            .await
            .expect("find candidates");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].execution_id, execution_id);

    let attempt = MaintenanceRepository::mark_execution_reschedule_attempt(
        &pool,
        execution_id,
        "test",
        "unit test",
        3,
        300,
        false,
    )
    .await
    .expect("mark attempt")
    .expect("eligible attempt");

    assert_eq!(attempt.execution_id, execution_id);
    assert_eq!(attempt.attempt_count, 1);
    assert_eq!(attempt.last_source.as_deref(), Some("test"));

    let candidates =
        MaintenanceRepository::find_requested_executions_for_reschedule(&pool, 300, 3, 10)
            .await
            .expect("find candidates after mark");
    assert!(candidates.is_empty());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn reschedule_respects_max_attempts() {
    let pool = setup_db().await;
    let execution_id = insert_execution(&pool, "requested", 600).await;

    sqlx::query(
        "INSERT INTO execution_reschedule_state (
             execution_id, attempt_count, last_attempt_at, last_source, last_reason
         )
         VALUES ($1, 3, NOW() - INTERVAL '10 minutes', 'test', 'exhausted')",
    )
    .bind(execution_id)
    .execute(&pool)
    .await
    .expect("insert reschedule state");

    let candidates =
        MaintenanceRepository::find_requested_executions_for_reschedule(&pool, 300, 3, 10)
            .await
            .expect("find candidates");
    assert!(candidates.is_empty());

    let attempt = MaintenanceRepository::mark_execution_reschedule_attempt(
        &pool,
        execution_id,
        "test",
        "unit test",
        3,
        300,
        true,
    )
    .await
    .expect("mark attempt");
    assert!(attempt.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn terminal_execution_cannot_be_marked_for_reschedule() {
    let pool = setup_db().await;
    let execution_id = insert_execution(&pool, "completed", 600).await;

    let attempt = MaintenanceRepository::mark_execution_reschedule_attempt(
        &pool,
        execution_id,
        "test",
        "unit test",
        3,
        300,
        true,
    )
    .await
    .expect("mark attempt");
    assert!(attempt.is_none());
}
