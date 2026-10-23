mod helpers;
mod managed_readmission_fixture;

use attune_common::{
    models::{WorkerStatus, WorkerType},
    repositories::{
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        managed_component_readmission::ManagedComponentReadmission,
        managed_executable::ManagedExecutableRepository,
        rule::RuleRepository,
        runtime::{CreateWorkerInput, WorkerRepository},
        runtime_authority::RuntimeAuthorityRepository,
        schema_type::SchemaTypeRepository,
        sensor_rule_origin::SensorRuleOriginRepository,
        sensor_schema::{
            SensorSchemaRepository, ENTITY_SENSOR_GENERATION_RULE_CONFIG,
            ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
        },
        sensor_workload::AcquireSensorWorkloadInput,
        sensor_workload_admission::{
            AcquireEligibleSensorWorkloadOutcome, SensorWorkloadAdmissionRepository,
        },
        trigger::SensorRepository,
        Create, FindByRef,
    },
    runtime_authority::InvocationBinding,
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::{json, Value};

fn write_source(
    ctx: &TestContext,
    pack: &str,
    files: &[(&str, &str)],
) -> Result<std::path::PathBuf> {
    let source = ctx.test_packs_dir.join(pack);
    std::fs::create_dir_all(source.join("runtimes"))?;
    std::fs::write(
        source.join("pack.yaml"),
        format!("ref: {pack}\nversion: '1.0.0'\nconf_schema: {{}}\nconfig: {{}}\n"),
    )?;
    std::fs::write(
        source.join("runtimes/native.yaml"),
        format!("ref: {pack}.native\nname: {pack}_native\nexecution_config: {{}}\n"),
    )?;
    for (file, body) in files {
        let path = source.join(file);
        std::fs::create_dir_all(path.parent().ok_or("source has no parent")?)?;
        std::fs::write(&path, body)?;
        #[cfg(unix)]
        if body.starts_with("#!") {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(source)
}

async fn dispatch_rows(
    connection: &mut sqlx::PgConnection,
    policy: i64,
) -> Result<Vec<(String, i64, i64, Value, String)>> {
    Ok(sqlx::query_as("SELECT component_kind,component_id,source_release,metadata,metadata_digest FROM managed_dispatch_metadata_revision WHERE policy_revision=$1 ORDER BY component_kind,component_id")
        .bind(policy).fetch_all(connection).await?)
}

fn metadata_digest(value: &Value) -> Result<String> {
    use sha2::Digest;
    Ok(sha2::Sha256::digest(serde_jcs::to_vec(value)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[tokio::test]
async fn fresh_deployment_prepares_foreign_sensor_dispatch_and_retains_exact_policy_history(
) -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let actor = ctx.user.as_ref().ok_or("actor missing")?.id;
    let sensor_pack = helpers::create_test_pack(&ctx.pool, "fresh_sensor").await?;
    let sensor_source = write_source(&ctx, "fresh_sensor", &[
        ("actions/sink.yaml", "ref: fresh_sensor.sink\nrunner_type: fresh_sensor.native\nentry_point: sink.sh\n"),
        ("actions/sink.sh", "#!/bin/sh\nexit 0\n"),
        ("sensors/source.yaml", "ref: fresh_sensor.source\nrunner_type: fresh_sensor.native\nentry_point: source.sh\ntrigger_types: [fresh_sensor.tick]\n"),
        ("sensors/source.sh", "#!/bin/sh\nexit 0\n"),
        ("triggers/tick.yaml", "ref: fresh_sensor.tick\nreference_visibility: public\nparam_schema:\n  note:\n    type: string\n"),
    ])?;
    managed_readmission_fixture::deploy(&ctx, &sensor_pack, &sensor_source, actor).await?;
    let foreign = helpers::create_test_pack(&ctx.pool, "fresh_dispatch").await?;
    let foreign_source = write_source(&ctx, "fresh_dispatch", &[
        ("actions/receive.yaml", "ref: fresh_dispatch.receive\nrunner_type: fresh_dispatch.native\nentry_point: receive.sh\nparameters:\n  message:\n    type: string\n"),
        ("actions/receive.sh", "#!/bin/sh\nexit 0\n"),
        ("actions/other.yaml", "ref: fresh_dispatch.other\nrunner_type: fresh_dispatch.native\nentry_point: receive.sh\n"),
        ("rules/first.yaml", "ref: fresh_dispatch.first\ntrigger_ref: fresh_sensor.tick\naction_ref: fresh_dispatch.receive\npermission_set_refs: []\ntrigger_params:\n  note: first\n"),
        ("rules/second.yaml", "ref: fresh_dispatch.second\ntrigger_ref: fresh_sensor.tick\naction_ref: fresh_dispatch.receive\npermission_set_refs: []\ntrigger_params:\n  note: second\n"),
        ("queues/jobs.yaml", "ref: fresh_dispatch.jobs\nlabel: Jobs\ndispatch_action: fresh_dispatch.receive\npermission_set_refs: []\n"),
    ])?;
    let original =
        managed_readmission_fixture::deploy(&ctx, &foreign, &foreign_source, actor).await?;
    let rule = RuleRepository::find_by_ref(&ctx.pool, "fresh_dispatch.first")
        .await?
        .ok_or("rule missing")?;
    let mut tx = ctx.pool.begin().await?;
    SchemaTypeRepository::lock_reclamation(&mut tx).await?;
    let original_rows = dispatch_rows(&mut tx, original.id).await?;
    let policy_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pack_runtime_policy_revision WHERE pack=$1")
            .bind(foreign.id)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        policy_count, 1,
        "deployment must not readmit once per dispatch source"
    );
    let input_revision: i64 =
        sqlx::query_scalar("SELECT revision FROM rule_runtime_authority_current WHERE rule=$1")
            .bind(rule.id)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        original_rows.len(),
        3,
        "every rule and queue must have a deployment body"
    );
    for invocation in &original.policy.invocations {
        assert!(
            invocation.metadata_revision.is_some(),
            "fresh executable metadata was not prepared"
        );
        let target = ManagedExecutableRepository::resolve_invocation(
            &mut tx,
            &original.policy,
            &invocation.binding,
        )
        .await?;
        assert_eq!(target.release.id, invocation.target_release_id);
        assert_eq!(target.release.digest, invocation.target_release_digest);
        if matches!(
            invocation.binding,
            InvocationBinding::Rule { .. } | InvocationBinding::Queue { .. }
        ) {
            let row = original_rows
                .iter()
                .find(|row| {
                    Some(row.1) == invocation.source_id
                        && row.0
                            == if matches!(invocation.binding, InvocationBinding::Rule { .. }) {
                                "rules"
                            } else {
                                "queues"
                            }
                })
                .ok_or("dispatch row missing")?;
            assert_eq!(Some(&row.4), invocation.dispatch_template_digest.as_ref());
            assert_eq!(row.2, original.policy.release_id);
            assert_eq!(metadata_digest(&row.3)?, row.4);
        }
    }
    ManagedComponentReadmission::capture_deployment_dispatch(&mut tx, &original).await?;
    assert_eq!(
        dispatch_rows(&mut tx, original.id).await?,
        original_rows,
        "idempotent producer changed history"
    );
    tx.rollback().await?;

    let sensor = SensorRepository::find_by_ref(&ctx.pool, "fresh_sensor.source")
        .await?
        .ok_or("sensor missing")?;
    let worker = WorkerRepository::create(
        &ctx.pool,
        CreateWorkerInput {
            name: "fresh-dispatch-sensor-worker".into(),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: Some(json!({"runtimes":["fresh_sensor_native"]})),
            meta: None,
        },
    )
    .await?;
    sqlx::query(
        "UPDATE worker SET worker_role='sensor',last_heartbeat=clock_timestamp() WHERE id=$1",
    )
    .bind(worker.id)
    .execute(&ctx.pool)
    .await?;
    // Real workload/generation admission must capture the fresh FOREIGN source
    // immediately. No ManagedComponentReadmission::rule call repairs deployment.
    let outcome = SensorWorkloadAdmissionRepository::acquire_with_encryption_key(
        &ctx.pool,
        AcquireSensorWorkloadInput {
            sensor_id: sensor.id,
            worker_id: worker.id,
            worker_instance: uuid::Uuid::new_v4(),
            lease_seconds: 300,
        },
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    let AcquireEligibleSensorWorkloadOutcome::Acquired(owned) = outcome else {
        return Err(format!("fresh sensor admission failed: {outcome:?}").into());
    };
    let (generation, binding, _) =
        SensorSchemaRepository::find(&ctx.pool, owned.workload_id, owned.generation)
            .await?
            .ok_or("generation missing")?;
    let initial = SensorRuleOriginRepository::find(
        &ctx.pool,
        ENTITY_SENSOR_GENERATION_RULE_CONFIG,
        generation,
        rule.id,
    )
    .await?
    .ok_or("fresh initial rule proof missing")?;
    assert_eq!(initial.dispatch_policy_revision, Some(original.id));
    assert_eq!(initial.managing_pack_id, foreign.id);
    assert_eq!(initial.sensor_managing_pack_id, sensor_pack.id);
    initial.validate_binding(generation, &binding)?;
    let mut tx = ctx.pool.begin().await?;
    SensorSchemaRepository::capture_rule_delivery(
        &mut tx,
        generation,
        &binding,
        &rule,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    let delivery: i64 = sqlx::query_scalar(
        "SELECT id FROM sensor_rule_config_delivery WHERE generation_binding=$1 AND rule=$2",
    )
    .bind(generation)
    .bind(rule.id)
    .fetch_one(&mut *tx)
    .await?;
    let proof = SensorRuleOriginRepository::find(
        &mut *tx,
        ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
        delivery,
        rule.id,
    )
    .await?
    .ok_or("fresh delivery proof missing")?;
    assert_eq!(proof.dispatch_policy_revision, Some(original.id));
    assert_eq!(
        proof.dispatch_source_release,
        Some(original.policy.release_id)
    );
    proof.validate_binding(generation, &binding)?;
    let proof_body = serde_json::to_value(&proof)?;
    tx.commit().await?;

    // A real action edit replaces current policy while retaining every dispatch
    // body and the already produced sensor proof under its ORIGINAL policy.
    let rights = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "fresh_dispatch.editor".into(), pack: Some(foreign.id), pack_ref: Some(foreign.r#ref.clone()), label: None, description: None,
        grants: json!([{"resource":"actions","actions":["update"],"constraints":{"refs":["fresh_dispatch.receive"]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: actor,
            permset: rights.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(actor).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let response = ctx
        .put(
            "/api/v1/actions/fresh_dispatch.receive",
            json!({"default_execution_permission_set_refs":["standard"]}),
            ctx.token.as_deref(),
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        response.text().await?
    );
    let mut tx = ctx.pool.begin().await?;
    SchemaTypeRepository::lock_reclamation(&mut tx).await?;
    let current = RuntimeAuthorityRepository::current_policy(&mut tx, foreign.id)
        .await?
        .ok_or("replacement missing")?;
    assert_ne!(current.id, original.id);
    assert_eq!(dispatch_rows(&mut tx, current.id).await?, original_rows);
    assert_eq!(dispatch_rows(&mut tx, original.id).await?, original_rows);
    let retained_input: i64 =
        sqlx::query_scalar("SELECT revision FROM rule_runtime_authority_current WHERE rule=$1")
            .bind(rule.id)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        retained_input, input_revision,
        "dispatch persistence recaptured input authority"
    );
    let retained = SensorRuleOriginRepository::find(
        &mut *tx,
        ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
        delivery,
        rule.id,
    )
    .await?
    .ok_or("retained proof missing")?;
    assert_eq!(serde_json::to_value(retained)?, proof_body);
    RuntimeAuthorityRepository::revoke_policy(&mut tx, original.id).await?;
    tx.commit().await?;
    let mut tx = ctx.pool.begin().await?;
    assert!(
        RuntimeAuthorityRepository::require_policy(&mut tx, original.id)
            .await
            .is_err()
    );
    tx.rollback().await?;
    let mut tx = ctx.pool.begin().await?;
    RuntimeAuthorityRepository::require_policy(&mut tx, current.id).await?;
    assert_eq!(dispatch_rows(&mut tx, original.id).await?, original_rows);
    tx.rollback().await?;

    // A public producer helper may validate existing history, but it cannot
    // repair a historical policy whose retained source evidence was removed.
    let mut tx = ctx.pool.begin().await?;
    SchemaTypeRepository::lock_reclamation(&mut tx).await?;
    sqlx::query("DELETE FROM managed_dispatch_metadata_revision WHERE policy_revision=$1 AND component_kind='rules' AND component_id=$2")
        .bind(current.id).bind(rule.id).execute(&mut *tx).await?;
    let missing = ManagedComponentReadmission::capture_deployment_dispatch(&mut tx, &current).await;
    assert!(missing
        .err()
        .ok_or("historical dispatch evidence was recreated")?
        .to_string()
        .contains("Historical policy cannot acquire deployment dispatch metadata"));
    tx.rollback().await?;

    // An exact target mismatch must fail policy capture, not borrow the source
    // body's old target while following the other action's same-pack name.
    let mut changed = current.policy.0.clone();
    let other = changed.invocations.iter().find(|entry| matches!(&entry.binding, InvocationBinding::Action {action_ref} if action_ref == "fresh_dispatch.other")).ok_or("other action missing")?.clone();
    let dispatch = changed.invocations.iter_mut().find(|entry| matches!(&entry.binding, InvocationBinding::Rule {rule_ref,..} if rule_ref == &rule.r#ref)).ok_or("rule binding missing")?;
    dispatch.binding = InvocationBinding::Rule {
        rule_ref: rule.r#ref.clone(),
        action_ref: "fresh_dispatch.other".into(),
    };
    dispatch.metadata_revision = other.metadata_revision;
    let mut tx = ctx.pool.begin().await?;
    let rejected = RuntimeAuthorityRepository::capture_policy(&mut tx, &changed).await;
    assert!(
        rejected.is_err(),
        "source proof accepted a different exact executable target"
    );
    tx.rollback().await?;
    let mut tx = ctx.pool.begin().await?;
    assert_eq!(
        RuntimeAuthorityRepository::current_policy(&mut tx, foreign.id)
            .await?
            .ok_or("current disappeared")?
            .id,
        current.id
    );
    tx.rollback().await?;
    // Identical rule/queue bodies in a NEW release must pin that newly
    // projected source release, not the first historical matching body digest.
    let replacement_source = ctx.test_packs_dir.join("fresh-dispatch-replacement");
    for file in [
        "pack.yaml",
        "runtimes/native.yaml",
        "actions/receive.yaml",
        "actions/receive.sh",
        "actions/other.yaml",
        "rules/first.yaml",
        "rules/second.yaml",
        "queues/jobs.yaml",
    ] {
        let destination = replacement_source.join(file);
        std::fs::create_dir_all(
            destination
                .parent()
                .ok_or("replacement source has no parent")?,
        )?;
        std::fs::copy(foreign_source.join(file), destination)?;
    }
    std::fs::write(
        replacement_source.join("pack.yaml"),
        "ref: fresh_dispatch\nversion: '1.0.1'\nconf_schema: {}\nconfig: {}\n",
    )?;
    let mut replacement_pack = foreign.clone();
    replacement_pack.version = "1.0.1".into();
    let redeployed =
        managed_readmission_fixture::deploy(&ctx, &replacement_pack, &replacement_source, actor)
            .await?;
    assert_ne!(redeployed.policy.release_id, original.policy.release_id);
    let mut tx = ctx.pool.begin().await?;
    let rows = dispatch_rows(&mut tx, redeployed.id).await?;
    assert_eq!(rows.len(), original_rows.len());
    for (new, old) in rows.iter().zip(&original_rows) {
        assert_eq!(
            (&new.0, new.1, &new.3, &new.4),
            (&old.0, old.1, &old.3, &old.4)
        );
        assert_eq!(new.2, redeployed.policy.release_id);
    }
    assert_eq!(dispatch_rows(&mut tx, original.id).await?, original_rows);
    let retained = SensorRuleOriginRepository::find(
        &mut *tx,
        ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
        delivery,
        rule.id,
    )
    .await?
    .ok_or("redeployment lost original proof")?;
    assert_eq!(serde_json::to_value(retained)?, proof_body);
    tx.rollback().await?;
    ctx.cleanup().await?;
    Ok(())
}
