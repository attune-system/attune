//! C's structural consumer against D's real producer and H's retained dispatch.
use super::*;
use attune_common::{
    repositories::{
        executable_snapshot::ExecutableSnapshotRepository,
        runtime_authority::RuntimeAuthorityRepository,
        runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
        sensor_read_origin::SensorOriginFrontier,
        sensor_schema::{
            SensorSchemaRepository, ENTITY_SENSOR_GENERATION_RULE_CONFIG,
            ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
        },
        trigger::TriggerRepository,
    },
    runtime_authority::{InvocationBinding, InvocationPolicy, RuntimePolicy},
};

#[tokio::test]
async fn generic_foreign_rule_proof_accepts_original_producer_after_policy_replacement_not_missing_or_mismatched_sources(
) {
    let (pool, sensor, worker, _) = setup_fixture().await;
    let foreign = PackFixture::new_unique("c_foreign_proof")
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
            object_key: "packs/blobs/c-foreign-proof.tar.gz".into(),
            provider_version: "e:c-foreign-proof".into(),
            content_path: "/packs/.releases/c-foreign-proof".into(),
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
    let source = super::fixture_rule_dispatch_source(&rule, &trigger, &sensor);
    let hash = super::fixture_digest(&source);
    let executable = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id)
        .await
        .unwrap();
    let mut invocation = InvocationPolicy::from_executable(
        InvocationBinding::Rule {
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
    let policy = RuntimePolicy {
        owner: owner.clone(),
        release_id: release.id,
        release_digest: release.digest.clone(),
        invocations: vec![invocation],
        concrete_targets: Default::default(),
    };
    let admitted = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    // The production writer must already have captured this exact source in
    // policy creation. Do not repair missing evidence with a post-hoc INSERT.
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
        panic!("sensor not admitted");
    };
    let (generation, binding, _) =
        SensorSchemaRepository::find(&*pool, owned.workload_id, owned.generation)
            .await
            .unwrap()
            .unwrap();
    let mut tx = pool.begin().await.unwrap();
    SensorSchemaRepository::capture_rule_delivery(&mut tx, generation, &binding, &rule, Some(key))
        .await
        .unwrap();
    let delivery: i64 = sqlx::query_scalar(
        "SELECT id FROM sensor_rule_config_delivery WHERE generation_binding=$1 AND rule=$2",
    )
    .bind(generation)
    .bind(rule.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let mut replacement = policy.clone();
    replacement.invocations.clear();
    let replacement = RuntimeAuthorityRepository::capture_policy(&mut tx, &replacement)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, replacement.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let nodes = vec![
        (
            ENTITY_SENSOR_GENERATION_RULE_CONFIG.to_owned(),
            generation,
            format!("/{}/config/interval", rule.id),
        ),
        (
            ENTITY_SENSOR_RULE_DELIVERY_CONFIG.to_owned(),
            delivery,
            "/interval".into(),
        ),
    ];
    let mut frontier = SensorOriginFrontier::default();
    frontier.prepare(&mut tx, &nodes).await.unwrap();
    frontier.prepare(&mut tx, &nodes).await.unwrap();
    let original = frontier
        .validated_binding(&mut tx, generation)
        .await
        .unwrap();
    assert_eq!(
        original.binding_digest().unwrap(),
        binding.binding_digest().unwrap()
    );
    assert_eq!(
        frontier.initial_rule(generation, rule.id).unwrap().1,
        rule.r#ref
    );
    assert_eq!(frontier.delivery(delivery).unwrap().0, generation);
    let target = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Component(ComponentTarget::Rule, rule.id),
        owner.sponsor_identity_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(target.context().managing_pack_id, Some(foreign.id));
    for (entity, source, _) in &nodes {
        assert!(frontier
            .rule_owner_matches(
                entity,
                *source,
                rule.id,
                &rule.r#ref,
                &rule.pack_ref,
                generation,
                &original,
                &target,
            )
            .unwrap());
        assert!(!frontier
            .rule_owner_matches(
                entity,
                *source,
                rule.id,
                "same-id-wrong-ref",
                &rule.pack_ref,
                generation,
                &original,
                &target,
            )
            .unwrap());
        assert!(frontier
            .rule_owner_matches(
                entity,
                *source,
                rule.id,
                &rule.r#ref,
                &rule.pack_ref,
                generation + 1,
                &original,
                &target,
            )
            .is_err());
    }
    // Structural evidence never grants disclosure by itself.
    let no_grants = attune_common::delegation::DelegationAuthority::new(
        owner.sponsor_identity_id,
        Default::default(),
        vec![],
    );
    assert!(!no_grants.allows_input_origin_target(&target, attune_common::rbac::Action::Read));
    assert!(!no_grants.allows_input_origin_target(&target, attune_common::rbac::Action::Decrypt));
    tx.rollback().await.unwrap();

    sqlx::query("DELETE FROM sensor_rule_structural_origin WHERE source_entity=$1 AND source_id=$2 AND rule_id=$3")
        .bind(ENTITY_SENSOR_RULE_DELIVERY_CONFIG).bind(delivery).bind(rule.id).execute(&*pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let mut missing = SensorOriginFrontier::default();
    missing.prepare(&mut tx, &nodes[1..]).await.unwrap();
    let original = missing
        .validated_binding(&mut tx, generation)
        .await
        .unwrap();
    let target = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Component(ComponentTarget::Rule, rule.id),
        owner.sponsor_identity_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!missing
        .rule_owner_matches(
            ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
            delivery,
            rule.id,
            &rule.r#ref,
            &rule.pack_ref,
            generation,
            &original,
            &target,
        )
        .unwrap());
    tx.rollback().await.unwrap();
    pool.cleanup().await.unwrap();
}
