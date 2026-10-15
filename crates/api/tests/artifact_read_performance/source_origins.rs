//! Distinct non-execution origins use the real disclosure route and repositories.
use super::*;
#[path = "../artifact_workload/profile.rs"]
mod workload;
use attune_common::{
    config::NativeMaintenanceConfig,
    models::InquiryStatus,
    repositories::{
        component_configuration::{
            ComponentConfigurationRepository, ENTITY_COMPONENT_CONFIGURATION,
        },
        event::{CreateEventInput, EventRepository},
        event_schema::EventSchemaRepository,
        inquiry::{CreateInquiryInput, InquiryRepository},
        native_maintenance::{
            partitions::{PartitionRepairOutcome, PartitionRepository},
            ManagedTable,
        },
        pack::PackRepository,
        trigger::{CreateTriggerInput, TriggerRepository},
        FindByRef,
    },
    secret_values::{ENTITY_EVENT_PAYLOAD, ENTITY_INQUIRY_OPTIONS, ENTITY_INQUIRY_RESPONSE},
};

fn entity_origin(kind: &str, id: i64) -> SecretOrigin {
    SecretOrigin::Entity {
        entity_type: kind.into(),
        entity_id: id,
        path: "/private".into(),
    }
}

fn fixture_maintenance_config() -> NativeMaintenanceConfig {
    NativeMaintenanceConfig {
        operation_timeout_milliseconds: 30_000,
        lock_timeout_milliseconds: 1_000,
        ..Default::default()
    }
}

#[tokio::test]
async fn distinct_event_and_inquiry_chains_grow_by_frontier_not_source_count() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    let trigger = trigger(&ctx).await?;
    grant(
        &ctx,
        "fixture.source_chains",
        json!([
            {"resource":"events","actions":["read","decrypt"]},
            {"resource":"inquiries","actions":["read","decrypt"]}
        ]),
    )
    .await?;
    let path = format!("/api/v1/artifacts/{log}");
    let mut report = Vec::new();
    for kind in [
        ENTITY_EVENT_PAYLOAD,
        ENTITY_INQUIRY_OPTIONS,
        ENTITY_INQUIRY_RESPONSE,
    ] {
        let mut roots = Vec::new();
        let mut samples = Vec::new();
        for width in [1, 16] {
            while roots.len() < width {
                let mut origin = local_origin();
                for _ in 0..4 {
                    let id = if kind == ENTITY_EVENT_PAYLOAD {
                        // Events and their encrypted provenance stay immutable.
                        // Insert each link once, rather than changing a leaf.
                        event_with_origins(&ctx, &trigger, None, vec![origin]).await?
                    } else {
                        let id = inquiry(&ctx, destination).await?;
                        source_secret_with_origins(&ctx, kind, id, vec![origin]).await?;
                        id
                    };
                    origin = entity_origin(kind, id);
                }
                roots.push(origin);
            }
            secret_origins(&ctx, destination, roots.clone()).await?;
            let _ = ctx.get(&path, ctx.token()).await?.bytes().await?;
            let (sample, body) = measure(&ctx, &path).await?;
            assert_eq!(sample.status, 200, "{kind}: {body}");
            assert_eq!(sample.queries.ciphertext_reads, 0);
            assert_eq!(sample.queries.source_payload_reads, 0);
            samples.push(sample);
        }
        assert_eq!(
            samples[0].queries.source_scope_reads, samples[1].queries.source_scope_reads,
            "{kind}"
        );
        assert_eq!(
            samples[0].queries.graph_record_reads, samples[1].queries.graph_record_reads,
            "{kind}"
        );
        report.push(json!({"entity_kind":kind,"depth":4,"widths":[1,16],"samples":samples}));
    }
    save_report("event-inquiry-chain-frontiers", &report)?;
    ctx.cleanup().await?;
    Ok(())
}

async fn source_secret(ctx: &TestContext, kind: &str, id: i64) -> Result<()> {
    source_secret_with_origins(ctx, kind, id, vec![local_origin()]).await
}

async fn source_secret_with_origins(
    ctx: &TestContext,
    kind: &str,
    id: i64,
    origins: Vec<SecretOrigin>,
) -> Result<()> {
    let prepared = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/private".into(),
            value: json!("owned source fixture"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&SecretProvenance {
                origins,
                templates: vec![],
            })?),
        }],
        ctx.state
            .config
            .security
            .encryption_key
            .as_deref()
            .ok_or("fixture key missing")?,
    )?;
    ExecutionSecretValueRepository::upsert_many(&ctx.pool, kind, id, &prepared).await?;
    Ok(())
}

