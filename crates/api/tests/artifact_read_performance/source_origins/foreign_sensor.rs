//! Cold ordinary disclosure budgets using D-produced foreign delivery proofs.
use super::*;

#[tokio::test]
async fn standalone_sensor_log_disclosure_batches_foreign_delivery_owners_origins_and_proofs(
) -> Result<()> {
    use attune_common::repositories::sensor_schema::SensorSchemaRepository;
    use tracing::instrument::WithSubscriber;
    let mut reports = Vec::new();
    let mut scopes = Vec::new();
    for count in [1, 64, 100] {
        let ctx = TestContext::new().await?.with_auth().await?;
        let _ = action(&ctx).await?;
        let deliveries = foreign_sensor_deliveries(&ctx, count, false).await?;
        let (generation, _, _, _) =
            SensorSchemaRepository::find_delivery_owner(&ctx.pool, deliveries[0].0)
                .await?
                .ok_or("delivery missing")?;
        let (binding, _) = SensorSchemaRepository::find_by_binding_id(&ctx.pool, generation)
            .await?
            .ok_or("generation missing")?;
        let sensor_ref = binding.executable.sensor.r#ref;
        let artifact = ArtifactRepository::create(
            &ctx.pool,
            CreateArtifactInput {
                r#ref: format!("sensor.{sensor_ref}.stdout"),
                scope: OwnerType::Sensor,
                owner: sensor_ref.clone(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::RuntimeLog,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 2000,
                name: None,
                description: None,
                content_type: Some("text/plain".into()),
                data: None,
            },
        )
        .await?;
        let version = ArtifactVersionRepository::create(
            &ctx.pool,
            CreateArtifactVersionInput {
                artifact: artifact.id,
                execution: None,
                content_type: Some("text/plain".into()),
                content: None,
                content_json: None,
                file_path: None,
                meta: None,
                created_by: None,
            },
        )
        .await?;
        let mut tx = ctx.pool.begin().await?;
        SensorSchemaRepository::attach_log_version(&mut tx, version.id, generation).await?;
        SensorSchemaRepository::attach_current_log_deliveries(&mut tx, version.id, generation)
            .await?;
        tx.commit().await?;
        let user = attune_api::auth::middleware::AuthenticatedUser {
            claims: attune_common::auth::jwt::validate_token(
                ctx.token().ok_or("token missing")?,
                &ctx.state.jwt_config,
            )?,
        };
        let observer = RequestQueries::default();
        let subscriber = tracing_subscriber::registry().with(observer.clone());
        // First call for this new context, through the real ordinary sensor-log
        // disclosure entry point. No preflight request warms its source cache.
        attune_api::secret_disclosure::authorize_sensor_runtime_log(
            &ctx.state,
            &user,
            version.id,
            &sensor_ref,
        )
        .with_subscriber(subscriber)
        .await
        .map_err(|error| format!("Sensor disclosure with {count} delivery origins: {error}"))?;
        let metrics = observer.0.lock().unwrap().clone();
        assert!(metrics.statements > 0);
        assert_eq!(metrics.ciphertext_reads, 0);
        assert_eq!(metrics.source_payload_reads, 0);
        assert!(
            metrics.application_statements() <= if count <= 64 { 25 } else { 40 },
            "{count}: {}; metrics={}",
            metrics.application_statements(),
            serde_json::to_string(&metrics)?
        );
        scopes.push(metrics.source_scope_reads);
        reports.push(json!({"origins":count,"application_queries":metrics.application_statements(),"driver_catalog_queries":metrics.driver_metadata_reads(),"queries":metrics}));
        ctx.cleanup().await?;
    }
    assert_eq!(scopes[0], scopes[1]);
    save_report("ordinary-sensor-log-foreign-proof-cold-frontiers", &reports)?;
    Ok(())
}

/// Capture real delivery proof, including H's retained dispatch source. Never
/// insert hand-authored proof JSON as the positive test fixture.
async fn foreign_sensor_deliveries(
    ctx: &TestContext,
    count: usize,
    distinct_generations: bool,
) -> Result<Vec<(i64, i64)>> {
    use attune_common::repositories::{
        managed_component_readmission::ManagedComponentReadmission, rule::RuleRepository,
        sensor_schema::SensorSchemaRepository,
    };
    let binding = sensor_binding(ctx).await?;
    let trigger = binding
        .executable
        .triggers
        .first()
        .ok_or("admitted trigger missing")?;
    let mut files = vec![
        (
            "actions/receive.yaml".to_string(),
            "ref: foreign_sensor_budget.receive\nrunner_type: native\nentry_point: receive.sh\n"
                .to_string(),
        ),
        (
            "actions/receive.sh".to_string(),
            "#!/bin/sh\nprintf '{}\\n'\n".to_string(),
        ),
    ];
    for index in 0..count {
        files.push((format!("rules/rule{index}.yaml"), format!(
            "ref: foreign_sensor_budget.rule{index}\ntrigger_ref: {}\naction_ref: foreign_sensor_budget.receive\ntrigger_params:\n  private: foreign origin\n",
            trigger.r#ref
        )));
    }
    let borrowed = files
        .iter()
        .map(|(path, content)| (path.as_str(), content.clone()))
        .collect::<Vec<_>>();
    managed_fixture::deploy(ctx, "foreign_sensor_budget", &borrowed).await?;
    let foreign = PackRepository::find_by_ref(&ctx.pool, "foreign_sensor_budget")
        .await?
        .ok_or("admitted foreign pack missing")?;
    let mut rules = Vec::new();
    for index in 0..count {
        rules.push(
            RuleRepository::find_by_ref(&ctx.pool, &format!("{}.rule{index}", foreign.r#ref))
                .await?
                .ok_or("admitted foreign rule missing")?,
        );
    }
    grant(ctx,"fixture.foreign_sensor_budget",json!([
        {"resource":"triggers","actions":["update","decrypt"],"constraints":{"refs":[binding.executable.sensor.r#ref]}}
    ])).await?;
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("{}.ordinary_reader", foreign.r#ref),
            pack: Some(foreign.id),
            pack_ref: Some(foreign.r#ref.clone()),
            label: None,
            description: None,
            grants: json!([{"resource":"rules","actions":["read","decrypt"]}]),
        },
    )
    .await?;
    let identity = ctx.user.as_ref().ok_or("identity missing")?.id;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await?;
    AuthorizationService::invalidate_identity_authz_cache(identity).await;
    AuthorizationService::invalidate_permission_set_caches().await;
    let mut result = Vec::new();
    let mut shared_generation = None;
    for (index, rule) in rules.iter().enumerate() {
        let mut tx = ctx.pool.begin().await?;
        attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
            .await?;
        let generation = if let Some(generation) = shared_generation {
            generation
        } else {
            let generation = managed_fixture::retained_generation(
                &mut tx,
                index as i64 + 1,
                &binding,
                json!({}),
            )
            .await?;
            if !distinct_generations {
                shared_generation = Some(generation);
            }
            generation
        };
        // Deployment admits the rule's dispatch digest. The managed-edit
        // boundary also retains its exact source body, which D requires for a
        // foreign delivery. Model an unchanged managed readmission, then capture
        // this delivery while that policy is still current. Later rules may
        // readmit another policy; their retained proofs must remain independent.
        let dispatch = ManagedComponentReadmission::rule(
            &mut tx,
            rule.id,
            ctx.user.as_ref().ok_or("editor missing")?.id,
        )
        .await?
        .ok_or("foreign rule was not managed during readmission")?;
        assert_eq!(dispatch.policy.owner.pack_id, foreign.id);
        let invocation = dispatch.policy.invocations.iter().find(|entry| {
            matches!(&entry.binding, attune_common::runtime_authority::InvocationBinding::Rule { rule_ref, .. } if rule_ref == &rule.r#ref)
        }).ok_or("readmitted foreign rule invocation missing")?;
        assert_eq!(invocation.source_id, Some(rule.id));
        assert!(invocation.dispatch_template_digest.is_some());
        SensorSchemaRepository::capture_rule_delivery(
            &mut tx,
            generation,
            &binding,
            rule,
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
        let proof =
            attune_common::repositories::sensor_rule_origin::SensorRuleOriginRepository::find(
                &mut *tx,
                "sensor_rule_delivery_config",
                delivery,
                rule.id,
            )
            .await?
            .ok_or("foreign delivery producer did not retain structural proof")?;
        assert_eq!(proof.dispatch_policy_revision, Some(dispatch.id));
        assert_eq!(
            proof.dispatch_source_release,
            Some(dispatch.policy.release_id)
        );
        assert_eq!(
            proof.dispatch_metadata_digest,
            invocation.dispatch_template_digest
        );
        assert_eq!(proof.managing_pack_id, foreign.id);
        proof.validate_binding(generation, &binding)?;
        tx.commit().await?;
        let prepared = prepare_secret_values(
            vec![SecretValueInput {
                json_path: "/private".into(),
                value: json!("foreign origin"),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&SecretProvenance {
                    origins: vec![local_origin()],
                    templates: vec![],
                })?),
            }],
            ctx.state
                .config
                .security
                .encryption_key
                .as_deref()
                .ok_or("encryption key missing")?,
        )?;
        ExecutionSecretValueRepository::upsert_many(
            &ctx.pool,
            "sensor_rule_delivery_config",
            delivery,
            &prepared,
        )
        .await?;
        result.push((delivery, rule.id));
    }
    Ok(result)
}

#[tokio::test]
async fn ordinary_foreign_sensor_origin_cold_queries_are_chunk_bounded_and_proofs_are_required(
) -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.foreign.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    let deliveries = foreign_sensor_deliveries(&ctx, 100, true).await?;
    let path = format!("/api/v1/artifacts/{log}");
    let mut samples = Vec::new();
    for count in [1, 64, 100] {
        secret_origins(
            &ctx,
            destination,
            deliveries[..count]
                .iter()
                .map(|(delivery, _)| entity_origin("sensor_rule_delivery_config", *delivery))
                .collect(),
        )
        .await?;
        // First request after replacing the graph. No endpoint warm-up and no
        // request-local ownership/proof cache can survive across these calls.
        let (sample, body) = measure(&ctx, &path).await?;
        assert_eq!(sample.status, 200, "{count}: {body}");
        assert_eq!(sample.queries.ciphertext_reads, 0);
        assert_eq!(sample.queries.source_payload_reads, 0);
        assert!(
            sample.queries.application_statements() <= if count <= 64 { 25 } else { 40 },
            "{count}: {}; metrics={}",
            sample.queries.application_statements(),
            serde_json::to_string(&sample.queries)?
        );
        samples.push((count, sample));
    }
    assert_eq!(
        samples[0].1.queries.source_scope_reads,
        samples[1].1.queries.source_scope_reads
    );
    assert_eq!(
        samples[0].1.queries.graph_record_reads,
        samples[1].1.queries.graph_record_reads
    );
    assert!(
        samples[2].1.queries.application_statements()
            <= samples[1].1.queries.application_statements() + 15
    );
    let report=samples.iter().map(|(count,sample)| json!({"origins":count,"application_queries":sample.queries.application_statements(),"driver_catalog_queries":sample.queries.driver_metadata_reads(),"measurement":sample})).collect::<Vec<_>>();
    save_report("ordinary-foreign-sensor-proof-cold-frontiers", &report)?;
    let parent = execution(&ctx, &action_ref).await?;
    secret_origins(
        &ctx,
        parent,
        deliveries
            .iter()
            .map(|(delivery, _)| entity_origin("sensor_rule_delivery_config", *delivery))
            .collect(),
    )
    .await?;
    secret_origins(&ctx, destination, vec![execution_origin(parent)]).await?;
    let (parent_sample, body) = measure(&ctx, &path).await?;
    assert_eq!(parent_sample.status, 200, "retained parent: {body}");
    assert!(parent_sample.queries.application_statements() <= 40);
    assert_eq!(parent_sample.queries.ciphertext_reads, 0);
    save_report(
        "ordinary-foreign-sensor-proof-retained-parent",
        &parent_sample,
    )?;
    let (delivery, rule) = deliveries[0];
    secret_origins(
        &ctx,
        destination,
        vec![entity_origin("sensor_rule_delivery_config", delivery)],
    )
    .await?;
    let current=<attune_common::repositories::rule::RuleRepository as attune_common::repositories::FindById>::find_by_id(&ctx.pool,rule).await?.ok_or("rule missing")?;
    let permission_ref = format!("{}.ordinary_reader", current.pack_ref);
    replace_grants(&ctx,&permission_ref,json!([{"resource":"rules","actions":["read"],"constraints":{"managing_pack_id":current.pack}}])).await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND,
        "a valid D proof does not grant decrypt"
    );
    replace_grants(&ctx,&permission_ref,json!([{"resource":"rules","actions":["read","decrypt"],"constraints":{"managing_pack_id":current.pack}}])).await?;
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    let identity = ctx.user.as_ref().ok_or("identity missing")?.id;
    let pool = &ctx.pool;
    let update_attributes = |attributes| async move {
        <attune_common::repositories::identity::IdentityRepository as attune_common::repositories::Update>::update(
            pool,identity,attune_common::repositories::identity::UpdateIdentityInput {attributes:Some(attributes),..Default::default()}).await
    };
    update_attributes(json!({"team":"ops"})).await?;
    replace_grants(&ctx,&permission_ref,json!([{"resource":"rules","actions":["read","decrypt"],"constraints":{"managing_pack_id":current.pack,"attributes":{"team":"ops"}}}])).await?;
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    update_attributes(json!({"team":"blocked"})).await?;
    // Deliberately do not invalidate the service's identity-attribute cache.
    // Historical source IAM must read this request's actual caller metadata.
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    replace_grants(&ctx,&permission_ref,json!([{"resource":"rules","actions":["read","decrypt"],"constraints":{"managing_pack_id":current.pack}}])).await?;
    let pin: (i64, i64, i64) = sqlx::query_as("DELETE FROM managed_sensor_generation g USING sensor_generation_schema_binding b,sensor_rule_config_delivery d WHERE d.id=$1 AND b.id=d.generation_binding AND g.workload=b.workload AND g.generation=b.generation RETURNING g.workload,g.generation,g.revision")
        .bind(delivery).fetch_one(&ctx.pool).await?;
    assert_ne!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::OK,
        "foreign proof cannot replace a missing original metadata pin"
    );
    sqlx::query(
        "INSERT INTO managed_sensor_generation(workload,generation,revision) VALUES ($1,$2,$3)",
    )
    .bind(pin.0)
    .bind(pin.1)
    .bind(pin.2)
    .execute(&ctx.pool)
    .await?;
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    // Corruption fixture replaces one immutable record; do not disable the
    // production immutability guard merely to exercise decoder rejection.
    sqlx::query("WITH removed AS (DELETE FROM sensor_rule_structural_origin WHERE source_entity='sensor_rule_delivery_config' AND source_id=$2 AND rule_id=$3 RETURNING source_entity,source_id,rule_id,generation_binding,dispatch_policy_revision,dispatch_source_release,proof) INSERT INTO sensor_rule_structural_origin(source_entity,source_id,rule_id,generation_binding,dispatch_policy_revision,dispatch_source_release,proof,proof_digest) SELECT source_entity,source_id,rule_id,generation_binding,dispatch_policy_revision,dispatch_source_release,proof,$1 FROM removed")
        .bind("0".repeat(64)).bind(delivery).bind(rule).execute(&ctx.pool).await?;
    assert_ne!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::OK,
        "tampered D proof must not disclose"
    );
    sqlx::query("DELETE FROM sensor_rule_structural_origin WHERE source_entity='sensor_rule_delivery_config' AND source_id=$1 AND rule_id=$2")
        .bind(delivery).bind(rule).execute(&ctx.pool).await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND,
        "missing foreign proof must not disclose"
    );
    let (other_delivery, other_rule) = deliveries[1];
    let original=<attune_common::repositories::rule::RuleRepository as attune_common::repositories::FindById>::find_by_id(&ctx.pool,other_rule).await?.ok_or("original rule missing")?;
    sqlx::query("UPDATE rule SET retired_at=NOW(),ref=ref||'_retired' WHERE id=$1")
        .bind(other_rule)
        .execute(&ctx.pool)
        .await?;
    let replacement = attune_common::repositories::rule::RuleRepository::create(
        &ctx.pool,
        attune_common::repositories::rule::CreateRuleInput {
            r#ref: original.r#ref,
            pack: original.pack,
            pack_ref: original.pack_ref,
            label: original.label,
            description: None,
            action: original.action.ok_or("rule action missing")?,
            action_ref: original.action_ref,
            trigger: original.trigger.ok_or("rule trigger missing")?,
            trigger_ref: original.trigger_ref,
            conditions: original.conditions,
            action_params: original.action_params,
            trigger_params: original.trigger_params,
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await?;
    assert_ne!(replacement.id, other_rule);
    secret_origins(
        &ctx,
        destination,
        vec![entity_origin("sensor_rule_delivery_config", other_delivery)],
    )
    .await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND,
        "a same-ref replacement cannot repair the original rule proof"
    );
    ctx.cleanup().await?;
    Ok(())
}
