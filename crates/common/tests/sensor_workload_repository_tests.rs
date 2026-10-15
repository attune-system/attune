//! Integration tests for durable sensor workload ownership and fencing.

mod helpers;

use attune_common::{
    models::{
        enums::{WorkerStatus, WorkerType},
        Sensor, SensorWorkloadFence, SensorWorkloadLease, Worker,
    },
    repositories::{
        component_lifecycle::PackProjectionIds,
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
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
        Create, FindById, PackRepository,
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
    setup_fixture_with_schema(json!({}), json!({})).await
}

async fn setup_fixture_with_schema(
    schema: serde_json::Value,
    config: serde_json::Value,
) -> (
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
    if let Some(references) = config
        .get("signing_permission_set_refs")
        .and_then(|value| value.as_array())
    {
        for reference in references {
            attune_common::repositories::identity::PermissionSetRepository::create(&*pool,
                attune_common::repositories::identity::CreatePermissionSetInput {
                    r#ref: reference.as_str().unwrap().to_string(), pack: None, pack_ref: None,
                    label: None, description: None,
                    grants: json!([{"resource": "keys", "actions": ["use", "read"], "constraints": {"refs": ["system.sensor_signer"]}}]),
                }).await.expect("signing permission set fixture");
        }
    }
    if let Some(references) = config
        .get("input_permission_set_refs")
        .and_then(|value| value.as_array())
    {
        for reference in references {
            attune_common::repositories::identity::PermissionSetRepository::create(&*pool,
                attune_common::repositories::identity::CreatePermissionSetInput {
                    r#ref: reference.as_str().unwrap().to_string(), pack: None, pack_ref: None, label: None, description: None,
                    grants: if reference.as_str() == Some("test.sensor_pack_inputs") {
                        json!([{"resource":"packs","actions":["read","decrypt","configure"],"constraints":{"refs":[pack.r#ref]}}])
                    } else {json!([{"resource":"keys","actions":["read","decrypt","use"],"constraints":{"refs":["system.sensor_input", "system.sensor_input_alt"]}}])},
                }).await.unwrap();
        }
    }
    sqlx::query("UPDATE sensor SET param_schema = $1, config = $2 WHERE id = $3")
        .bind(schema)
        .bind(config)
        .bind(sensor.id)
        .execute(&*pool)
        .await
        .expect("sensor settings");
    let trigger =
        TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "snapshot_trigger")
            .create(&pool)
            .await
            .expect("released trigger");
    sqlx::query("UPDATE trigger SET sensor = $1, sensor_ref = $2, out_schema = $3, event_config_schema = $4, param_schema = $6 WHERE id = $5")
        .bind(sensor.id).bind(&sensor.r#ref)
        .bind(json!({"token": {"type": "string", "secret": true}}))
        .bind(json!({"count": {"type": "integer", "default": 3}}))
        .bind(trigger.id)
        .bind(json!({"credential": {"type": "string", "secret": true}, "interval": {"type": "integer", "default": 5}}))
        .execute(&*pool).await.expect("released trigger settings");
    let mut tx = pool.begin().await.expect("begin release transaction");
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version.clone(),
            digest: "a".repeat(64),
            object_key: format!("packs/blobs/{}.tar.gz", pack.r#ref),
            provider_version: "e:test-version".to_string(),
            content_path: format!("/packs/.releases/{}", pack.r#ref),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .expect("release");
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        release.id,
        &PackProjectionIds {
            runtimes: vec![runtime.id],
            sensors: vec![sensor.id],
            triggers: vec![trigger.id],
            ..PackProjectionIds::default()
        },
    )
    .await
    .expect("activate release");
    tx.commit().await.expect("commit release transaction");

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
async fn sensor_generation_keeps_encrypted_config_and_contract_after_projection_changes() {
    use attune_common::repositories::sensor_schema::SensorSchemaRepository;
    let (pool, sensor, worker, _) = setup_fixture_with_schema(
        json!({"credential": {"type": "string", "secret": true}, "count": {"type": "integer", "default": 7}}),
        json!({"credential": "private-sensor-fixture"}),
    ).await;
    create_active_rule(&pool, &sensor).await;
    use attune_common::{
        models::RetentionPolicyType,
        repositories::{
            trigger::{SensorRepository, UpdateSensorInput},
            Patch,
        },
    };
    SensorRepository::update(
        &*pool,
        sensor.id,
        UpdateSensorInput {
            log_retention_policy: Some(Patch::Set(RetentionPolicyType::Versions)),
            log_retention_limit: Some(Patch::Set(1)),
            ..Default::default()
        },
    )
    .await
    .expect("set operational retention before generation admission");
    let key = "0123456789abcdef0123456789abcdef";
    let outcome = SensorWorkloadAdmissionRepository::acquire_with_encryption_key(
        &pool,
        acquire_input(sensor.id, worker.id, Uuid::new_v4()),
        Some(key),
    )
    .await
    .expect("admit secret sensor settings");
    let AcquireEligibleSensorWorkloadOutcome::Acquired(workload) = outcome else {
        panic!("not admitted")
    };
    let (id, binding, public) =
        SensorSchemaRepository::find(&*pool, workload.workload_id, workload.generation)
            .await
            .expect("binding lookup")
            .expect("binding");
    assert!(!serde_json::to_string(&binding)
        .unwrap()
        .contains("private-sensor-fixture"));
    assert!(!public.to_string().contains("private-sensor-fixture"));
    assert_eq!(public["count"], 7);
    assert_eq!(
        binding.executable.sensor.log_retention_policy,
        Some(RetentionPolicyType::Versions)
    );
    assert_eq!(binding.executable.sensor.log_retention_limit, Some(1));
    SensorRepository::update(
        &*pool,
        sensor.id,
        UpdateSensorInput {
            log_retention_limit: Some(Patch::Set(9)),
            ..Default::default()
        },
    )
    .await
    .expect("change current operational retention");
    let (_, preserved, _) =
        SensorSchemaRepository::find(&*pool, workload.workload_id, workload.generation)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(preserved.executable.sensor.log_retention_limit, Some(1));
    let admitted_trigger = binding
        .executable
        .triggers
        .first()
        .expect("trigger snapshot");
    let event_binding = &binding.triggers[&admitted_trigger.r#ref].event;
    sqlx::query("UPDATE trigger SET out_schema = $1, event_config_schema = $2 WHERE id = $3")
        .bind(json!({"token": {"type": "integer"}}))
        .bind(json!({"count": {"type": "integer", "default": 99}}))
        .bind(admitted_trigger.id)
        .execute(&*pool)
        .await
        .expect("change live trigger schemas");
    let protected = event_binding
        .prepare(Some(json!({"token": "original-secret"})), Some(json!({})))
        .expect("validate event with admitted schema");
    assert_eq!(protected.config.unwrap()["count"], 3);
    assert!(!protected
        .payload
        .unwrap()
        .to_string()
        .contains("original-secret"));
    sqlx::query("UPDATE sensor SET param_schema = $1, config = $2 WHERE id = $3")
        .bind(json!({"credential": {"type": "integer"}}))
        .bind(json!({"credential": 99}))
        .bind(sensor.id)
        .execute(&*pool)
        .await
        .expect("change mutable settings");
    let mut connection = pool.acquire().await.unwrap();
    let restored =
        SensorSchemaRepository::restore_config(&mut connection, id, &binding, public, Some(key))
            .await
            .expect("restore original settings");
    assert_eq!(
        restored,
        json!({"credential": "private-sensor-fixture", "count": 7})
    );
    assert!(
        sqlx::query("UPDATE sensor_generation_schema_binding SET config = '{}' WHERE id = $1")
            .bind(id)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    assert!(sqlx::query("UPDATE execution_secret_value SET encrypted_value = '{}'::jsonb WHERE entity_type = 'sensor_generation_config' AND entity_id = $1")
        .bind(id).execute(&mut *connection).await.is_err());
    sqlx::query("DELETE FROM sensor_workload WHERE id = $1")
        .bind(workload.workload_id)
        .execute(&mut *connection)
        .await
        .expect("delete generation owner");
    assert!(SensorSchemaRepository::find(
        &mut *connection,
        workload.workload_id,
        workload.generation
    )
    .await
    .unwrap()
    .is_none());
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_secret_value WHERE entity_type = 'sensor_generation_config' AND entity_id = $1")
        .bind(id).fetch_one(&mut *connection).await.unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn sensor_sdk_key_read_origins_enforce_count_and_total_byte_budgets() {
    use attune_common::{
        repositories::sensor_schema::SensorSchemaRepository, secret_provenance::KeyOrigin,
    };
    let (pool, sensor, worker, _) = setup_fixture().await;
    create_active_rule(&pool, &sensor).await;
    let outcome = SensorWorkloadAdmissionRepository::acquire_with_encryption_key(
        &pool,
        acquire_input(sensor.id, worker.id, Uuid::new_v4()),
        Some("0123456789abcdef0123456789abcdef"),
    )
    .await
    .expect("admit SDK provenance fixture");
    let AcquireEligibleSensorWorkloadOutcome::Acquired(workload) = outcome else {
        panic!("SDK provenance fixture was not admitted")
    };
    let (generation, _, _) =
        SensorSchemaRepository::find(&*pool, workload.workload_id, workload.generation)
            .await
            .unwrap()
            .unwrap();
    let base = KeyOrigin {
        key_id: 1,
        key_ref: "system.sdk_fixture".into(),
        owner_type: attune_common::models::OwnerType::System,
        owner_identity: None,
        owner_ref: None,
        encrypted: true,
    };
    let mut transaction = pool.begin().await.unwrap();
    for index in 0..1024 {
        let mut origin = base.clone();
        origin.key_id = index + 1;
        origin.key_ref = format!("system.sdk_fixture_{index}");
        SensorSchemaRepository::record_key_read_origin(&mut transaction, generation, &origin)
            .await
            .expect("record bounded SDK origin");
    }
    assert_eq!(
        SensorSchemaRepository::key_read_origins(&mut *transaction, generation)
            .await
            .unwrap()
            .len(),
        1024
    );
    let repeated = KeyOrigin {
        key_ref: "system.sdk_fixture_0".into(),
        ..base.clone()
    };
    SensorSchemaRepository::record_key_read_origin(&mut transaction, generation, &repeated)
        .await
        .expect("a repeated read must remain available at the count limit");
    let error = SensorSchemaRepository::record_key_read_origin(&mut transaction, generation, &base)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("origin limit reached"));
    transaction.rollback().await.unwrap();

    let mut transaction = pool.begin().await.unwrap();
    let mut rejected = false;
    for index in 0..100 {
        let origin = KeyOrigin {
            key_id: index + 1,
            key_ref: format!("system.sdk_{index}_{}", "x".repeat(15000)),
            ..base.clone()
        };
        if let Err(error) =
            SensorSchemaRepository::record_key_read_origin(&mut transaction, generation, &origin)
                .await
        {
            assert!(error
                .to_string()
                .contains("origins exceed their byte limit"));
            rejected = true;
            break;
        }
    }
    assert!(
        rejected,
        "SDK origins must not exceed their aggregate byte budget"
    );
    let retained = SensorSchemaRepository::key_read_origins(&mut *transaction, generation)
        .await
        .unwrap();
    assert!(!retained.is_empty());
    assert!(retained.len() < 100);
    // Corrupt aggregate metadata via a fixture write to exercise fail-closed reads,
    // independently of the normal writer's budget enforcement.
    let extra = KeyOrigin {
        key_id: 1000,
        key_ref: format!("system.sdk_extra_{}", "x".repeat(15000)),
        ..base
    };
    let extra_body = serde_json::to_value(extra).unwrap();
    let extra_digest = "f".repeat(64);
    sqlx::query("INSERT INTO sensor_generation_key_read_origin (generation_binding, origin_digest, origin) VALUES ($1, $2, $3)")
        .bind(generation).bind(extra_digest).bind(extra_body)
        .execute(&mut *transaction).await.unwrap();
    let error = SensorSchemaRepository::key_read_origins(&mut *transaction, generation)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("origins exceed their byte limit"));
    transaction.rollback().await.unwrap();
    assert!(SensorSchemaRepository::key_read_origins(&*pool, generation)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn secret_sensor_admission_without_encryption_rolls_back_assignment() {
    let (pool, sensor, worker, _) = setup_fixture_with_schema(
        json!({"credential": {"type": "string", "secret": true}}),
        json!({"credential": "private-sensor-fixture"}),
    )
    .await;
    create_active_rule(&pool, &sensor).await;
    let error = SensorWorkloadAdmissionRepository::acquire(
        &pool,
        acquire_input(sensor.id, worker.id, Uuid::new_v4()),
    )
    .await
    .unwrap_err();
    assert!(!error.to_string().contains("private-sensor-fixture"));
    let assignments: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sensor_workload_assignment a JOIN sensor_workload w ON w.id = a.workload WHERE w.sensor = $1")
        .bind(sensor.id).fetch_one(&*pool).await.unwrap();
    assert_eq!(assignments, 0);
}

#[tokio::test]
async fn sensor_signing_grants_are_release_owned_use_only_and_immutable_after_policy_changes() {
    let (pool, sensor, worker, _) = setup_fixture_with_schema(
        json!({}),
        json!({"signing_permission_set_refs": ["test.sensor_signing"]}),
    )
    .await;
    create_active_rule(&pool, &sensor).await;
    let AcquireEligibleSensorWorkloadOutcome::Acquired(workload) =
        SensorWorkloadAdmissionRepository::acquire(
            &pool,
            acquire_input(sensor.id, worker.id, Uuid::new_v4()),
        )
        .await
        .unwrap()
    else {
        panic!("not admitted")
    };
    let (_, binding, _) = attune_common::repositories::sensor_schema::SensorSchemaRepository::find(
        &*pool,
        workload.workload_id,
        workload.generation,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        binding.executable.signing_permission_set_refs,
        ["test.sensor_signing"]
    );
    assert_eq!(binding.executable.signing_grants.len(), 1);
    assert_eq!(
        binding.executable.signing_grants[0].actions,
        [attune_common::rbac::Action::Use]
    );
    sqlx::query("UPDATE permission_set SET grants = '[]' WHERE ref = 'test.sensor_signing'")
        .execute(&*pool)
        .await
        .unwrap();
    sqlx::query("UPDATE sensor SET config = '{}' WHERE id = $1")
        .bind(sensor.id)
        .execute(&*pool)
        .await
        .unwrap();
    let (_, retained, _) =
        attune_common::repositories::sensor_schema::SensorSchemaRepository::find(
            &*pool,
            workload.workload_id,
            workload.generation,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retained.executable.signing_grants,
        binding.executable.signing_grants
    );
}

#[tokio::test]
async fn sensor_input_templates_use_deployed_read_decrypt_grants_and_record_key_origins() {
    use attune_common::repositories::key::{CreateKeyInput, KeyRepository};
    let (pool, sensor, worker, _) = setup_fixture_with_schema(
        json!({"credential":{"type":"string","secret":true},"sensor_id":{"type":"integer"}}),
        json!({"credential":"{{ keystore['system.sensor_input'] }}", "sensor_id":"{{ system.sensor.id }}", "input_permission_set_refs":["test.sensor_inputs"]}),
    ).await;
    create_active_rule(&pool, &sensor).await;
    let key = "0123456789abcdef0123456789abcdef";
    let mut tx = pool.begin().await.unwrap();
    attune_common::repositories::component_configuration::ComponentConfigurationRepository::lock_selection(&mut tx).await.unwrap();
    let author =
        attune_common::repositories::trigger::SensorRepository::find_by_id(&mut *tx, sensor.id)
            .await
            .unwrap()
            .unwrap();
    let revision = attune_common::repositories::component_configuration::ComponentConfigurationRepository::bind_sensor(
        &mut tx,&author,author.config.as_ref(),Some(key)).await.unwrap();
    assert!(!revision.config.to_string().contains("keystore"));
    tx.commit().await.unwrap();
    let stored_key = KeyRepository::create(
        &*pool,
        CreateKeyInput {
            local_ref: "sensor_input".into(),
            owner_type: attune_common::models::OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Sensor input".into(),
            encrypted: true,
            encryption_key_hash: Some(attune_common::crypto::hash_encryption_key(key)),
            value: attune_common::crypto::encrypt_json(&json!("private-key-input"), key).unwrap(),
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE permission_set SET grants = '[]' WHERE ref = 'test.sensor_inputs'")
        .execute(&*pool)
        .await
        .unwrap();
    let AcquireEligibleSensorWorkloadOutcome::Acquired(workload) =
        SensorWorkloadAdmissionRepository::acquire_with_encryption_key(
            &pool,
            acquire_input(sensor.id, worker.id, Uuid::new_v4()),
            Some(key),
        )
        .await
        .unwrap()
    else {
        panic!("not admitted")
    };
    let (id, binding, public) =
        attune_common::repositories::sensor_schema::SensorSchemaRepository::find(
            &*pool,
            workload.workload_id,
            workload.generation,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        binding.executable.input_grants[0].actions,
        [
            attune_common::rbac::Action::Read,
            attune_common::rbac::Action::Decrypt
        ]
    );
    assert!(!public.to_string().contains("private-key-input"));
    let mut connection = pool.acquire().await.unwrap();
    let restored =
        attune_common::repositories::sensor_schema::SensorSchemaRepository::restore_config(
            &mut connection,
            id,
            &binding,
            public,
            Some(key),
        )
        .await
        .unwrap();
    assert_eq!(restored["credential"], "private-key-input");
    let secrets = attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *connection, attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_CONFIG, id).await.unwrap();
    let provenance: attune_common::secret_provenance::SecretProvenance =
        serde_json::from_str(secrets[0].source_ref.as_ref().unwrap()).unwrap();
    assert!(provenance.origins.iter().any(|origin| matches!(origin, attune_common::secret_provenance::SecretOrigin::Key(origin) if origin.key_id == stored_key.id)));
}

#[tokio::test]
async fn sensor_pack_inputs_use_deployed_pack_read_decrypt_and_record_configuration_revision_origins(
) {
    use attune_common::repositories::{
        component_configuration::ComponentConfigurationRepository, pack::UpdatePackInput,
    };
    let (pool, sensor, worker, _) = setup_fixture_with_schema(
        json!({"credential":{"type":"string","secret":true}}),
        json!({"credential":"{{ pack.config.token }}","input_permission_set_refs":["test.sensor_pack_inputs"]}),
    ).await;
    create_active_rule(&pool, &sensor).await;
    let key = "0123456789abcdef0123456789abcdef";
    let mut tx = pool.begin().await.unwrap();
    ComponentConfigurationRepository::lock_selection(&mut tx)
        .await
        .unwrap();
    let pack = PackRepository::update(
        &mut *tx,
        sensor.pack.unwrap(),
        UpdatePackInput {
            conf_schema: Some(json!({"token":{"type":"string","secret":true}})),
            config: Some(json!({"token":"private-pack-input"})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let revision =
        ComponentConfigurationRepository::bind_pack(&mut tx, &pack, Some(&pack.config), Some(key))
            .await
            .unwrap();
    tx.commit().await.unwrap();
    sqlx::query("UPDATE permission_set SET grants = '[]' WHERE ref = 'test.sensor_pack_inputs'")
        .execute(&*pool)
        .await
        .unwrap();
    let AcquireEligibleSensorWorkloadOutcome::Acquired(workload) =
        SensorWorkloadAdmissionRepository::acquire_with_encryption_key(
            &pool,
            acquire_input(sensor.id, worker.id, Uuid::new_v4()),
            Some(key),
        )
        .await
        .unwrap()
    else {
        panic!("not admitted")
    };
    let (id, binding, public) =
        attune_common::repositories::sensor_schema::SensorSchemaRepository::find(
            &*pool,
            workload.workload_id,
            workload.generation,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        binding.executable.input_grants[0].resource,
        attune_common::rbac::Resource::Packs
    );
    assert_eq!(
        binding.executable.input_grants[0].actions,
        [
            attune_common::rbac::Action::Read,
            attune_common::rbac::Action::Decrypt
        ]
    );
    assert!(!public.to_string().contains("private-pack-input"));
    let mut connection = pool.acquire().await.unwrap();
    let value = attune_common::repositories::sensor_schema::SensorSchemaRepository::restore_config(
        &mut connection,
        id,
        &binding,
        public,
        Some(key),
    )
    .await
    .unwrap();
    assert_eq!(value["credential"], "private-pack-input");
    let secrets = attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *connection, attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_CONFIG, id).await.unwrap();
    let provenance: attune_common::secret_provenance::SecretProvenance =
        serde_json::from_str(secrets[0].source_ref.as_ref().unwrap()).unwrap();
    assert!(provenance.origins.iter().any(|origin| matches!(origin, attune_common::secret_provenance::SecretOrigin::Entity {
        entity_type, entity_id, path,
    } if entity_type == attune_common::repositories::component_configuration::ENTITY_COMPONENT_CONFIGURATION && *entity_id == revision.id && path == "/token")));
}

#[tokio::test]
async fn sensor_initial_rule_values_are_encrypted_and_pinned_before_launch() {
    use attune_common::repositories::sensor_schema::SensorSchemaRepository;
    let (pool, sensor, worker, _) = setup_fixture_with_schema(
        json!({}),
        json!({"input_permission_set_refs":["test.sensor_inputs"]}),
    )
    .await;
    let key = "0123456789abcdef0123456789abcdef";
    use attune_common::repositories::key::{CreateKeyInput, KeyRepository};
    let input_key = KeyRepository::create(
        &*pool,
        CreateKeyInput {
            local_ref: "sensor_input".into(),
            owner_type: attune_common::models::OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Sensor input".into(),
            encrypted: true,
            encryption_key_hash: Some(attune_common::crypto::hash_encryption_key(key)),
            value: attune_common::crypto::encrypt_json(&json!("private-rule-fixture"), key)
                .unwrap(),
        },
    )
    .await
    .unwrap();
    let rule_id = create_active_rule(&pool, &sensor).await;
    let trigger =
        attune_common::repositories::trigger::TriggerRepository::find_by_sensor(&*pool, sensor.id)
            .await
            .unwrap()
            .into_iter()
            .find(|trigger| trigger.r#ref.contains("snapshot_trigger"))
            .unwrap();
    sqlx::query(
        "UPDATE rule SET trigger = $1, trigger_ref = $2, trigger_params = $3 WHERE id = $4",
    )
    .bind(trigger.id)
    .bind(&trigger.r#ref)
    .bind(json!({"credential":"{{ keystore['system.sensor_input'] }}"}))
    .bind(rule_id)
    .execute(&*pool)
    .await
    .unwrap();
    let AcquireEligibleSensorWorkloadOutcome::Acquired(workload) =
        SensorWorkloadAdmissionRepository::acquire_with_encryption_key(
            &pool,
            acquire_input(sensor.id, worker.id, Uuid::new_v4()),
            Some(key),
        )
        .await
        .unwrap()
    else {
        panic!("not admitted")
    };
    let (id, binding, _) =
        SensorSchemaRepository::find(&*pool, workload.workload_id, workload.generation)
            .await
            .unwrap()
            .unwrap();
    let public: serde_json::Value = sqlx::query_scalar(
        "SELECT initial_rules FROM sensor_generation_schema_binding WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&*pool)
    .await
    .unwrap();
    assert!(!public.to_string().contains("private-rule-fixture"));
    sqlx::query("UPDATE rule SET trigger_params = '{}' WHERE id = $1")
        .bind(rule_id)
        .execute(&*pool)
        .await
        .unwrap();
    let mut connection = pool.acquire().await.unwrap();
    let rules =
        SensorSchemaRepository::restore_initial_rules(&mut connection, id, &binding, Some(key))
            .await
            .unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["id"], rule_id);
    assert_eq!(
        rules[0]["config"],
        json!({"credential":"private-rule-fixture", "interval":5})
    );
    assert!(
        SensorSchemaRepository::restore_initial_rules(&mut connection, id, &binding, None)
            .await
            .is_err()
    );
    let failure = sqlx::query("UPDATE execution_secret_value SET encrypted_value = '{}' WHERE entity_type = 'sensor_generation_rule_config' AND entity_id = $1")
        .bind(id).execute(&mut *connection).await.unwrap_err();
    assert!(failure.to_string().contains("immutable"));
    let rule = RuleRepository::find_by_id(&mut *connection, rule_id)
        .await
        .unwrap()
        .unwrap();
    let mut changed = rule.clone();
    changed.trigger_params = json!({"credential":"updated-rule-fixture"});
    let mut rejected = pool.begin().await.unwrap();
    let error = SensorSchemaRepository::capture_rule_delivery(
        &mut rejected,
        id,
        &binding,
        &changed,
        Some(key),
    )
    .await
    .unwrap_err();
    assert!(!error.to_string().contains("updated-rule-fixture"));
    rejected.rollback().await.unwrap();
    changed.trigger_params = json!({"credential":"{{ keystore['system.sensor_input'] }}"});
    sqlx::query("UPDATE key SET value = $1 WHERE id = $2")
        .bind(attune_common::crypto::encrypt_json(&json!("updated-rule-fixture"), key).unwrap())
        .bind(input_key.id)
        .execute(&*pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let delivered =
        SensorSchemaRepository::capture_rule_delivery(&mut tx, id, &binding, &changed, Some(key))
            .await
            .unwrap();
    assert_eq!(
        delivered,
        json!({"credential":"updated-rule-fixture", "interval":5})
    );
    SensorSchemaRepository::capture_rule_delivery(&mut tx, id, &binding, &changed, Some(key))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let stored: Vec<(i64, serde_json::Value)> = sqlx::query_as(
        "SELECT id, config FROM sensor_rule_config_delivery WHERE generation_binding = $1",
    )
    .bind(id)
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert_eq!(stored.len(), 1);
    assert!(!stored[0].1.to_string().contains("updated-rule-fixture"));
    sqlx::query("UPDATE key SET value = $1 WHERE id = $2")
        .bind(attune_common::crypto::encrypt_json(&json!("another-rule-fixture"), key).unwrap())
        .bind(input_key.id)
        .execute(&*pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    SensorSchemaRepository::capture_rule_delivery(&mut tx, id, &binding, &changed, Some(key))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let paths = SensorSchemaRepository::delivery_origin_paths(&mut *connection, id)
        .await
        .unwrap();
    assert_eq!(paths.len(), 2);
    assert_ne!(paths[0].0, paths[1].0);
    assert_eq!(paths[0].1, "/credential");
    let alternate_key = KeyRepository::create(
        &*pool,
        CreateKeyInput {
            local_ref: "sensor_input_alt".into(),
            owner_type: attune_common::models::OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Alternate sensor input".into(),
            encrypted: true,
            encryption_key_hash: Some(attune_common::crypto::hash_encryption_key(key)),
            value: attune_common::crypto::encrypt_json(&json!("another-rule-fixture"), key)
                .unwrap(),
        },
    )
    .await
    .unwrap();
    changed.trigger_params = json!({"credential":"{{ keystore['system.sensor_input_alt'] }}"});
    let mut tx = pool.begin().await.unwrap();
    SensorSchemaRepository::capture_rule_delivery(&mut tx, id, &binding, &changed, Some(key))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let paths = SensorSchemaRepository::delivery_origin_paths(&mut *connection, id)
        .await
        .unwrap();
    assert_eq!(
        paths.len(),
        3,
        "equal values with different Key origins must not reuse a delivery"
    );
    let secrets = attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *connection, attune_common::repositories::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG, paths[2].0).await.unwrap();
    let provenance: attune_common::secret_provenance::SecretProvenance =
        serde_json::from_str(secrets[0].source_ref.as_ref().unwrap()).unwrap();
    assert!(provenance.origins.iter().any(|origin| matches!(origin, attune_common::secret_provenance::SecretOrigin::Key(origin) if origin.key_id == alternate_key.id)));
}

async fn activate_schema_test_release(
    pool: &PgPool,
    pack: &attune_common::models::Pack,
    digest: &str,
    version: &str,
    manifest: serde_json::Value,
    projections: PackProjectionIds,
) {
    let mut tx = pool.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: version.to_string(),
            digest: digest.repeat(64),
            object_key: format!("packs/blobs/{}-{digest}.tar.gz", pack.r#ref),
            provider_version: "e:test-version".into(),
            content_path: format!("/packs/.releases/{}-{digest}", pack.r#ref),
            archive_size: 1,
            manifest,
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(&mut tx, pack.id, release.id, &projections)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn external_sensor_trigger_bodies_are_assigned_once_under_eligible_release_dependencies() {
    let (pool, sensor, worker, _) = setup_fixture().await;
    create_active_rule(&pool, &sensor).await;
    let pack = PackRepository::find_by_id(&*pool, sensor.pack.unwrap())
        .await
        .unwrap()
        .unwrap();
    let foreign = PackFixture::new_unique("sensor_external")
        .create(&pool)
        .await
        .unwrap();
    let trigger = TriggerFixture::new_unique(
        Some(foreign.id),
        Some(foreign.r#ref.clone()),
        "foreign_trigger",
    )
    .create(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE trigger SET sensor = $1, sensor_ref = $2, out_schema = $3 WHERE id = $4")
        .bind(sensor.id)
        .bind(&sensor.r#ref)
        .bind(json!({"count":{"type":"integer","default":1}}))
        .bind(trigger.id)
        .execute(&*pool)
        .await
        .unwrap();
    activate_schema_test_release(
        &pool,
        &foreign,
        "c",
        &foreign.version,
        json!({}),
        PackProjectionIds {
            triggers: vec![trigger.id],
            ..Default::default()
        },
    )
    .await;
    let projections = PackProjectionIds {
        runtimes: vec![sensor.runtime],
        sensors: vec![sensor.id],
        ..Default::default()
    };
    activate_schema_test_release(
        &pool,
        &pack,
        "b",
        "1.0.1",
        json!({"dependencies":{(foreign.r#ref.clone()):"*"}}),
        projections,
    )
    .await;
    let AcquireEligibleSensorWorkloadOutcome::Acquired(workload) =
        SensorWorkloadAdmissionRepository::acquire(
            &pool,
            acquire_input(sensor.id, worker.id, Uuid::new_v4()),
        )
        .await
        .unwrap()
    else {
        panic!("not admitted")
    };
    let (_, binding, _) = attune_common::repositories::sensor_schema::SensorSchemaRepository::find(
        &*pool,
        workload.workload_id,
        workload.generation,
    )
    .await
    .unwrap()
    .unwrap();
    let event = &binding.triggers[&trigger.r#ref].event;
    assert!(matches!(
        event.payload.as_ref().unwrap().source,
        attune_common::value_schema::SourceIdentity::Assigned { .. }
    ));
    sqlx::query("UPDATE trigger SET out_schema = $1 WHERE id = $2")
        .bind(json!({"count":{"type":"integer","default":99}}))
        .bind(trigger.id)
        .execute(&*pool)
        .await
        .unwrap();
    assert_eq!(
        event
            .prepare(Some(json!({})), None)
            .unwrap()
            .payload
            .unwrap()["count"],
        1
    );
}

#[tokio::test]
async fn callback_demand_acquires_and_renews_without_active_rules() {
    let (pool, sensor, worker, _) = setup_fixture().await;
    sqlx::query("UPDATE sensor SET config = $1 WHERE id = $2")
        .bind(json!({
            "inquiry_callback_adapters": {
                "test.callback": {
                    "provider": "test",
                    "subject_kind": "user",
                    "request": {
                        "delivery_id_pointer": "/id",
                        "tenant_pointer": "/tenant",
                        "external_subject_pointer": "/user",
                        "response_handle_pointer": "/handle"
                    }
                }
            }
        }))
        .bind(sensor.id)
        .execute(&*pool)
        .await
        .expect("enable callback adapter");

    let acquired = SensorWorkloadAdmissionRepository::acquire(
        &pool,
        acquire_input(sensor.id, worker.id, Uuid::new_v4()),
    )
    .await
    .expect("acquire callback workload");
    let AcquireEligibleSensorWorkloadOutcome::Acquired(acquired) = acquired else {
        panic!("callback workload was not acquired");
    };

    let renewed = SensorWorkloadAdmissionRepository::renew(
        &pool,
        sensor.id,
        SensorWorkloadLeaseInput {
            fence: acquired.fence(),
            lease_seconds: LEASE_SECONDS * 2,
        },
    )
    .await
    .expect("renew callback workload");
    assert!(matches!(
        renewed,
        RenewEligibleSensorWorkloadOutcome::Renewed(_)
    ));
}

#[tokio::test]
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
async fn pack_release_lookup_requires_the_complete_current_fence() {
    let (pool, sensor, worker, other_worker) = setup_fixture().await;
    let acquired = acquire(&pool, sensor.id, worker.id, Uuid::new_v4()).await;
    let current = SensorWorkloadRepository::begin_process(&pool, acquired)
        .await
        .expect("begin process")
        .expect("owned workload");
    let fence = current.fence();

    assert_eq!(
        SensorWorkloadRepository::pack_release_for_current_fence(&pool, fence)
            .await
            .expect("current release"),
        current.pack_release
    );
    assert!(SensorWorkloadRepository::pack_release_for_current_fence(
        &pool,
        SensorWorkloadFence {
            generation: fence.generation - 1,
            ..fence
        },
    )
    .await
    .unwrap()
    .is_none());
    assert!(SensorWorkloadRepository::pack_release_for_current_fence(
        &pool,
        SensorWorkloadFence {
            worker_id: other_worker.id,
            ..fence
        },
    )
    .await
    .unwrap()
    .is_none());
    assert!(SensorWorkloadRepository::pack_release_for_current_fence(
        &pool,
        SensorWorkloadFence {
            worker_instance: Uuid::new_v4(),
            ..fence
        },
    )
    .await
    .unwrap()
    .is_none());

    sqlx::query(
        "UPDATE sensor_workload_assignment SET lease_expires_at = NOW() - INTERVAL '1 second' \
         WHERE workload = $1",
    )
    .bind(fence.workload_id)
    .execute(&*pool)
    .await
    .expect("expire lease");
    assert!(
        SensorWorkloadRepository::pack_release_for_current_fence(&pool, fence)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
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
async fn periodic_reconciliation_converges_when_one_replica_misses_pack_lifecycle_prompt() {
    let (pool, sensor, worker_a, worker_b) = setup_fixture().await;
    create_active_rule(&pool, &sensor).await;
    sqlx::query("UPDATE worker SET capabilities = capabilities || $2::jsonb WHERE id = $1")
        .bind(worker_a.id)
        .bind(json!({"labels": {"replica": "a"}}))
        .execute(&*pool)
        .await
        .expect("label replica a");
    sqlx::query("UPDATE worker SET capabilities = capabilities || $2::jsonb WHERE id = $1")
        .bind(worker_b.id)
        .bind(json!({"labels": {"replica": "b"}}))
        .execute(&*pool)
        .await
        .expect("label replica b");

    let owner_a = match SensorWorkloadAdmissionRepository::acquire(
        &pool,
        acquire_input(sensor.id, worker_a.id, Uuid::new_v4()),
    )
    .await
    .expect("replica a initial reconciliation")
    {
        AcquireEligibleSensorWorkloadOutcome::Acquired(workload) => workload,
        outcome => panic!("replica a should own the initial workload: {outcome:?}"),
    };

    PackRepository::update_worker_placement(
        &pool,
        sensor.pack.expect("sensor pack"),
        &json!({"replica": "b"}),
        &json!([]),
        &json!({}),
    )
    .await
    .expect("request pack placement on replica b");

    // Only replica A receives the prompt. Replica B deliberately misses it.
    assert_eq!(
        SensorWorkloadAdmissionRepository::renew(
            &pool,
            sensor.id,
            SensorWorkloadLeaseInput {
                fence: owner_a.fence(),
                lease_seconds: LEASE_SECONDS,
            },
        )
        .await
        .expect("prompted replica a reconciliation"),
        RenewEligibleSensorWorkloadOutcome::Ineligible
    );
    assert!(SensorWorkloadRepository::release(&pool, owner_a.fence())
        .await
        .expect("old owner releases workload"));

    // Replica B's periodic PostgreSQL reconciliation converges without a prompt.
    let owner_b = match SensorWorkloadAdmissionRepository::acquire(
        &pool,
        acquire_input(sensor.id, worker_b.id, Uuid::new_v4()),
    )
    .await
    .expect("replica b periodic reconciliation")
    {
        AcquireEligibleSensorWorkloadOutcome::Acquired(workload) => workload,
        outcome => panic!("replica b should acquire requested pack workload: {outcome:?}"),
    };
    assert_eq!(owner_b.worker_id, worker_b.id);
    assert!(owner_b.generation > owner_a.generation);
    let assignment = SensorWorkloadRepository::find_assignment(&pool, owner_b.workload_id)
        .await
        .expect("load converged assignment")
        .expect("assignment exists");
    assert_eq!(assignment.worker, Some(worker_b.id));
    assert_eq!(assignment.generation, owner_b.generation);
}

#[tokio::test]
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
