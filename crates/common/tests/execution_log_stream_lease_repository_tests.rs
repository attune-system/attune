mod helpers;

use attune_common::repositories::execution_log_stream_lease::{
    ExecutionLogStreamAdmission, ExecutionLogStreamLeaseRepository,
};
use helpers::create_test_pool;

fn acquired_lease(
    admission: ExecutionLogStreamAdmission,
) -> (uuid::Uuid, chrono::DateTime<chrono::Utc>) {
    match admission {
        ExecutionLogStreamAdmission::Acquired {
            lease_id,
            expires_at,
        } => (lease_id, expires_at),
        other => panic!("expected acquired lease, got {other:?}"),
    }
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn concurrent_replicas_enforce_cluster_global_limit() {
    let database = create_test_pool().await.expect("test database");
    let first_replica = database.pool().clone();
    let second_replica = database.pool().clone();

    let (first, second, third) = tokio::join!(
        ExecutionLogStreamLeaseRepository::acquire(&first_replica, 1, 2, 2, 30),
        ExecutionLogStreamLeaseRepository::acquire(&second_replica, 2, 2, 2, 30),
        ExecutionLogStreamLeaseRepository::acquire(&first_replica, 3, 2, 2, 30),
    );
    let admissions = [first.unwrap(), second.unwrap(), third.unwrap()];

    assert_eq!(
        admissions
            .iter()
            .filter(|result| matches!(result, ExecutionLogStreamAdmission::Acquired { .. }))
            .count(),
        2
    );
    assert_eq!(
        admissions
            .iter()
            .filter(|result| matches!(result, ExecutionLogStreamAdmission::GlobalLimit))
            .count(),
        1
    );
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn concurrent_replicas_scope_limit_to_signed_identity() {
    let database = create_test_pool().await.expect("test database");
    let first_replica = database.pool().clone();
    let second_replica = database.pool().clone();

    let (first, second, other_identity) = tokio::join!(
        ExecutionLogStreamLeaseRepository::acquire(&first_replica, 7, 3, 1, 30),
        ExecutionLogStreamLeaseRepository::acquire(&second_replica, 7, 3, 1, 30),
        ExecutionLogStreamLeaseRepository::acquire(&second_replica, 8, 3, 1, 30),
    );
    let same_identity = [first.unwrap(), second.unwrap()];

    assert_eq!(
        same_identity
            .iter()
            .filter(|result| matches!(result, ExecutionLogStreamAdmission::Acquired { .. }))
            .count(),
        1
    );
    assert_eq!(
        same_identity
            .iter()
            .filter(|result| matches!(result, ExecutionLogStreamAdmission::IdentityLimit))
            .count(),
        1
    );
    assert!(matches!(
        other_identity.unwrap(),
        ExecutionLogStreamAdmission::Acquired { .. }
    ));
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn expired_lease_is_recovered_after_replica_crash() {
    let database = create_test_pool().await.expect("test database");
    let (abandoned_lease, initial_expires_at) = acquired_lease(
        ExecutionLogStreamLeaseRepository::acquire(&database, 1, 1, 1, 300)
            .await
            .unwrap(),
    );
    assert!(matches!(
        ExecutionLogStreamLeaseRepository::acquire(&database, 2, 1, 1, 30)
            .await
            .unwrap(),
        ExecutionLogStreamAdmission::GlobalLimit
    ));

    let expired: bool = sqlx::query_scalar(
        "UPDATE execution_log_stream_lease \
         SET expires_at = clock_timestamp() - INTERVAL '1 second' \
         WHERE id = $1 \
         RETURNING expires_at <= clock_timestamp()",
    )
    .bind(abandoned_lease)
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert!(
        expired,
        "database did not observe the abandoned lease as expired"
    );

    let recovered = ExecutionLogStreamLeaseRepository::acquire(&database, 2, 1, 1, 300)
        .await
        .unwrap();
    assert!(
        matches!(recovered, ExecutionLogStreamAdmission::Acquired { .. }),
        "expired lease was not recovered: initial_expires_at={initial_expires_at}, expired={expired}, admission={recovered:?}"
    );
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn release_and_renewal_control_lease_lifetime() {
    let database = create_test_pool().await.expect("test database");
    let (lease, initial_expires_at) = acquired_lease(
        ExecutionLogStreamLeaseRepository::acquire(&database, 1, 1, 1, 300)
            .await
            .unwrap(),
    );

    assert!(
        ExecutionLogStreamLeaseRepository::renew(&database, lease, 600)
            .await
            .unwrap()
    );
    let (renewed_expires_at, database_now): (
        chrono::DateTime<chrono::Utc>,
        chrono::DateTime<chrono::Utc>,
    ) = sqlx::query_as(
        "SELECT expires_at, clock_timestamp() \
         FROM execution_log_stream_lease \
         WHERE id = $1",
    )
    .bind(lease)
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert!(
        renewed_expires_at > initial_expires_at,
        "renewal did not extend lease: initial={initial_expires_at}, renewed={renewed_expires_at}"
    );
    assert!(
        renewed_expires_at > database_now,
        "renewed lease is not active: database_now={database_now}, expires_at={renewed_expires_at}"
    );
    assert_eq!(
        ExecutionLogStreamLeaseRepository::active_count(&database)
            .await
            .unwrap(),
        1
    );
    assert!(ExecutionLogStreamLeaseRepository::release(&database, lease)
        .await
        .unwrap());
    assert_eq!(
        ExecutionLogStreamLeaseRepository::active_count(&database)
            .await
            .unwrap(),
        0
    );
}