async fn trigger(ctx: &TestContext) -> Result<attune_common::models::Trigger> {
    Ok(TriggerRepository::create(
        &ctx.pool,
        CreateTriggerInput {
            r#ref: "artifact_sources.event".into(),
            pack: None,
            pack_ref: None,
            label: "Source fixture".into(),
            description: None,
            enabled: true,
            param_schema: None,
            out_schema: None,
            event_config_schema: None,
            sensor: None,
            sensor_ref: None,
            is_adhoc: true,
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: vec![],
        },
    )
    .await?)
}

async fn event(
    ctx: &TestContext,
    trigger: &attune_common::models::Trigger,
    at: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<i64> {
    event_with_origins(ctx, trigger, at, vec![local_origin()]).await
}

async fn event_with_origins(
    ctx: &TestContext,
    trigger: &attune_common::models::Trigger,
    at: Option<chrono::DateTime<chrono::Utc>>,
    origins: Vec<SecretOrigin>,
) -> Result<i64> {
    let mut tx = ctx.pool.begin().await?;
    let (_, binding) =
        EventSchemaRepository::materialize_current(&mut tx, trigger.id, false).await?;
    let id = if let Some(at) = at {
        // Only fixture seeding sets historical timestamps. Production reads go
        // through repositories, and no database outside this clone is touched.
        sqlx::query_scalar(
            "INSERT INTO event (trigger, trigger_ref, created) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(trigger.id)
        .bind(&trigger.r#ref)
        .bind(at)
        .fetch_one(&mut *tx)
        .await?
    } else {
        EventRepository::create(
            &mut *tx,
            CreateEventInput {
                trigger: Some(trigger.id),
                trigger_ref: trigger.r#ref.clone(),
                config: None,
                payload: None,
                trace_tag: None,
                source: None,
                source_ref: None,
                rule: None,
                rule_ref: None,
            },
        )
        .await?
        .id
    };
    EventSchemaRepository::attach(&mut tx, id, &binding).await?;
    tx.commit().await?;
    source_secret_with_origins(ctx, ENTITY_EVENT_PAYLOAD, id, origins).await?;
    Ok(id)
}

async fn inquiry(ctx: &TestContext, creator: i64) -> Result<i64> {
    Ok(InquiryRepository::create(
        &ctx.pool,
        CreateInquiryInput {
            created_by_execution: creator,
            prompt: "Owned source?".into(),
            response_schema: Some(json!({"approved":{"type":"boolean","required":true}})),
            response_options: vec![attune_common::models::inquiry::InquiryResponseOption {
                r#ref: "approve".into(),
                label: "Approve".into(),
                style: attune_common::models::inquiry::InquiryResponseOptionStyle::Default,
                response: json!({"approved":true}),
            }],
            assigned_to: None,
            status: InquiryStatus::Pending,
            response: None,
            timeout_at: None,
        },
    )
    .await?
    .id)
}

pub(super) fn save_report(name: &str, report: &impl Serialize) -> Result<()> {
    let report = serde_json::to_string_pretty(report)?;
    println!("{name}: {report}");
    if let Some(output) = std::env::var_os("ATTUNE_ARTIFACT_BENCHMARK_OUTPUT") {
        use std::io::Write;
        writeln!(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(std::path::PathBuf::from(output).join(format!("{name}.json")))?,
            "{report}"
        )?;
    }
    Ok(())
}

#[tokio::test]
async fn event_inquiry_and_component_sources_have_constant_within_batch_query_counts() -> Result<()>
{
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    grant(
        &ctx,
        "fixture.sources",
        json!([
            {"resource":"events","actions":["read","decrypt"]},
            {"resource":"inquiries","actions":["read","decrypt"]},
            {"resource":"packs","actions":["read","decrypt"]}
        ]),
    )
    .await?;
    let trigger = trigger(&ctx).await?;
    let pack = PackRepository::find_by_ref(&ctx.pool, "artifact_perf")
        .await?
        .ok_or("pack missing")?;
    let mut tx = ctx.pool.begin().await?;
    ComponentConfigurationRepository::lock_selection(&mut tx).await?;
    let revision = ComponentConfigurationRepository::bind_pack(&mut tx, &pack, None, None).await?;
    tx.commit().await?;
    let path = format!("/api/v1/artifacts/{log}");
    let mut report = Vec::new();
    for kind in [
        ENTITY_EVENT_PAYLOAD,
        ENTITY_INQUIRY_OPTIONS,
        ENTITY_INQUIRY_RESPONSE,
        ENTITY_COMPONENT_CONFIGURATION,
    ] {
        let mut origins = Vec::new();
        let mut samples = Vec::new();
        for count in [1, 64, 65] {
            while origins.len() < count {
                let id = match kind {
                    ENTITY_EVENT_PAYLOAD => event(&ctx, &trigger, None).await?,
                    ENTITY_INQUIRY_OPTIONS | ENTITY_INQUIRY_RESPONSE => inquiry(&ctx, destination).await?,
                    _ => sqlx::query_scalar("INSERT INTO component_configuration_revision (owner_kind, owner_id, owner_ref, pack_ref, contract, binding_digest, config) VALUES ('pack',$1,$2,$2,$3,$4,'{}') RETURNING id")
                        .bind(pack.id).bind(&pack.r#ref).bind(&revision.contract).bind(&revision.binding_digest).fetch_one(&ctx.pool).await?,
                };
                if kind != ENTITY_EVENT_PAYLOAD {
                    source_secret(&ctx, kind, id).await?;
                }
                origins.push(entity_origin(kind, id));
            }
            secret_origins(&ctx, destination, origins.clone()).await?;
            let _ = ctx.get(&path, ctx.token()).await?.bytes().await?;
            let (sample, body) = measure(&ctx, &path).await?;
            assert_eq!(sample.status, 200, "{kind}: {body}");
            assert_eq!(sample.queries.ciphertext_reads, 0);
            assert_eq!(sample.queries.source_payload_reads, 0);
            samples.push((count, sample));
        }
        // Predeclared: 1 and 64 fit one frontier batch. The 65th origin
        // may add one set of source/binding/provenance reads, not 64 lookups.
        assert_eq!(
            samples[0].1.queries.source_scope_reads, samples[1].1.queries.source_scope_reads,
            "{kind}"
        );
        assert_eq!(
            samples[0].1.queries.graph_record_reads, samples[1].1.queries.graph_record_reads,
            "{kind}"
        );
        assert!(samples[1].1.queries.statements <= 25, "{kind}");
        assert!(samples[2].1.queries.statements <= 40, "{kind}");
        let expired = match origins.last().ok_or("source origin missing")? {
            SecretOrigin::Entity { entity_id, .. } => *entity_id,
            _ => return Err("expected fixture entity origin".into()),
        };
        match kind {
            ENTITY_EVENT_PAYLOAD => {
                sqlx::query("DELETE FROM event WHERE id = $1")
                    .bind(expired)
                    .execute(&ctx.pool)
                    .await?;
            }
            ENTITY_INQUIRY_OPTIONS | ENTITY_INQUIRY_RESPONSE => {
                sqlx::query("DELETE FROM inquiry WHERE id = $1")
                    .bind(expired)
                    .execute(&ctx.pool)
                    .await?;
            }
            _ => {
                sqlx::query("DELETE FROM component_configuration_revision WHERE id = $1")
                    .bind(expired)
                    .execute(&ctx.pool)
                    .await?;
            }
        }
        assert_eq!(
            ctx.get(&path, ctx.token()).await?.status(),
            StatusCode::NOT_FOUND,
            "{kind}: expired source records must not survive the request"
        );
        report.push((kind, samples));
    }
    save_report("non-execution-source-frontiers", &report)?;
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn event_sources_survive_default_repair_but_deny_ambiguous_or_expired_ids() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    grant(
        &ctx,
        "fixture.event_sources",
        json!([{"resource":"events","actions":["read","decrypt"]}]),
    )
    .await?;
    let trigger = trigger(&ctx).await?;
    let day =
        chrono::DateTime::parse_from_rfc3339("2020-01-02T00:00:00Z")?.with_timezone(&chrono::Utc);
    let id = event(&ctx, &trigger, Some(day + chrono::Duration::hours(3))).await?;
    secret_origins(
        &ctx,
        destination,
        vec![entity_origin(ENTITY_EVENT_PAYLOAD, id)],
    )
    .await?;
    let path = format!("/api/v1/artifacts/{log}");
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    let config = fixture_maintenance_config();
    assert_eq!(
        PartitionRepository::ensure_day(&ctx.pool, ManagedTable::Event, day, &config).await?,
        PartitionRepairOutcome::Applied { rows_moved: 1 }
    );
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    sqlx::query("INSERT INTO event (id, trigger, trigger_ref, created) VALUES ($1,$2,$3,$4)")
        .bind(id)
        .bind(trigger.id)
        .bind(&trigger.r#ref)
        .bind(day + chrono::Duration::days(1))
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        EventRepository::find_disclosure_rows(&ctx.pool, &[id])
            .await?
            .len(),
        2
    );
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND,
        "ID-only provenance cannot select an arbitrary physical event"
    );
    sqlx::query("DELETE FROM event WHERE id = $1 AND created = $2")
        .bind(id)
        .bind(day + chrono::Duration::days(1))
        .execute(&ctx.pool)
        .await?;
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    let expiry = PartitionRepository::expire_before(
        &ctx.pool,
        ManagedTable::Event,
        day + chrono::Duration::days(1),
        &config,
        false,
        || false,
    )
    .await?;
    assert_eq!(expiry.partitions_dropped, 1);
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    ctx.cleanup().await?;
    Ok(())
}

async fn sensor_binding(
    ctx: &TestContext,
) -> Result<attune_common::sensor_schema::SensorGenerationBinding> {
    use attune_common::{
        models::{PackReleasePin, SensorExecutableSnapshot},
        repositories::{
            runtime::{CreateRuntimeInput, RuntimeRepository},
            trigger::{CreateSensorInput, SensorRepository},
        },
        value_schema::{SchemaContext, SourceIdentity},
    };
    let pack = PackRepository::find_by_ref(&ctx.pool, "artifact_perf")
        .await?
        .ok_or("pack missing")?;
    let runtime = RuntimeRepository::create(
        &ctx.pool,
        CreateRuntimeInput {
            r#ref: "artifact_perf.native".into(),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            name: "native".into(),
            description: None,
            aliases: vec![],
            distributions: json!({}),
            installation: None,
            execution_config: json!({}),
            auto_detected: false,
            detection_config: json!({}),
        },
    )
    .await?;
    let sensor = SensorRepository::create(
        &ctx.pool,
        CreateSensorInput {
            r#ref: "artifact_perf.sensor".into(),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: "Source sensor".into(),
            description: None,
            entrypoint: "sensor".into(),
            runtime: runtime.id,
            runtime_ref: runtime.r#ref.clone(),
            runtime_version_constraint: None,
            enabled: true,
            param_schema: None,
            config: None,
            worker_selector: json!({}),
            worker_tolerations: json!([]),
            worker_affinity: json!({}),
            log_retention_policy: None,
            log_retention_limit: None,
            artifact_retention_policy: None,
            artifact_retention_limit: None,
        },
    )
    .await?;
    let release =
        attune_common::repositories::pack_release::PackReleaseRepository::find_active_by_pack_ref(
            &ctx.pool,
            &pack.r#ref,
        )
        .await?
        .ok_or("release missing")?;
    // Match the same immutable release identity as the production admission path.
    let config = attune_common::value_schema::compile(
        SourceIdentity::Released {
            component_ref: sensor.r#ref.clone(),
            release_id: release.id,
            release_digest: release.digest.clone(),
        },
        SchemaContext::FlatDocument,
        &json!({}),
        &Default::default(),
        Default::default(),
    )?;
    let binding = attune_common::sensor_schema::SensorGenerationBinding {
        executable: SensorExecutableSnapshot {
            release: PackReleasePin {
                id: release.id,
                digest: release.digest,
                content_path: release.content_path,
            },
            sensor,
            runtime,
            runtime_versions: vec![],
            triggers: vec![],
            cache_permission_set_refs: vec![],
            cache_grants: vec![],
            signing_permission_set_refs: vec![],
            signing_grants: vec![],
            input_permission_set_refs: vec![],
            input_grants: vec![],
        },
        config,
        triggers: Default::default(),
    };
    binding.validate()?;
    Ok(binding)
}

#[tokio::test]
async fn sensor_generation_rule_and_delivery_sources_are_batched_and_keep_recorded_identity(
) -> Result<()> {
    use attune_common::repositories::{
        action::ActionRepository,
        rule::{CreateRuleInput, RuleRepository},
        sensor_schema::{
            ENTITY_SENSOR_GENERATION_CONFIG, ENTITY_SENSOR_GENERATION_RULE_CONFIG,
            ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
        },
    };
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let action = ActionRepository::find_by_ref(&ctx.pool, &action_ref)
        .await?
        .ok_or("action missing")?;
    let trigger = trigger(&ctx).await?;
    let binding = sensor_binding(&ctx).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    grant(&ctx, "fixture.sensor_sources", json!([
        {"resource":"triggers","actions":["update","decrypt"],"constraints":{"refs":[binding.executable.sensor.r#ref]}},
        {"resource":"rules","actions":["read","decrypt"],"constraints":{"pack_refs":[action.pack_ref]}}
    ])).await?;
    let mut report = Vec::new();
    let path = format!("/api/v1/artifacts/{log}");
    let mut sequence = 0i64;
    for kind in [
        ENTITY_SENSOR_GENERATION_CONFIG,
        ENTITY_SENSOR_GENERATION_RULE_CONFIG,
        ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
    ] {
        let mut origins = Vec::new();
        let mut samples = Vec::new();
        let mut last_rule = 0;
        for count in [1, 64, 65] {
            while origins.len() < count {
                sequence += 1;
                let rule = RuleRepository::create(
                    &ctx.pool,
                    CreateRuleInput {
                        r#ref: format!("artifact_perf.rule{sequence}"),
                        pack: action.pack,
                        pack_ref: action.pack_ref.clone(),
                        label: "Origin rule".into(),
                        description: None,
                        action: action.id,
                        action_ref: action_ref.clone(),
                        trigger: trigger.id,
                        trigger_ref: trigger.r#ref.clone(),
                        conditions: json!({}),
                        action_params: json!({}),
                        trigger_params: json!({}),
                        trace_tag_template: None,
                        permission_set_refs: None,
                        enabled: true,
                        is_adhoc: true,
                        owner_identity: None,
                    },
                )
                .await?;
                last_rule = rule.id;
                let initial_rules = json!({rule.id.to_string(): {"ref":rule.r#ref, "pack_ref":rule.pack_ref, "config":{"unrelated":"x".repeat(64 * 1024)}}});
                let generation: i64 = sqlx::query_scalar("INSERT INTO sensor_generation_schema_binding (workload,generation,binding,binding_digest,config,initial_rules) VALUES (1,$1,$2,$3,'{}',$4) RETURNING id")
                    .bind(sequence).bind(serde_json::to_value(&binding)?).bind(binding.binding_digest()?).bind(initial_rules).fetch_one(&ctx.pool).await?;
                if origins.is_empty() {
                    let rows = attune_common::repositories::sensor_schema::SensorSchemaRepository::find_disclosure_rows(&ctx.pool, &[generation]).await?;
                    assert!(
                        rows[0].initial_rules[rule.id.to_string()]
                            .get("config")
                            .is_none(),
                        "scope records must exclude rule configuration values"
                    );
                }
                let (id, secret_path) = if kind == ENTITY_SENSOR_RULE_DELIVERY_CONFIG {
                    let id = sqlx::query_scalar("INSERT INTO sensor_rule_config_delivery (generation_binding,rule,rule_ref,pack_ref,trigger_ref,fingerprint,config) VALUES ($1,$2,$3,$4,$5,$6,'{}') RETURNING id")
                        .bind(generation).bind(rule.id).bind(&rule.r#ref).bind(&rule.pack_ref).bind(&trigger.r#ref).bind("a".repeat(64)).fetch_one(&ctx.pool).await?;
                    (id, "/private".to_string())
                } else if kind == ENTITY_SENSOR_GENERATION_RULE_CONFIG {
                    (generation, format!("/{}/private", rule.id))
                } else {
                    (generation, "/private".to_string())
                };
                let prepared = prepare_secret_values(
                    vec![SecretValueInput {
                        json_path: secret_path.clone(),
                        value: json!("owned sensor fixture"),
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
                        .ok_or("key missing")?,
                )?;
                ExecutionSecretValueRepository::upsert_many(&ctx.pool, kind, id, &prepared).await?;
                origins.push(SecretOrigin::Entity {
                    entity_type: kind.into(),
                    entity_id: id,
                    path: secret_path,
                });
            }
            secret_origins(&ctx, destination, origins.clone()).await?;
            let _ = ctx.get(&path, ctx.token()).await?.bytes().await?;
            let (sample, body) = measure(&ctx, &path).await?;
            assert_eq!(sample.status, 200, "{kind}: {body}");
            assert_eq!(sample.queries.ciphertext_reads, 0);
            assert_eq!(sample.queries.source_payload_reads, 0);
            samples.push((count, sample));
        }
        assert_eq!(
            samples[0].1.queries.source_scope_reads, samples[1].1.queries.source_scope_reads,
            "{kind}"
        );
        assert_eq!(
            samples[0].1.queries.graph_record_reads, samples[1].1.queries.graph_record_reads,
            "{kind}"
        );
        assert!(samples[1].1.queries.statements <= 25, "{kind}");
        assert!(samples[2].1.queries.statements <= 40, "{kind}");
        if kind != ENTITY_SENSOR_GENERATION_CONFIG {
            sqlx::query("UPDATE rule SET retired_at = NOW() WHERE id = $1")
                .bind(last_rule)
                .execute(&ctx.pool)
                .await?;
            assert_eq!(
                ctx.get(&path, ctx.token()).await?.status(),
                StatusCode::NOT_FOUND,
                "{kind}: original rule must survive"
            );
        }
        report.push((kind, samples));
    }
    save_report("sensor-source-frontiers", &report)?;
    ctx.cleanup().await?;
    Ok(())
}

async fn replace_grants(ctx: &TestContext, reference: &str, grants: Value) -> Result<()> {
    sqlx::query("UPDATE permission_set SET grants = $1 WHERE ref = $2")
        .bind(grants)
        .bind(reference)
        .execute(&ctx.pool)
        .await?;
    AuthorizationService::invalidate_identity_authz_cache(
        ctx.user.as_ref().ok_or("identity missing")?.id,
    )
    .await;
    AuthorizationService::invalidate_permission_set_caches().await;
    Ok(())
}

#[tokio::test]
async fn constrained_event_and_participant_inquiry_origins_keep_their_endpoint_policies(
) -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    let path = format!("/api/v1/artifacts/{log}");
    let trigger = trigger(&ctx).await?;
    sqlx::query("UPDATE trigger SET reference_visibility = 'private' WHERE id = $1")
        .bind(trigger.id)
        .execute(&ctx.pool)
        .await?;
    let event_id = event(&ctx, &trigger, None).await?;
    secret_origins(
        &ctx,
        destination,
        vec![entity_origin(ENTITY_EVENT_PAYLOAD, event_id)],
    )
    .await?;
    // Event authority needs its own resource grant plus original rule/trigger
    // visibility. Execution decrypt does not supply either permission.
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    grant(&ctx, "fixture.source_scope", json!([
        {"resource":"events","actions":["read","decrypt"],"constraints":{"refs":["scoped.resource"]}},
        {"resource":"triggers","actions":["read"],"constraints":{"refs":[trigger.r#ref]}}
    ])).await?;
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    replace_grants(&ctx, "fixture.source_scope", json!([
        {"resource":"events","actions":["read","decrypt"],"constraints":{"refs":["scoped.resource"]}}
    ])).await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );

    let id = inquiry(&ctx, i64::MAX).await?;
    source_secret(&ctx, ENTITY_INQUIRY_OPTIONS, id).await?;
    secret_origins(
        &ctx,
        destination,
        vec![entity_origin(ENTITY_INQUIRY_OPTIONS, id)],
    )
    .await?;
    let identity = ctx.user.as_ref().ok_or("identity missing")?.id;
    sqlx::query("UPDATE inquiry SET assigned_to = $1 WHERE id = $2")
        .bind(identity)
        .bind(id)
        .execute(&ctx.pool)
        .await?;
    replace_grants(&ctx, "fixture.source_scope", json!([
        {"resource":"inquiries","actions":["decrypt"],"constraints":{"refs":[format!("inquiry:{id}")]}}
    ])).await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::OK,
        "an assignee is a reader, but still needs explicit decrypt"
    );
    sqlx::query("UPDATE inquiry SET assigned_to = NULL WHERE id = $1")
        .bind(id)
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    replace_grants(&ctx, "fixture.source_scope", json!([
        {"resource":"inquiries","actions":["read","decrypt"],"constraints":{"refs":[format!("inquiry:{id}")]}}
    ])).await?;
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    sqlx::query("DELETE FROM inquiry_schema_binding WHERE entity_id = $1")
        .bind(id)
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );

    sqlx::query("INSERT INTO inquiry_schema_binding (entity_id,binding,binding_digest,request_fingerprint) VALUES ($1,'{}',$2,$2)")
        .bind(id).bind("0".repeat(64)).execute(&ctx.pool).await?;
    replace_grants(&ctx, "fixture.source_scope", json!([])).await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND,
        "denied inquiry scope must not disclose a corrupt binding"
    );
    replace_grants(
        &ctx,
        "fixture.source_scope",
        json!([{"resource":"inquiries","actions":["read","decrypt"]}]),
    )
    .await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::BAD_REQUEST
    );

    // Repository corruption is an error, not a successful empty list. There is
    // no trigger bypass here; this clone receives one deliberately bad binding.
    replace_grants(
        &ctx,
        "fixture.source_scope",
        json!([{"resource":"events","actions":["read","decrypt"]}]),
    )
    .await?;
    secret_origins(
        &ctx,
        destination,
        vec![entity_origin(ENTITY_EVENT_PAYLOAD, event_id)],
    )
    .await?;
    sqlx::query("DELETE FROM event_schema_binding WHERE event = $1")
        .bind(event_id)
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    sqlx::query(
        "INSERT INTO event_schema_binding (event,contract,binding_digest) VALUES ($1,'{}',$2)",
    )
    .bind(event_id)
    .bind("0".repeat(64))
    .execute(&ctx.pool)
    .await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::BAD_REQUEST
    );
    sqlx::query("ALTER TABLE event_schema_binding RENAME TO fixture_unavailable_event_binding")
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        ctx.get(
            &format!("/api/v1/executions/{destination}/artifacts"),
            ctx.token()
        )
        .await?
        .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn distinct_public_action_fallbacks_are_batched_but_never_grant_decrypt() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let pack = PackRepository::find_by_ref(&ctx.pool, "artifact_perf")
        .await?
        .ok_or("pack missing")?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    replace_grants(
        &ctx,
        "fixture.artifact_reader",
        json!([
            {"resource":"actions","actions":["read","execute"]},
            {"resource":"executions","actions":["read"],"constraints":{"refs":[action_ref]}},
            {"resource":"executions","actions":["decrypt"]}
        ]),
    )
    .await?;
    let path = format!("/api/v1/artifacts/{log}");
    let mut origins = Vec::new();
    let mut measurements = Vec::new();
    for count in [1, 64] {
        while origins.len() < count {
            let reference = format!("artifact_perf.public{}", origins.len());
            let action =
                helpers::create_test_action(&ctx.pool, pack.id, &pack.r#ref, &reference).await?;
            sqlx::query("UPDATE action SET reference_visibility = 'public' WHERE id = $1")
                .bind(action.id)
                .execute(&ctx.pool)
                .await?;
            let row = ExecutionRepository::create(
                &ctx.pool,
                CreateExecutionInput {
                    action_ref: reference,
                    ..Default::default()
                },
            )
            .await?;
            secret_origins(&ctx, row.id, vec![local_origin()]).await?;
            origins.push(execution_origin(row.id));
        }
        secret_origins(&ctx, destination, origins.clone()).await?;
        let _ = ctx.get(&path, ctx.token()).await?.bytes().await?;
        let (sample, body) = measure(&ctx, &path).await?;
        assert_eq!(sample.status, 200, "{body}");
        measurements.push((count, sample));
    }
    assert_eq!(
        measurements[0].1.queries.source_scope_reads,
        measurements[1].1.queries.source_scope_reads
    );
    assert!(measurements[1].1.queries.statements <= 25);
    save_report("public-action-fallback-frontier", &measurements)?;
    replace_grants(&ctx, "fixture.artifact_reader", json!([
        {"resource":"actions","actions":["read","execute"]},
        {"resource":"executions","actions":["read","decrypt"],"constraints":{"refs":[action_ref]}}
    ])).await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn inquiry_origins_recheck_fresh_identity_attributes_after_a_parent_snapshot_is_cached(
) -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let id = inquiry(&ctx, i64::MAX).await?;
    source_secret(&ctx, ENTITY_INQUIRY_OPTIONS, id).await?;
    secret_origins(
        &ctx,
        destination,
        vec![entity_origin(ENTITY_INQUIRY_OPTIONS, id)],
    )
    .await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    let identity = ctx.user.as_ref().ok_or("identity missing")?.id;
    sqlx::query("UPDATE identity SET attributes = $1 WHERE id = $2")
        .bind(json!({"team":"ops"}))
        .bind(identity)
        .execute(&ctx.pool)
        .await?;
    grant(&ctx, "fixture.attribute_inquiry", json!([
        {"resource":"inquiries","actions":["read","decrypt"],"constraints":{"attributes":{"team":"ops"}}}
    ])).await?;
    let path = format!("/api/v1/artifacts/{log}");
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    // Deliberately leave the access-token attribute cache warm. Inquiry access
    // has always fetched fresh attributes even when its parent consumer did not.
    sqlx::query("UPDATE identity SET attributes = '{}' WHERE id = $1")
        .bind(identity)
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        ctx.get(&format!("/api/v1/inquiries/{id}"), ctx.token())
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    let status = ctx.get(&path, ctx.token()).await?.status();
    sqlx::query("UPDATE identity SET attributes = $1 WHERE id = $2")
        .bind(json!({"team":"ops"}))
        .bind(identity)
        .execute(&ctx.pool)
        .await?;
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    ctx.cleanup().await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "inquiry origin authority must not inherit stale caller attributes"
    );
    Ok(())
}

#[tokio::test]
async fn execution_input_and_output_origins_share_one_frontier_provenance_read() -> Result<()> {
    use attune_common::secret_values::ENTITY_EXECUTION_RESULT;
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    let mut input = Vec::new();
    let mut output = Vec::new();
    for _ in 0..64 {
        let source = ExecutionRepository::create(
            &ctx.pool,
            CreateExecutionInput {
                action_ref: action_ref.clone(),
                ..Default::default()
            },
        )
        .await?;
        source_secret(&ctx, ENTITY_EXECUTION_CONFIG, source.id).await?;
        source_secret(&ctx, ENTITY_EXECUTION_RESULT, source.id).await?;
        input.push(entity_origin(ENTITY_EXECUTION_CONFIG, source.id));
        output.push(entity_origin(ENTITY_EXECUTION_RESULT, source.id));
    }
    let path = format!("/api/v1/artifacts/{log}");
    let mut samples = Vec::new();
    for origins in [input.clone(), input.into_iter().chain(output).collect()] {
        secret_origins(&ctx, destination, origins).await?;
        let _ = ctx.get(&path, ctx.token()).await?.bytes().await?;
        let (sample, body) = measure(&ctx, &path).await?;
        assert_eq!(sample.status, 200, "{body}");
        assert_eq!(sample.queries.ciphertext_reads, 0);
        samples.push(sample);
    }
    assert_eq!(
        samples[0].queries.graph_record_reads,
        samples[1].queries.graph_record_reads
    );
    save_report("combined-execution-origin-provenance", &samples)?;
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn distinct_workflow_ancestries_keep_source_queries_constant_within_a_batch() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    let mut origins = Vec::new();
    let mut samples = Vec::new();
    for count in [1, 64] {
        while origins.len() < count {
            let root = ExecutionRepository::create(
                &ctx.pool,
                CreateExecutionInput {
                    action_ref: action_ref.clone(),
                    ..Default::default()
                },
            )
            .await?;
            let child = ExecutionRepository::create(
                &ctx.pool,
                CreateExecutionInput {
                    action_ref: action_ref.clone(),
                    parent: Some(root.id),
                    ..Default::default()
                },
            )
            .await?;
            source_secret(&ctx, ENTITY_EXECUTION_CONFIG, child.id).await?;
            origins.push(execution_origin(child.id));
        }
        secret_origins(&ctx, destination, origins.clone()).await?;
        let path = format!("/api/v1/artifacts/{log}");
        let _ = ctx.get(&path, ctx.token()).await?.bytes().await?;
        let (sample, body) = measure(&ctx, &path).await?;
        assert_eq!(sample.status, 200, "{body}");
        assert_eq!(sample.queries.ciphertext_reads, 0);
        samples.push((count, sample));
    }
    assert_eq!(
        samples[0].1.queries.graph_record_reads,
        samples[1].1.queries.graph_record_reads
    );
    save_report("distinct-ancestry-frontiers", &samples)?;
    assert!(
        samples[1].1.queries.statements <= 20,
        "distinct ancestry statement ceiling exceeded: {}",
        serde_json::to_string(&samples[1])?
    );
    ctx.cleanup().await?;
    Ok(())
}
