mod helpers;

use attune_common::{
    models::{
        enums::{WorkerStatus, WorkerType},
        Action, Sensor, Worker,
    },
    repositories::{
        rule::{CreateRuleInput, RuleRepository, RuleSensorPlacementInput},
        runtime::{CreateWorkerInput, WorkerRepository},
        Create, SensorAdmissionRepository,
    },
};
use helpers::{
    create_test_pool, ActionFixture, PackFixture, RuntimeFixture, SensorFixture, TriggerFixture,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

async fn create_sensor_worker(pool: &PgPool, runtime_name: &str) -> Worker {
    let worker = WorkerRepository::create(
        pool,
        CreateWorkerInput {
            name: format!("sensor-admission-{}", Uuid::new_v4()),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: Some(json!({
                "labels": {"site": "edge"},
                "runtimes": [runtime_name]
            })),
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
    .expect("make worker live");
    worker
}

async fn create_rule_for_sensor(
    pool: &PgPool,
    sensor: &Sensor,
    action: &Action,
    trigger_enabled: bool,
    selector: Value,
) {
    let pack_id = sensor.pack.expect("sensor pack");
    let pack_ref = sensor.pack_ref.clone().expect("sensor pack ref");
    let trigger =
        TriggerFixture::new_unique(Some(pack_id), Some(pack_ref.clone()), "sensor_admission")
            .with_enabled(trigger_enabled)
            .create(pool)
            .await
            .expect("trigger");
    sqlx::query("UPDATE trigger SET sensor = $1, sensor_ref = $2 WHERE id = $3")
        .bind(sensor.id)
        .bind(&sensor.r#ref)
        .bind(trigger.id)
        .execute(pool)
        .await
        .expect("attach trigger to sensor");

    RuleRepository::create_with_sensor_placement(
        pool,
        CreateRuleInput {
            r#ref: format!("{}.admission_{}", pack_ref, Uuid::new_v4().simple()),
            pack: pack_id,
            pack_ref,
            label: "Sensor admission rule".to_string(),
            description: None,
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref,
            conditions: json!([]),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
        RuleSensorPlacementInput {
            selector,
            tolerations: json!([]),
            affinity: json!({}),
        },
    )
    .await
    .expect("rule");
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn worker_eligibility_by_sensor_batches_counts_and_placement() {
    let pool = create_test_pool().await.expect("test database");
    let pack = PackFixture::new_unique("sensor_admission")
        .create(&pool)
        .await
        .expect("pack");
    let runtime =
        RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "sensor_runtime")
            .create(&pool)
            .await
            .expect("runtime");
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "sensor_action")
        .create(&pool)
        .await
        .expect("action");
    let matching = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "matching",
    )
    .create(&pool)
    .await
    .expect("matching sensor");
    let mismatching = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "mismatching",
    )
    .create(&pool)
    .await
    .expect("mismatching sensor");
    let idle = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "idle",
    )
    .create(&pool)
    .await
    .expect("idle sensor");
    create_rule_for_sensor(&pool, &matching, &action, true, json!({"site": "edge"})).await;
    create_rule_for_sensor(&pool, &matching, &action, true, json!({"site": "edge"})).await;
    create_rule_for_sensor(
        &pool,
        &mismatching,
        &action,
        true,
        json!({"site": "central"}),
    )
    .await;
    create_rule_for_sensor(&pool, &idle, &action, false, json!({"site": "central"})).await;
    let worker = create_sensor_worker(&pool, &runtime.name).await;
    let sensors = vec![matching.clone(), mismatching.clone(), idle.clone()];

    let mut connection = pool.acquire().await.expect("connection");
    let eligibility = SensorAdmissionRepository::worker_eligibility_by_sensor(
        &mut connection,
        &sensors,
        worker.id,
    )
    .await
    .expect("batch eligibility");

    assert_eq!(eligibility[&matching.id].active_rule_count, 2);
    assert!(eligibility[&matching.id].eligible);
    assert_eq!(eligibility[&mismatching.id].active_rule_count, 1);
    assert!(!eligibility[&mismatching.id].eligible);
    assert_eq!(eligibility[&idle.id].active_rule_count, 0);
    assert!(eligibility[&idle.id].eligible);
    for sensor in &sensors {
        let scalar =
            SensorAdmissionRepository::worker_is_eligible(&mut connection, sensor.id, worker.id)
                .await
                .expect("scalar eligibility");
        assert_eq!(eligibility[&sensor.id].eligible, scalar);
    }

    sqlx::query(
        "UPDATE worker SET last_heartbeat = clock_timestamp() - INTERVAL '91 seconds' WHERE id = $1",
    )
    .bind(worker.id)
    .execute(&pool)
    .await
    .expect("make worker stale");
    let eligibility = SensorAdmissionRepository::worker_eligibility_by_sensor(
        &mut connection,
        &sensors,
        worker.id,
    )
    .await
    .expect("stale worker eligibility");
    assert!(!eligibility[&matching.id].eligible);
    assert_eq!(
        eligibility[&matching.id].eligible,
        SensorAdmissionRepository::worker_is_eligible(&mut connection, matching.id, worker.id,)
            .await
            .expect("stale scalar eligibility")
    );
}
