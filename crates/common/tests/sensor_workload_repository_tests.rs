//! Integration tests for durable sensor workload ownership and fencing.

#[path = "sensor_workload_repository_tests/c_generic_proof.rs"]
mod c_generic_proof;
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

#[path = "sensor_workload_repository_tests/origin_batches.rs"]
mod origin_batches;

fn fixture_digest(value: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(serde_jcs::to_vec(value).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Match the complete producer source, including placement and dispatch inputs.
/// H captures this body itself during policy creation. The fixture supplies only
/// the expected digest, never an invented retained row after capture returns.
fn fixture_rule_dispatch_source(
    rule: &attune_common::models::Rule,
    trigger: &attune_common::models::Trigger,
    sensor: &Sensor,
) -> serde_json::Value {
    let stored = serde_json::to_value(rule).unwrap();
    let mut source = json!({
        "rule_id": rule.id, "rule_ref": rule.r#ref, "pack_ref": rule.pack_ref,
        "managing_pack_id": rule.pack,
        "trigger_managing_pack_id": trigger.pack,
        "sensor_id": sensor.id, "sensor_managing_pack_id": sensor.pack,
    });
    for field in [
        "action",
        "action_ref",
        "trigger",
        "trigger_ref",
        "conditions",
        "action_params",
        "trigger_params",
        "permission_set_refs",
        "trace_tag_template",
        "sensor_worker_selector",
        "sensor_worker_tolerations",
        "sensor_worker_affinity",
    ] {
        source[field] = stored
            .get(field)
            .expect("complete stored Rule field")
            .clone();
    }
    source
}

#[tokio::test]
async fn sensor_fixed_external_approval_survives_issuer_freeze_but_not_ref_reuse_or_revocation() {
    use attune_common::repositories::{
        component_configuration::ComponentConfigurationRepository,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
            PermissionAssignmentRepository, PermissionSetRepository, UpdateIdentityInput,
        },
        key::KeyRepository,
        managed_sensor_metadata::ManagedSensorMetadataRepository,
        runtime_authority::RuntimeAuthorityRepository,
        trigger::{SensorRepository, UpdateSensorInput},
        Delete, Update,
    };
    let (pool, sensor, worker, _) = setup_fixture().await;
    create_active_rule(&pool, &sensor).await;
    let foreign = PackFixture::new_unique("foreign_sensor_keys")
        .create(&pool)
        .await
        .unwrap();
    let target =
        helpers::KeyFixture::new_pack(foreign.id, &foreign.r#ref, "signer", "fixture-value")
            .create(&pool)
            .await
            .unwrap();
    let issuer = helpers::IdentityFixture::new_unique("sensor_approval_issuer")
        .create(&pool)
        .await
        .unwrap();
    let editor = helpers::IdentityFixture::new_unique("sensor_control_editor")
        .create(&pool)
        .await
        .unwrap();
    let grant = json!({"resource":"keys","actions":["use"],"constraints":{"refs":[target.r#ref]}});
    let issuer_set = PermissionSetRepository::create(
        &*pool,
        CreatePermissionSetInput {
            r#ref: format!("{}.issuer", foreign.r#ref),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([grant]),
        },
    )
    .await
    .unwrap();
    PermissionAssignmentRepository::create(
        &*pool,
        CreatePermissionAssignmentInput {
            identity: issuer.id,
            permset: issuer_set.id,
        },
    )
    .await
    .unwrap();
    let selected = PermissionSetRepository::create(
        &*pool,
        CreatePermissionSetInput {
            r#ref: format!("{}.signing", foreign.r#ref),
            pack: Some(foreign.id),
            pack_ref: Some(foreign.r#ref.clone()),
            label: None,
            description: None,
            grants: json!([grant]),
        },
    )
    .await
    .unwrap();
    // The foreign set keeps its actual structural managing-pack envelope.
    let effective = PermissionSetRepository::find_by_id(&*pool, selected.id)
        .await
        .unwrap()
        .unwrap();
    let normalized: Vec<attune_common::rbac::Grant> =
        serde_json::from_value(effective.grants).unwrap();
    let key = "0123456789abcdef0123456789abcdef";
    let mut tx = pool.begin().await.unwrap();
    ManagedSensorMetadataRepository::lock_mutation(&mut tx)
        .await
        .unwrap();
    RuntimeAuthorityRepository::ensure_owner(&mut tx, sensor.pack.unwrap())
        .await
        .unwrap();
    let approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        sensor.pack.unwrap(),
        issuer.id,
        normalized.clone(),
        None,
    )
    .await
    .unwrap();
    let current = SensorRepository::find_by_id(&mut *tx, sensor.id)
        .await
        .unwrap()
        .unwrap();
    let patch = json!({"signing_permission_set_refs":[selected.r#ref]});
    ComponentConfigurationRepository::bind_sensor_readmission(
        &mut tx,
        &current,
        Some(&patch),
        &[],
        Some(key),
    )
    .await
    .unwrap();
    ManagedSensorMetadataRepository::readmit(&mut tx, &current, editor.id, Some(key))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let AcquireEligibleSensorWorkloadOutcome::Acquired(owned) =
        SensorWorkloadAdmissionRepository::acquire_with_encryption_key(
            &pool,
            acquire_input(sensor.id, worker.id, Uuid::new_v4()),
            Some(key),
        )
        .await
        .unwrap()
    else {
        panic!("sensor not admitted")
    };
    IdentityRepository::update(
        &*pool,
        issuer.id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        SensorWorkloadRepository::is_current_fence(&pool, owned.fence())
            .await
            .unwrap()
    );
    let mut tx = pool.begin().await.unwrap();
    RuntimeAuthorityRepository::revoke_approval(&mut tx, approval.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        SensorWorkloadRepository::is_current_fence(&pool, owned.fence())
            .await
            .is_err()
    );
    let next_issuer = helpers::IdentityFixture::new_unique("next_sensor_issuer")
        .create(&pool)
        .await
        .unwrap();
    PermissionAssignmentRepository::create(
        &*pool,
        CreatePermissionAssignmentInput {
            identity: next_issuer.id,
            permset: issuer_set.id,
        },
    )
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    ManagedSensorMetadataRepository::lock_mutation(&mut tx)
        .await
        .unwrap();
    RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        sensor.pack.unwrap(),
        next_issuer.id,
        normalized,
        None,
    )
    .await
    .unwrap();
    let renewed = SensorRepository::update(
        &mut *tx,
        sensor.id,
        UpdateSensorInput {
            entrypoint: Some("sensors/renewed.py".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ManagedSensorMetadataRepository::readmit(&mut tx, &renewed, editor.id, Some(key))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(SensorWorkloadRepository::release(&pool, owned.fence())
        .await
        .unwrap());
    let AcquireEligibleSensorWorkloadOutcome::Acquired(renewed) =
        SensorWorkloadAdmissionRepository::acquire_with_encryption_key(
            &pool,
            acquire_input(sensor.id, worker.id, Uuid::new_v4()),
            Some(key),
        )
        .await
        .unwrap()
    else {
        panic!("renewed sensor not admitted")
    };
    assert!(
        SensorWorkloadRepository::is_current_fence(&pool, renewed.fence())
            .await
            .unwrap()
    );
    KeyRepository::delete(&*pool, target.id).await.unwrap();
    let replacement =
        helpers::KeyFixture::new_pack(foreign.id, &foreign.r#ref, "signer", "new-fixture-value")
            .create(&pool)
            .await
            .unwrap();
    assert_ne!(target.id, replacement.id);
    assert!(
        SensorWorkloadRepository::is_current_fence(&pool, renewed.fence())
            .await
            .is_err()
    );
    // Rebinding the same selection cannot silently follow the recreated Key.
    let mut tx = pool.begin().await.unwrap();
    ManagedSensorMetadataRepository::lock_mutation(&mut tx)
        .await
        .unwrap();
    let updated = SensorRepository::update(
        &mut *tx,
        sensor.id,
        UpdateSensorInput {
            entrypoint: Some("sensors/changed.py".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        ManagedSensorMetadataRepository::readmit(&mut tx, &updated, editor.id, Some(key))
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    pool.cleanup().await.unwrap();
}

#[tokio::test]
async fn foreign_rule_generation_and_delivery_proofs_pin_the_original_dispatch_policy() {
    use attune_common::repositories::{
        executable_snapshot::ExecutableSnapshotRepository,
        runtime_authority::RuntimeAuthorityRepository,
        sensor_rule_origin::SensorRuleOriginRepository,
        sensor_schema::{
            SensorSchemaRepository, ENTITY_SENSOR_GENERATION_RULE_CONFIG,
            ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
        },
        trigger::TriggerRepository,
    };
    let (pool, sensor, worker, _) = setup_fixture().await;
    let foreign = PackFixture::new_unique("foreign_rule_proof")
        .create(&pool)
        .await
        .unwrap();
    let action = ActionFixture::new(foreign.id, &foreign.r#ref, "receive")
        .with_runtime(sensor.runtime)
        .create(&pool)
        .await
        .unwrap();
    let trigger = TriggerRepository::find_by_sensor(&*pool, sensor.id)
        .await
        .unwrap()
        .remove(0);
    let rule = RuleRepository::create(
        &*pool,
        CreateRuleInput {
            r#ref: format!("{}.subscription", foreign.r#ref),
            pack: foreign.id,
            pack_ref: foreign.r#ref.clone(),
            label: "Foreign subscription".into(),
            description: None,
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({"interval":9}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();
    // H's retained-source table is an explicit integration prerequisite, not a
    // reason to invent foreign ownership from the current pack-ref spelling.
    let available: bool =
        sqlx::query_scalar("SELECT to_regclass('managed_dispatch_metadata_revision') IS NOT NULL")
            .fetch_one(&*pool)
            .await
            .unwrap();
    assert!(
        available,
        "requires H migration 20261015008001 in the combined source"
    );
    let mut tx = pool.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: foreign.id,
            pack_ref: foreign.r#ref.clone(),
            version: "1.0.0".into(),
            digest: "c".repeat(64),
            object_key: "packs/blobs/foreign-rule-proof.tar.gz".into(),
            provider_version: "e:foreign-rule-proof".into(),
            content_path: "/packs/.releases/foreign-rule-proof".into(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(
        &mut tx,
        foreign.id,
        release.id,
        &PackProjectionIds {
            actions: vec![action.id],
            rules: vec![rule.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let owner = RuntimeAuthorityRepository::ensure_owner(&mut tx, foreign.id)
        .await
        .unwrap();
    let source = fixture_rule_dispatch_source(&rule, &trigger, &sensor);
    let hash = fixture_digest(&source);
    let executable = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id)
        .await
        .unwrap();
    let mut invocation = attune_common::runtime_authority::InvocationPolicy::from_executable(
        attune_common::runtime_authority::InvocationBinding::Rule {
            rule_ref: rule.r#ref.clone(),
            action_ref: action.r#ref.clone(),
        },
        &executable,
        Some(&[]),
        &Default::default(),
    )
    .unwrap();
    invocation.source_id = Some(rule.id);
    invocation.dispatch_template_digest = Some(hash.clone());
    let policy = attune_common::runtime_authority::RuntimePolicy {
        owner,
        release_id: release.id,
        release_digest: release.digest.clone(),
        invocations: vec![invocation],
        concrete_targets: Default::default(),
    };
    let admitted = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    let retained: (i64, serde_json::Value, String) = sqlx::query_as(
        "SELECT source_release,metadata,metadata_digest FROM managed_dispatch_metadata_revision WHERE policy_revision=$1 AND component_kind='rules' AND component_id=$2",
    ).bind(admitted.id).bind(rule.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(retained, (release.id, source, hash));
    RuntimeAuthorityRepository::activate_policy(&mut tx, admitted.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let key = "0123456789abcdef0123456789abcdef";
    let AcquireEligibleSensorWorkloadOutcome::Acquired(owned) =
        SensorWorkloadAdmissionRepository::acquire_with_encryption_key(
            &pool,
            acquire_input(sensor.id, worker.id, Uuid::new_v4()),
            Some(key),
        )
        .await
        .unwrap()
    else {
        panic!("sensor not admitted")
    };
    let (id, binding, _) =
        SensorSchemaRepository::find(&*pool, owned.workload_id, owned.generation)
            .await
            .unwrap()
            .unwrap();
    let proof =
        SensorRuleOriginRepository::find(&*pool, ENTITY_SENSOR_GENERATION_RULE_CONFIG, id, rule.id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(proof.managing_pack_id, foreign.id);
    assert_eq!(proof.rule_id, rule.id);
    assert_eq!(
        proof.parameter_contract_digest.as_deref(),
        Some(
            binding.triggers[&trigger.r#ref]
                .parameters
                .as_ref()
                .unwrap()
                .binding_digest
                .as_str()
        )
    );
    assert_eq!(proof.dispatch_policy_revision, Some(admitted.id));
    proof.validate_binding(id, &binding).unwrap();
    assert!(
        proof.validate_binding(id + 1, &binding).is_err(),
        "a retained foreign proof cannot authorize another generation"
    );
    let mut forged = rule.clone();
    forged.pack = sensor.pack.unwrap();
    forged.pack_ref = sensor.pack_ref.clone().unwrap();
    let mut rejected = pool.begin().await.unwrap();
    let error = SensorSchemaRepository::capture_rule_delivery(
        &mut rejected,
        id,
        &binding,
        &forged,
        Some(key),
    )
    .await
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("Sensor delivery rule changed before capture"));
    rejected.rollback().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    SensorSchemaRepository::capture_rule_delivery(&mut tx, id, &binding, &rule, Some(key))
        .await
        .unwrap();
    let delivery: i64 = sqlx::query_scalar(
        "SELECT id FROM sensor_rule_config_delivery WHERE generation_binding=$1 AND rule=$2",
    )
    .bind(id)
    .bind(rule.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let delivery_proof = SensorRuleOriginRepository::find(
        &*pool,
        ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
        delivery,
        rule.id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(delivery_proof.managing_pack_id, foreign.id);
    let mut next = policy.clone();
    next.invocations.clear();
    let mut tx = pool.begin().await.unwrap();
    let replacement = RuntimeAuthorityRepository::capture_policy(&mut tx, &next)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, replacement.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_ne!(replacement.id, admitted.id);
    let retained =
        SensorRuleOriginRepository::find(&*pool, ENTITY_SENSOR_GENERATION_RULE_CONFIG, id, rule.id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(retained.managing_pack_id, foreign.id);
    assert_eq!(retained.dispatch_policy_revision, Some(admitted.id));
    origin_batches::assert_foreign_batch_budgets(
        &pool,
        &retained,
        replacement.id,
        binding.executable.release.id,
    )
    .await
    .unwrap();
    origin_batches::assert_binding_batch_budgets(&pool, id)
        .await
        .unwrap();
    assert!(
        sqlx::query("DELETE FROM pack_runtime_policy_revision WHERE id=$1")
            .bind(admitted.id)
            .execute(&*pool)
            .await
            .is_err()
    );
    assert!(SensorRuleOriginRepository::find(
        &*pool,
        ENTITY_SENSOR_GENERATION_RULE_CONFIG,
        id,
        rule.id + 1
    )
    .await
    .unwrap()
    .is_none());
    pool.cleanup().await.unwrap();
}

#[tokio::test]
async fn managed_sensor_readmission_pins_code_and_old_generations_with_separate_local_purposes() {
    use attune_common::repositories::{
        component_configuration::ComponentConfigurationRepository,
        identity::{
            CreatePermissionSetInput, IdentityRepository, PermissionSetRepository,
            UpdateIdentityInput,
        },
        managed_sensor_metadata::{ManagedSensorMetadataRepository, SensorPurposeAuthority},
        sensor_schema::SensorSchemaRepository,
        trigger::{SensorRepository, UpdateSensorInput},
        Update,
    };
    let (pool, sensor, worker, _) =
        setup_fixture_with_schema(json!({"count":{"type":"integer"}}), json!({"count":1})).await;
    create_active_rule(&pool, &sensor).await;
    let editor = helpers::IdentityFixture::new_unique("sensor_editor_without_keys")
        .create(&pool)
        .await
        .unwrap();
    let pack = sensor.pack.unwrap();
    let pack_ref = sensor.pack_ref.clone().unwrap();
    let set = PermissionSetRepository::create(&*pool,CreatePermissionSetInput {
        r#ref:format!("{pack_ref}.sensor_capabilities"),pack:Some(pack),pack_ref:Some(pack_ref.clone()),label:None,description:None,
        grants:json!([{"resource":"keys","actions":["read","decrypt","use"]},{"resource":"caches","actions":["read"]}]),
    }).await.unwrap();
    let key = "0123456789abcdef0123456789abcdef";
    let mut tx = pool.begin().await.unwrap();
    ManagedSensorMetadataRepository::lock_mutation(&mut tx)
        .await
        .unwrap();
    let current = SensorRepository::find_by_id(&mut *tx, sensor.id)
        .await
        .unwrap()
        .unwrap();
    ComponentConfigurationRepository::bind_sensor(
        &mut tx,
        &current,
        current.config.as_ref(),
        Some(key),
    )
    .await
    .unwrap();
    let old_revision =
        ManagedSensorMetadataRepository::readmit(&mut tx, &current, editor.id, Some(key))
            .await
            .unwrap();
    tx.commit().await.unwrap();
    let AcquireEligibleSensorWorkloadOutcome::Acquired(old) =
        SensorWorkloadAdmissionRepository::acquire_with_encryption_key(
            &pool,
            acquire_input(sensor.id, worker.id, Uuid::new_v4()),
            Some(key),
        )
        .await
        .unwrap()
    else {
        panic!("old generation not admitted")
    };
    let (_, old_binding, old_config) =
        SensorSchemaRepository::find(&*pool, old.workload_id, old.generation)
            .await
            .unwrap()
            .unwrap();
    let mut tx = pool.begin().await.unwrap();
    ManagedSensorMetadataRepository::lock_mutation(&mut tx)
        .await
        .unwrap();
    let changed = SensorRepository::update(
        &mut *tx,
        sensor.id,
        UpdateSensorInput {
            entrypoint: Some("sensors/edited.py".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let patch = json!({"count":"{{ system.sensor.id }}","input_permission_set_refs":[set.r#ref],
        "signing_permission_set_refs":[set.r#ref],"cache_permission_set_refs":[set.r#ref]});
    ComponentConfigurationRepository::bind_sensor_readmission(
        &mut tx,
        &changed,
        Some(&patch),
        &[],
        Some(key),
    )
    .await
    .unwrap();
    let new_revision =
        ManagedSensorMetadataRepository::readmit(&mut tx, &changed, editor.id, Some(key))
            .await
            .unwrap();
    assert_ne!(old_revision, new_revision);
    let metadata = ManagedSensorMetadataRepository::find(&mut tx, new_revision)
        .await
        .unwrap();
    assert_eq!(
        metadata.executable.release.id,
        old_binding.executable.release.id
    );
    assert_eq!(
        metadata.executable.release.digest,
        old_binding.executable.release.digest
    );
    assert!(metadata.executable.sensor.config.is_none());
    assert!(metadata
        .executable
        .input_grants
        .iter()
        .all(|grant| grant.actions.iter().all(|action| matches!(
            action,
            attune_common::rbac::Action::Read | attune_common::rbac::Action::Decrypt
        ))));
    assert!(metadata
        .executable
        .signing_grants
        .iter()
        .all(|grant| grant.actions == [attune_common::rbac::Action::Use]));
    assert!(metadata
        .executable
        .cache_grants
        .iter()
        .all(|grant| grant.resource == attune_common::rbac::Resource::Caches));
    assert!(metadata.executable.input_grants.iter().all(|grant| grant
        .constraints
        .as_ref()
        .unwrap()
        .managing_pack_id
        == Some(pack)));
    assert!(
        matches!(&metadata.purposes["input"],SensorPurposeAuthority::Declared { sets } if sets.contains_key(&set.r#ref))
    );
    tx.commit().await.unwrap();
    assert!(
        SensorWorkloadRepository::is_current_fence(&pool, old.fence())
            .await
            .unwrap()
    );
    let renewed = SensorWorkloadRepository::renew(
        &pool,
        SensorWorkloadLeaseInput {
            fence: old.fence(),
            lease_seconds: LEASE_SECONDS,
        },
    )
    .await
    .unwrap()
    .unwrap();
    let renewed: attune_common::models::SensorExecutableSnapshot =
        serde_json::from_value(renewed.executable_snapshot.unwrap()).unwrap();
    assert_eq!(
        renewed.sensor.entrypoint,
        old_binding.executable.sensor.entrypoint
    );
    assert_eq!(old_config["count"], 1);
    assert!(SensorWorkloadRepository::release(&pool, old.fence())
        .await
        .unwrap());
    let AcquireEligibleSensorWorkloadOutcome::Acquired(new) =
        SensorWorkloadAdmissionRepository::acquire_with_encryption_key(
            &pool,
            acquire_input(sensor.id, worker.id, Uuid::new_v4()),
            Some(key),
        )
        .await
        .unwrap()
    else {
        panic!("new generation not admitted")
    };
    let (new_binding_id, new_binding, new_config) =
        SensorSchemaRepository::find(&*pool, new.workload_id, new.generation)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        new_binding.executable.sensor.entrypoint,
        "sensors/edited.py"
    );
    assert_eq!(new_config["count"], sensor.id);
    origin_batches::assert_metadata_binding(&pool, new_binding_id, new_revision)
        .await
        .unwrap();
    IdentityRepository::update(
        &*pool,
        editor.id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        SensorWorkloadRepository::is_current_fence(&pool, new.fence())
            .await
            .unwrap()
    );
    let mut tx = pool.begin().await.unwrap();
    ManagedSensorMetadataRepository::revoke(&mut tx, sensor.id, Some(new_revision))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        SensorWorkloadRepository::is_current_fence(&pool, new.fence())
            .await
            .is_err()
    );
    let (_, retained, _) = SensorSchemaRepository::find(&*pool, old.workload_id, old.generation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retained.executable.sensor.entrypoint,
        old_binding.executable.sensor.entrypoint
    );
    assert_eq!(
        retained.executable.input_grants,
        old_binding.executable.input_grants
    );
    pool.cleanup().await.unwrap();
}

#[tokio::test]
async fn rollout_inventory_follows_live_admitted_sensor_generation_not_mutable_sensor_relation() {
    use attune_common::repositories::{
        sensor_schema::SensorSchemaRepository,
        trigger::{TriggerRepository, UpdateTriggerInput},
        trigger_rollout::TriggerRolloutRepository,
        Patch,
    };
    let (pool, sensor, worker, _) = setup_fixture().await;
    create_active_rule(&pool, &sensor).await;
    let outcome = SensorWorkloadAdmissionRepository::acquire_with_encryption_key(
        &pool,
        acquire_input(sensor.id, worker.id, Uuid::new_v4()),
        Some("0123456789abcdef0123456789abcdef"),
    )
    .await
    .unwrap();
    let AcquireEligibleSensorWorkloadOutcome::Acquired(workload) = outcome else {
        panic!("sensor generation was not admitted")
    };
    let (_, binding, _) =
        SensorSchemaRepository::find(&*pool, workload.workload_id, workload.generation)
            .await
            .unwrap()
            .unwrap();
    let admitted = binding
        .executable
        .triggers
        .first()
        .expect("admitted trigger");
    let mut tx = pool.begin().await.unwrap();
    TriggerRolloutRepository::lock_activation(&mut tx)
        .await
        .unwrap();
    // A mutable association cannot hide a still-live generation that retains this trigger.
    let trigger = TriggerRepository::update(
        &mut *tx,
        admitted.id,
        UpdateTriggerInput {
            sensor: Some(Patch::Clear),
            sensor_ref: Some(Patch::Clear),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(trigger.sensor.is_none());
    let unchanged = TriggerRolloutRepository::preflight(
        &mut tx,
        trigger.id,
        trigger.out_schema.as_ref(),
        trigger.event_config_schema.as_ref(),
    )
    .await
    .unwrap();
    assert!(!unchanged.requires_acknowledgement());
    let changed = json!({"new_required":{"type":"integer","required":true}});
    let impact = TriggerRolloutRepository::preflight(
        &mut tx,
        trigger.id,
        Some(&changed),
        trigger.event_config_schema.as_ref(),
    )
    .await
    .unwrap();
    assert_eq!(impact.workload_ids, [workload.workload_id]);
    assert!(impact.execution_ids.is_empty());
    assert!(impact.require_drained_or_acknowledged(false).is_err());
    assert!(impact.require_drained_or_acknowledged(true).is_ok());
    // The candidate graph hook must fence resolved named dependencies as well.
    let named = TriggerRolloutRepository::preflight_changed_contract(&mut tx, trigger.id)
        .await
        .unwrap();
    assert_eq!(named.workload_ids, [workload.workload_id]);
    assert!(named.require_drained_or_acknowledged(false).is_err());
    tx.commit().await.unwrap();
    assert!(SensorWorkloadRepository::release(&pool, workload.fence())
        .await
        .unwrap());
    let mut tx = pool.begin().await.unwrap();
    TriggerRolloutRepository::lock_activation(&mut tx)
        .await
        .unwrap();
    let drained = TriggerRolloutRepository::preflight_changed_contract(&mut tx, trigger.id)
        .await
        .unwrap();
    assert!(!drained.requires_acknowledgement());
    tx.rollback().await.unwrap();
    pool.cleanup().await.unwrap();
}

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
    assert!(error
        .to_string()
        .contains("Sensor delivery rule changed before capture"));
    assert!(!error.to_string().contains("updated-rule-fixture"));
    rejected.rollback().await.unwrap();
    // A persisted literal must still reach the secret-template validator, rather
    // than passing this negative case only because the supplied row is stale.
    let mut rejected = pool.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(&mut rejected)
        .await
        .unwrap();
    sqlx::query("UPDATE rule SET trigger_params = $1 WHERE id = $2")
        .bind(&changed.trigger_params)
        .bind(rule_id)
        .execute(&mut *rejected)
        .await
        .unwrap();
    let invalid = RuleRepository::find_by_id(&mut *rejected, rule_id)
        .await
        .unwrap()
        .unwrap();
    let error = SensorSchemaRepository::capture_rule_delivery(
        &mut rejected,
        id,
        &binding,
        &invalid,
        Some(key),
    )
    .await
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("Sensor rule secret parameters require whole Key templates"));
    assert!(!error.to_string().contains("updated-rule-fixture"));
    rejected.rollback().await.unwrap();
    sqlx::query("UPDATE key SET value = $1 WHERE id = $2")
        .bind(attune_common::crypto::encrypt_json(&json!("updated-rule-fixture"), key).unwrap())
        .bind(input_key.id)
        .execute(&*pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    // The bootstrap above deliberately cleared the live rule to prove history
    // was pinned. Delivery must select a REAL persisted replacement, not mutate
    // an in-memory copy of that cleared row.
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(&mut tx)
        .await
        .unwrap();
    sqlx::query("UPDATE rule SET trigger_params = $1 WHERE id = $2")
        .bind(json!({"credential":"{{ keystore['system.sensor_input'] }}"}))
        .bind(rule_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let changed = RuleRepository::find_by_id(&mut *tx, rule_id)
        .await
        .unwrap()
        .unwrap();
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
    use attune_common::{
        models::enums::{
            ArtifactClassification, ArtifactType, ArtifactVisibility, LogStreamBackend, OwnerType,
            RetentionPolicyType,
        },
        repositories::artifact::{
            ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
        },
    };
    let log_ref = format!("sensor.{}.stdout", sensor.r#ref);
    let log = ArtifactRepository::create(
        &*pool,
        CreateArtifactInput {
            r#ref: log_ref.clone(),
            scope: OwnerType::Sensor,
            owner: sensor.r#ref.clone(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::RuntimeLog,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 1,
            name: None,
            description: None,
            content_type: Some("text/plain".into()),
            data: None,
        },
    )
    .await
    .unwrap();
    let log_version = ArtifactVersionRepository::create_log_pending(
        &*pool,
        log.id,
        &log_ref,
        LogStreamBackend::SharedFile,
        "text/plain".into(),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    SensorSchemaRepository::attach_log_version(&mut tx, log_version.id, id)
        .await
        .unwrap();
    SensorSchemaRepository::attach_current_log_deliveries(&mut tx, log_version.id, id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        SensorSchemaRepository::log_delivery_origins(&*pool, log_version.id)
            .await
            .unwrap(),
        vec![stored[0].0]
    );
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
    let mut tx = pool.begin().await.unwrap();
    SensorSchemaRepository::attach_current_log_deliveries(&mut tx, log_version.id, id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        SensorSchemaRepository::log_delivery_origins(&*pool, log_version.id)
            .await
            .unwrap(),
        paths
            .iter()
            .map(|(delivery, _)| *delivery)
            .collect::<Vec<_>>()
    );
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
    let mut tx = pool.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(&mut tx)
        .await
        .unwrap();
    sqlx::query("UPDATE rule SET trigger_params = $1 WHERE id = $2")
        .bind(json!({"credential":"{{ keystore['system.sensor_input_alt'] }}"}))
        .bind(rule_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let changed = RuleRepository::find_by_id(&mut *tx, rule_id)
        .await
        .unwrap()
        .unwrap();
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
    let mut tx = pool.begin().await.unwrap();
    SensorSchemaRepository::attach_current_log_deliveries(&mut tx, log_version.id, id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        SensorSchemaRepository::log_delivery_origins(&*pool, log_version.id)
            .await
            .unwrap(),
        paths
            .iter()
            .map(|(delivery, _)| *delivery)
            .collect::<Vec<_>>()
    );
    let rules =
        SensorSchemaRepository::restore_initial_rules(&mut connection, id, &binding, Some(key))
            .await
            .unwrap();
    assert_eq!(
        rules[0]["config"],
        json!({"credential":"private-rule-fixture","interval":5})
    );
    drop(connection);
    pool.cleanup().await.unwrap();
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
