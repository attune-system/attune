mod helpers;

use std::time::Duration;

use attune_common::repositories::execution_log_stream_lease::{
    ExecutionLogStreamAdmission, ExecutionLogStreamLeaseRepository,
};
use helpers::create_test_pool;

fn lease_id(admission: ExecutionLogStreamAdmission) -> uuid::Uuid {
    match admission {
        ExecutionLogStreamAdmission::Acquired { lease_id, .. } => lease_id,
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
    let _abandoned = ExecutionLogStreamLeaseRepository::acquire(&database, 1, 1, 1, 1)
        .await
        .unwrap();
    assert!(matches!(
        ExecutionLogStreamLeaseRepository::acquire(&database, 2, 1, 1, 30)
            .await
            .unwrap(),
        ExecutionLogStreamAdmission::GlobalLimit
    ));

    tokio::time::sleep(Duration::from_millis(1_100)).await;
    assert!(matches!(
        ExecutionLogStreamLeaseRepository::acquire(&database, 2, 1, 1, 30)
            .await
            .unwrap(),
        ExecutionLogStreamAdmission::Acquired { .. }
    ));
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn release_and_renewal_control_lease_lifetime() {
    let database = create_test_pool().await.expect("test database");
    let lease = lease_id(
        ExecutionLogStreamLeaseRepository::acquire(&database, 1, 1, 1, 1)
            .await
            .unwrap(),
    );

    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        ExecutionLogStreamLeaseRepository::renew(&database, lease, 2)
            .await
            .unwrap()
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
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
