//! Integration tests for durable sensor workload ownership and fencing.

mod helpers;

use attune_common::{
    models::{
        enums::{WorkerStatus, WorkerType},
        Sensor, SensorWorkloadFence, SensorWorkloadLease, Worker,
    },
    repositories::{
        rule::{CreateRuleInput, RuleRepository},
        runtime::{CreateWorkerInput, WorkerRepository},
        sensor_workload::{
            AcquireSensorWorkloadInput, AcquireSensorWorkloadOutcome, SensorWorkloadLeaseInput,
            SensorWorkloadRepository, DEFAULT_SENSOR_WORKLOAD_KEY,
        },
        sensor_workload_admission::{
            AcquireEligibleSensorWorkloadOutcome, RenewEligibleSensorWorkloadOutcome,
            SensorWorkloadAdmissionRepository,
        },
        Create,
    },
};
use helpers::{
    create_test_pool, ActionFixture, PackFixture, RuntimeFixture, SensorFixture, TriggerFixture,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

const LEASE_SECONDS: i64 = 300;

async fn setup_fixture() -> (
    attune_common::test_database::TestDatabase,
    Sensor,
    Worker,
    Worker,
) {
    let pool = create_test_pool().await.expect("test database");
    let pack = PackFixture::new_unique("sensor_workload")
        .create(&pool)
        .await
        .expect("pack");
    let runtime =
        RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "sensor_runtime")
            .create(&pool)
            .await
            .expect("runtime");
    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "leased_sensor",
    )
    .create(&pool)
    .await
    .expect("sensor");

    let worker_a = create_worker(
        &pool,
        format!("{}-sensor-worker-a", pack.r#ref),
        &runtime.name,
    )
    .await;
    let worker_b = create_worker(
        &pool,
        format!("{}-sensor-worker-b", pack.r#ref),
        &runtime.name,
    )
    .await;

    (pool, sensor, worker_a, worker_b)
}

async fn create_worker(pool: &PgPool, name: String, runtime_name: &str) -> Worker {
    let worker = WorkerRepository::create(
        pool,
        CreateWorkerInput {
            name,
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: Some(json!({"runtimes": [runtime_name]})),
            meta: None,
        },
    )
    .await
    .expect("worker");
    sqlx::query(
        "UPDATE worker SET worker_role = 'sensor', last_heartbeat = clock_timestamp() WHERE id = $1",
    )
    .bind(worker.id)
    .execute(pool)
    .await
    .expect("make live sensor worker");
    worker
}

async fn create_active_rule(pool: &PgPool, sensor: &Sensor) -> i64 {
    let pack_id = sensor.pack.expect("sensor pack");
    let pack_ref = sensor.pack_ref.clone().expect("sensor pack ref");
    let trigger =
        TriggerFixture::new_unique(Some(pack_id), Some(pack_ref.clone()), "active_trigger")
            .create(pool)
            .await
            .expect("trigger");
    sqlx::query("UPDATE trigger SET sensor = $1, sensor_ref = $2 WHERE id = $3")
        .bind(sensor.id)
        .bind(&sensor.r#ref)
        .bind(trigger.id)
        .execute(pool)
        .await
        .expect("associate trigger with sensor");
    let action = ActionFixture::new_unique(pack_id, &pack_ref, "active_action")
        .create(pool)
        .await
        .expect("action");
    RuleRepository::create(
        pool,
        CreateRuleInput {
            r#ref: format!("{}.active_rule_{}", pack_ref, Uuid::new_v4().simple()),
            pack: pack_id,
            pack_ref,
            label: "Active rule".to_string(),
            description: None,
            action: action.id,
            action_ref: action.r#ref,
            trigger: trigger.id,
            trigger_ref: trigger.r#ref,
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .expect("rule")
    .id
}

fn acquire_input(
    sensor_id: i64,
    worker_id: i64,
    worker_instance: Uuid,
) -> AcquireSensorWorkloadInput {
    AcquireSensorWorkloadInput {
        sensor_id,
        worker_id,
        worker_instance,
        lease_seconds: LEASE_SECONDS,
    }
}

async fn acquire(
    pool: &PgPool,
    sensor_id: i64,
    worker_id: i64,
    worker_instance: Uuid,
) -> SensorWorkloadLease {
    match SensorWorkloadRepository::acquire_or_renew(
        pool,
        acquire_input(sensor_id, worker_id, worker_instance),
    )
    .await
    .expect("acquire workload")
    {
        AcquireSensorWorkloadOutcome::Acquired(workload) => workload,
        AcquireSensorWorkloadOutcome::HeldByOther(_) => panic!("workload held by another worker"),
    }
}

fn lease_fence(lease: &SensorWorkloadLease) -> SensorWorkloadFence {
    SensorWorkloadFence {
        workload_id: lease.workload_id,
        worker_id: lease.worker_id,
        worker_instance: lease.worker_instance,
        generation: lease.generation,
    }
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn default_workload_is_idempotent() {
    let (pool, sensor, _, _) = setup_fixture().await;

    let first = SensorWorkloadRepository::ensure_default_for_sensor(&pool, sensor.id)
        .await
        .expect("first default workload");
    let second = SensorWorkloadRepository::ensure_default_for_sensor(&pool, sensor.id)
        .await
        .expect("second default workload");

    assert_eq!(first.id, second.id);
    assert_eq!(first.sensor, sensor.id);
    assert_eq!(first.workload_key, DEFAULT_SENSOR_WORKLOAD_KEY);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sensor_workload WHERE sensor = $1 AND workload_key = $2",
    )
    .bind(sensor.id)
    .bind(DEFAULT_SENSOR_WORKLOAD_KEY)
    .fetch_one(&*pool)
    .await
    .expect("count default workloads");
    assert_eq!(count, 1);
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn eligibility_aware_acquisition_checks_active_workload_and_starts_generation() {
    let (pool, sensor, worker, _) = setup_fixture().await;
    let worker_instance = Uuid::new_v4();

    let no_rule = SensorWorkloadAdmissionRepository::acquire(
        &pool,
        acquire_input(sensor.id, worker.id, worker_instance),
    )
    .await
    .expect("check workload without an active rule");
    assert!(matches!(
        no_rule,
        AcquireEligibleSensorWorkloadOutcome::Ineligible
    ));

    create_active_rule(&pool, &sensor).await;
    sqlx::query("UPDATE sensor SET enabled = FALSE WHERE id = $1")
        .bind(sensor.id)
        .execute(&*pool)
        .await
        .expect("disable sensor");
    let disabled = SensorWorkloadAdmissionRepository::acquire(
        &pool,
        acquire_input(sensor.id, worker.id, worker_instance),
    )
    .await
    .expect("check disabled sensor");
    assert!(matches!(
        disabled,
        AcquireEligibleSensorWorkloadOutcome::Ineligible
    ));

    sqlx::query("UPDATE sensor SET enabled = TRUE WHERE id = $1")
        .bind(sensor.id)
        .execute(&*pool)
        .await
        .expect("enable sensor");
    sqlx::query("UPDATE worker SET cordoned = TRUE WHERE id = $1")
        .bind(worker.id)
        .execute(&*pool)
        .await
        .expect("cordon worker");
    let cordoned = SensorWorkloadAdmissionRepository::acquire(
        &pool,
        acquire_input(sensor.id, worker.id, worker_instance),
    )
    .await
    .expect("check cordoned worker");
    assert!(matches!(
        cordoned,
        AcquireEligibleSensorWorkloadOutcome::Ineligible
    ));

    sqlx::query("UPDATE worker SET cordoned = FALSE WHERE id = $1")
        .bind(worker.id)
        .execute(&*pool)
        .await
        .expect("uncordon worker");
    let acquired = SensorWorkloadAdmissionRepository::acquire(
        &pool,
        acquire_input(sensor.id, worker.id, worker_instance),
    )
    .await
    .expect("acquire eligible workload");
    let AcquireEligibleSensorWorkloadOutcome::Acquired(acquired) = acquired else {
        panic!("eligible workload was not acquired");
    };
    assert_eq!(acquired.generation, 1);
    let assignment = SensorWorkloadRepository::find_assignment(&pool, acquired.workload_id)
        .await
        .expect("load assignment")
        .expect("assignment");
    assert_eq!(assignment.generation, acquired.generation);
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn eligibility_aware_acquisition_enforces_worker_sensor_capacity() {
    let (pool, first_sensor, worker, _) = setup_fixture().await;
    let pack_id = first_sensor.pack.expect("sensor pack");
    let pack_ref = first_sensor.pack_ref.clone().expect("sensor pack ref");
    let second_sensor = SensorFixture::new_unique(
        Some(pack_id),
        Some(pack_ref),
        first_sensor.runtime,
        first_sensor.runtime_ref.clone(),
        "second_leased_sensor",
    )
    .create(&pool)
    .await
    .expect("second sensor");
    create_active_rule(&pool, &first_sensor).await;
    create_active_rule(&pool, &second_sensor).await;
    sqlx::query(
        "UPDATE worker SET capabilities = capabilities || \
         '{\"max_concurrent_sensors\": 1}'::jsonb WHERE id = $1",
    )
    .bind(worker.id)
    .execute(&*pool)
    .await
    .expect("set worker capacity");

    let first = SensorWorkloadAdmissionRepository::acquire(
        &pool,
        acquire_input(first_sensor.id, worker.id, Uuid::new_v4()),
    )
    .await
    .expect("acquire first workload");
    assert!(matches!(
        first,
        AcquireEligibleSensorWorkloadOutcome::Acquired(_)
    ));

    let second = SensorWorkloadAdmissionRepository::acquire(
        &pool,
        acquire_input(second_sensor.id, worker.id, Uuid::new_v4()),
    )
    .await
    .expect("check second workload");
    assert!(matches!(
        second,
        AcquireEligibleSensorWorkloadOutcome::Ineligible
    ));
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn concurrent_eligibility_aware_acquisition_has_one_owner() {
    let (pool, sensor, worker_a, worker_b) = setup_fixture().await;
    create_active_rule(&pool, &sensor).await;

    let (outcome_a, outcome_b) = tokio::join!(
        SensorWorkloadAdmissionRepository::acquire(
            &pool,
            acquire_input(sensor.id, worker_a.id, Uuid::new_v4()),
        ),
        SensorWorkloadAdmissionRepository::acquire(
            &pool,
            acquire_input(sensor.id, worker_b.id, Uuid::new_v4()),
        )
    );
    let outcomes = [outcome_a.expect("worker A"), outcome_b.expect("worker B")];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, AcquireEligibleSensorWorkloadOutcome::Acquired(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(
                outcome,
                AcquireEligibleSensorWorkloadOutcome::HeldByOther(_)
            ))
            .count(),
        1
    );
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn eligibility_aware_renewal_stops_on_ineligibility_or_ownership_loss() {
    let (pool, sensor, worker, _) = setup_fixture().await;
    let rule_id = create_active_rule(&pool, &sensor).await;
    let acquired = SensorWorkloadAdmissionRepository::acquire(
        &pool,
        acquire_input(sensor.id, worker.id, Uuid::new_v4()),
    )
    .await
    .expect("acquire workload");
    let AcquireEligibleSensorWorkloadOutcome::Acquired(acquired) = acquired else {
        panic!("eligible workload was not acquired");
    };
    let renewal_input = SensorWorkloadLeaseInput {
        fence: acquired.fence(),
        lease_seconds: LEASE_SECONDS * 2,
    };

    sqlx::query("UPDATE rule SET enabled = FALSE WHERE id = $1")
        .bind(rule_id)
        .execute(&*pool)
        .await
        .expect("disable active rule");
    let ineligible = SensorWorkloadAdmissionRepository::renew(&pool, sensor.id, renewal_input)
        .await
        .expect("renew ineligible workload");
    assert_eq!(ineligible, RenewEligibleSensorWorkloadOutcome::Ineligible);

    sqlx::query("UPDATE rule SET enabled = TRUE WHERE id = $1")
        .bind(rule_id)
        .execute(&*pool)
        .await
        .expect("enable active rule");
    assert!(SensorWorkloadRepository::release(&pool, acquired.fence())
        .await
        .expect("release workload"));
    let lost = SensorWorkloadAdmissionRepository::renew(&pool, sensor.id, renewal_input)
        .await
        .expect("renew released workload");
    assert_eq!(lost, RenewEligibleSensorWorkloadOutcome::OwnershipLost);
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn concurrent_acquisition_has_exactly_one_owner() {
    let (pool, sensor, worker_a, worker_b) = setup_fixture().await;
    let instance_a = Uuid::new_v4();
    let instance_b = Uuid::new_v4();

    let (outcome_a, outcome_b) = tokio::join!(
        SensorWorkloadRepository::acquire_or_renew(
            &pool,
            acquire_input(sensor.id, worker_a.id, instance_a),
        ),
        SensorWorkloadRepository::acquire_or_renew(
            &pool,
            acquire_input(sensor.id, worker_b.id, instance_b),
        )
    );
    let outcome_a = outcome_a.expect("worker A acquisition");
    let outcome_b = outcome_b.expect("worker B acquisition");

    let (owner, blocked) = match (outcome_a, outcome_b) {
        (
            AcquireSensorWorkloadOutcome::Acquired(owner),
            AcquireSensorWorkloadOutcome::HeldByOther(blocked),
        )
        | (
            AcquireSensorWorkloadOutcome::HeldByOther(blocked),
            AcquireSensorWorkloadOutcome::Acquired(owner),
        ) => (owner, blocked),
        _ => panic!("expected one acquisition and one blocked worker"),
    };

    assert_eq!(blocked.workload, owner.workload_id);
    assert_eq!(blocked.worker, Some(owner.worker_id));
    assert_eq!(blocked.worker_instance, Some(owner.worker_instance));
    assert_eq!(blocked.generation, owner.generation);
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn begin_process_increments_generation() {
    let (pool, sensor, worker, _) = setup_fixture().await;
    let acquired = acquire(&pool, sensor.id, worker.id, Uuid::new_v4()).await;

    let started = SensorWorkloadRepository::begin_process(&pool, acquired.clone())
        .await
        .expect("begin process")
        .expect("current fence should begin process");

    assert_eq!(started.workload_id, acquired.workload_id);
    assert_eq!(started.worker_id, acquired.worker_id);
    assert_eq!(started.generation, acquired.generation + 1);
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn renewal_preserves_generation() {
    let (pool, sensor, worker, _) = setup_fixture().await;
    let acquired = acquire(&pool, sensor.id, worker.id, Uuid::new_v4()).await;
    let started = SensorWorkloadRepository::begin_process(&pool, acquired)
        .await
        .expect("begin process")
        .expect("current lease should begin process");

    let renewed = SensorWorkloadRepository::renew(
        &pool,
        SensorWorkloadLeaseInput {
            fence: started.fence(),
            lease_seconds: LEASE_SECONDS * 2,
        },
    )
    .await
    .expect("renew workload")
    .expect("current fence should renew");

    assert_eq!(renewed.fence(), started.fence());
    assert!(renewed.lease_expires_at >= started.lease_expires_at);
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn stale_generation_is_rejected() {
    let (pool, sensor, worker, _) = setup_fixture().await;
    let acquired = acquire(&pool, sensor.id, worker.id, Uuid::new_v4()).await;
    let stale_fence = lease_fence(&acquired);
    let current = SensorWorkloadRepository::begin_process(&pool, acquired)
        .await
        .expect("begin process")
        .expect("current fence should begin process");

    let renewed = SensorWorkloadRepository::renew(
        &pool,
        SensorWorkloadLeaseInput {
            fence: stale_fence,
            lease_seconds: LEASE_SECONDS,
        },
    )
    .await
    .expect("reject stale renewal");
    assert!(renewed.is_none());
    assert!(!SensorWorkloadRepository::release(&pool, stale_fence)
        .await
        .expect("reject stale release"));

    let mut tx = pool.begin().await.expect("database transaction");
    assert!(
        !SensorWorkloadRepository::lock_current_fence(&mut tx, sensor.id, stale_fence,)
            .await
            .expect("check stale fence")
    );
    assert!(
        SensorWorkloadRepository::lock_current_fence(&mut tx, sensor.id, current.fence(),)
            .await
            .expect("check current fence")
    );
    tx.rollback().await.expect("rollback fence transaction");
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn released_workload_can_be_reacquired() {
    let (pool, sensor, worker_a, worker_b) = setup_fixture().await;
    let acquired = acquire(&pool, sensor.id, worker_a.id, Uuid::new_v4()).await;
    let started = SensorWorkloadRepository::begin_process(&pool, acquired)
        .await
        .expect("begin process")
        .expect("current fence should begin process");

    assert!(SensorWorkloadRepository::release(&pool, started.fence())
        .await
        .expect("release workload"));
    let released = SensorWorkloadRepository::find_assignment(&pool, started.workload_id)
        .await
        .expect("find released assignment")
        .expect("assignment remains after release");
    assert!(released.worker.is_none());
    assert!(released.worker_instance.is_none());
    assert!(released.lease_expires_at.is_none());

    let reacquired = acquire(&pool, sensor.id, worker_b.id, Uuid::new_v4()).await;
    assert_eq!(reacquired.workload_id, started.workload_id);
    assert_eq!(reacquired.worker_id, worker_b.id);
    assert_eq!(reacquired.generation, started.generation);
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn expired_lease_can_be_taken_over() {
    let (pool, sensor, worker_a, worker_b) = setup_fixture().await;
    let acquired = acquire(&pool, sensor.id, worker_a.id, Uuid::new_v4()).await;
    let old_fence = lease_fence(&acquired);

    sqlx::query(
        "UPDATE sensor_workload_assignment \
         SET lease_expires_at = NOW() - INTERVAL '1 second' WHERE workload = $1",
    )
    .bind(acquired.workload_id)
    .execute(&*pool)
    .await
    .expect("expire lease");

    let instance_b = Uuid::new_v4();
    let takeover = acquire(&pool, sensor.id, worker_b.id, instance_b).await;
    assert_eq!(takeover.workload_id, acquired.workload_id);
    assert_eq!(takeover.worker_id, worker_b.id);
    assert_eq!(takeover.worker_instance, instance_b);
    assert_eq!(takeover.generation, acquired.generation);

    assert!(SensorWorkloadRepository::renew(
        &pool,
        SensorWorkloadLeaseInput {
            fence: old_fence,
            lease_seconds: LEASE_SECONDS,
        },
    )
    .await
    .expect("reject previous owner")
    .is_none());
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn default_workload_membership_requires_enabled_owned_targets() {
    let (pool, sensor, _, _) = setup_fixture().await;
    let pack_id = sensor.pack.expect("sensor pack");
    let pack_ref = sensor.pack_ref.clone().expect("sensor pack ref");
    let trigger =
        TriggerFixture::new_unique(Some(pack_id), Some(pack_ref.clone()), "workload_trigger")
            .create(&pool)
            .await
            .expect("trigger");
    sqlx::query("UPDATE trigger SET sensor = $1, sensor_ref = $2 WHERE id = $3")
        .bind(sensor.id)
        .bind(&sensor.r#ref)
        .bind(trigger.id)
        .execute(&*pool)
        .await
        .expect("associate trigger with sensor");
    let action = ActionFixture::new_unique(pack_id, &pack_ref, "workload_action")
        .create(&pool)
        .await
        .expect("action");
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.workload_rule_{}", pack_ref, Uuid::new_v4().simple()),
            pack: pack_id,
            pack_ref,
            label: "Workload rule".to_string(),
            description: None,
            action: action.id,
            action_ref: action.r#ref,
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .expect("rule");
    let workload = SensorWorkloadRepository::ensure_default_for_sensor(&pool, sensor.id)
        .await
        .expect("default workload");

    let mut tx = pool.begin().await.expect("membership transaction");
    assert!(SensorWorkloadRepository::trigger_is_member(
        &mut tx,
        sensor.id,
        workload.id,
        trigger.id,
    )
    .await
    .expect("trigger membership"));
    assert!(SensorWorkloadRepository::rule_is_member(
        &mut tx,
        sensor.id,
        workload.id,
        rule.id,
        trigger.id,
    )
    .await
    .expect("rule membership"));
    tx.rollback()
        .await
        .expect("rollback membership transaction");

    sqlx::query("UPDATE rule SET enabled = FALSE WHERE id = $1")
        .bind(rule.id)
        .execute(&*pool)
        .await
        .expect("disable rule");
    let mut tx = pool.begin().await.expect("disabled membership transaction");
    assert!(!SensorWorkloadRepository::rule_is_member(
        &mut tx,
        sensor.id,
        workload.id,
        rule.id,
        trigger.id,
    )
    .await
    .expect("disabled rule membership"));
    tx.rollback().await.expect("rollback disabled transaction");
}
