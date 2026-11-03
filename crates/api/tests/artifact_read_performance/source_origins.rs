//! Distinct non-execution origins use the real disclosure route and repositories.
use super::*;
#[path = "source_origins/foreign_sensor.rs"]
mod foreign_sensor;
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
        event_origin::{EventOriginRepository, EVENT_SOURCE_PAYLOAD},
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
    secret_values::{ENTITY_INQUIRY_OPTIONS, ENTITY_INQUIRY_RESPONSE},
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
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    let path = format!("/api/v1/artifacts/{log}/versions/1");
    assert_safe_runtime_log_header(&ctx, log).await?;
    let mut report = Vec::new();
    for kind in [
        EVENT_SOURCE_PAYLOAD,
        ENTITY_INQUIRY_OPTIONS,
        ENTITY_INQUIRY_RESPONSE,
    ] {
        let mut roots = Vec::new();
        let mut samples = Vec::new();
        for width in [1, 16] {
            while roots.len() < width {
                let mut origin = local_origin();
                for _ in 0..4 {
                    let id = if kind == EVENT_SOURCE_PAYLOAD {
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
    let event = if let Some(at) = at {
        // Only fixture seeding sets historical timestamps. Production reads go
        // through repositories, and no database outside this clone is touched.
        sqlx::query_as::<_, attune_common::models::Event>(
            "INSERT INTO event (trigger, trigger_ref, created) VALUES ($1, $2, $3) RETURNING id,trigger,trigger_ref,config,payload,trace_tag,source,source_ref,rule,rule_ref,created",
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
    };
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
    let id = EventOriginRepository::capture(&mut tx, &event, &binding, &prepared, &[]).await?;
    tx.commit().await?;
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
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    let path = format!("/api/v1/artifacts/{log}/versions/1");
    assert_safe_runtime_log_header(&ctx, log).await?;
    let mut report = Vec::new();
    for kind in [
        EVENT_SOURCE_PAYLOAD,
        ENTITY_INQUIRY_OPTIONS,
        ENTITY_INQUIRY_RESPONSE,
        ENTITY_COMPONENT_CONFIGURATION,
    ] {
        let mut origins = Vec::new();
        let mut samples = Vec::new();
        for count in [1, 64, 65] {
            while origins.len() < count {
                let id = match kind {
                    EVENT_SOURCE_PAYLOAD => event(&ctx, &trigger, None).await?,
                    ENTITY_INQUIRY_OPTIONS | ENTITY_INQUIRY_RESPONSE => inquiry(&ctx, destination).await?,
                    _ => sqlx::query_scalar("INSERT INTO component_configuration_revision (owner_kind, owner_id, owner_ref, pack_ref, contract, binding_digest, config) VALUES ('pack',$1,$2,$2,$3,$4,'{}') RETURNING id")
                        .bind(pack.id).bind(&pack.r#ref).bind(&revision.contract).bind(&revision.binding_digest).fetch_one(&ctx.pool).await?,
                };
                if kind != EVENT_SOURCE_PAYLOAD {
                    source_secret(&ctx, kind, id).await?;
                }
                origins.push(entity_origin(kind, id));
            }
            secret_origins(&ctx, destination, origins.clone()).await?;
            let _ = ctx.get(&path, ctx.token()).await?.bytes().await?;
            if count == 64 {
                // Keep the within-batch budget independent of connection-local
                // type discovery, including the inquiry options regression.
                ctx.flush_audit().await?;
                let mut parked = Vec::new();
                for _ in 0..ctx.pool.size() {
                    parked.push(ctx.pool.acquire().await?);
                }
                for connection in parked {
                    connection.close().await?;
                }
            }
            let (sample, body) = measure(&ctx, &path).await?;
            if count == 64 {
                assert!(
                    sample.queries.driver_metadata_reads() > 0,
                    "{kind}: fresh-connection regression must exercise SQLx type discovery"
                );
            }
            assert_eq!(sample.status, 200, "{kind}: {body}");
            assert_eq!(sample.queries.ciphertext_reads, 0);
            assert_eq!(sample.queries.source_payload_reads, 0);
            samples.push((count, sample));
        }
        let expired = match origins.last().ok_or("source origin missing")? {
            SecretOrigin::Entity { entity_id, .. } => *entity_id,
            _ => return Err("expected fixture entity origin".into()),
        };
        match kind {
            EVENT_SOURCE_PAYLOAD => {
                let source = EventOriginRepository::evidence(&ctx.pool, expired).await?;
                sqlx::query("DELETE FROM event WHERE id = $1 AND created = $2")
                    .bind(source.event)
                    .bind(source.created)
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
    // First exercise EVERY kind, including its expired-source denial, and save
    // the complete numeric report. A budget failure must not silently prevent
    // inquiry options/response or component-configuration coverage.
    for (kind, samples) in &report {
        for (_, sample) in samples {
            assert_selected_content_query_safety(&sample.queries);
        }
        // The historical 25/40 ceilings covered the aggregate metadata reader,
        // not selected content's transaction/current IAM/version pin. Add only
        // the explicitly enumerated fixed guard work; keep all statements in the
        // report and preserve the 15-statement extra-frontier limit separately.
        assert_eq!(
            samples[0].1.queries.source_scope_reads, samples[1].1.queries.source_scope_reads,
            "{kind}"
        );
        assert_eq!(
            samples[0].1.queries.graph_record_reads, samples[1].1.queries.graph_record_reads,
            "{kind}"
        );
        assert_eq!(
            samples[0].1.queries.application_statements(),
            samples[1].1.queries.application_statements(),
            "{kind}: within-batch application queries grew"
        );
        assert!(
            samples[1].1.queries.application_statements() <= selected_content_query_ceiling(25),
            "{kind}: within-batch selected-content query budget exceeded: {}",
            serde_json::to_string(samples)?
        );
        assert!(
            samples[2].1.queries.application_statements() <= selected_content_query_ceiling(40),
            "{kind}: cross-batch selected-content query budget exceeded: {}",
            serde_json::to_string(samples)?
        );
        assert!(
            samples[2].1.queries.application_statements()
                <= samples[1].1.queries.application_statements() + 15,
            "{kind}: the 65th origin exceeded one additional frontier batch: {}",
            serde_json::to_string(samples)?
        );
    }
    Ok(())
}

#[tokio::test]
async fn exact_event_sources_survive_default_repair_and_partition_twins_but_not_expiry(
) -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
        vec![entity_origin(EVENT_SOURCE_PAYLOAD, id)],
    )
    .await?;
    let path = format!("/api/v1/artifacts/{log}/versions/1");
    assert_safe_runtime_log_header(&ctx, log).await?;
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    let config = fixture_maintenance_config();
    assert_eq!(
        PartitionRepository::ensure_day(&ctx.pool, ManagedTable::Event, day, &config).await?,
        PartitionRepairOutcome::Applied { rows_moved: 1 }
    );
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    let source = EventOriginRepository::evidence(&ctx.pool, id).await?;
    sqlx::query("INSERT INTO event (id, trigger, trigger_ref, created) VALUES ($1,$2,$3,$4)")
        .bind(source.event)
        .bind(trigger.id)
        .bind(&trigger.r#ref)
        .bind(day + chrono::Duration::days(1))
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        EventRepository::find_disclosure_rows(&ctx.pool, &[source.event])
            .await?
            .len(),
        2
    );
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::OK,
        "the immutable snapshot selects the original complete partition identity"
    );
    sqlx::query("DELETE FROM event WHERE id = $1 AND created = $2")
        .bind(source.event)
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
    use attune_common::repositories::{
        managed_sensor_metadata::ManagedSensorMetadataRepository, trigger::SensorRepository,
    };
    let sensor = SensorRepository::find_by_ref(&ctx.pool, "artifact_perf.sensor")
        .await?
        .ok_or("admitted sensor missing")?;
    let revision = ManagedSensorMetadataRepository::current_revision(&ctx.pool, sensor.id)
        .await?
        .ok_or("admitted sensor metadata missing")?;
    let mut connection = ctx.pool.acquire().await?;
    let metadata = ManagedSensorMetadataRepository::find(&mut connection, revision).await?;
    let binding = attune_common::sensor_schema::SensorGenerationBinding {
        metadata_revision: Some(revision),
        executable: metadata.executable,
        config: metadata.config_contract,
        triggers: metadata.trigger_contracts,
    };
    binding.validate()?;
    Ok(binding)
}

#[tokio::test]
async fn retained_foreign_sensor_rule_proof_validates_only_original_generation_and_contract(
) -> Result<()> {
    use attune_common::repositories::sensor_rule_origin::SensorRuleOriginProof;
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    let _ = action(&ctx).await?;
    let binding = sensor_binding(&ctx).await?;
    let trigger = binding
        .executable
        .triggers
        .first()
        .ok_or("admitted trigger missing")?;
    let pack = binding
        .executable
        .sensor
        .pack
        .ok_or("sensor pack missing")?;
    let proof = SensorRuleOriginProof {
        source_entity: "sensor_rule_delivery_config".into(),
        source_id: 100,
        rule_id: 200,
        generation_binding: 300,
        rule_ref: "foreign.rule".into(),
        managing_pack_id: pack + 1,
        pack_ref: "foreign".into(),
        trigger_id: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        trigger_managing_pack_id: trigger.pack,
        sensor_id: binding.executable.sensor.id,
        sensor_managing_pack_id: pack,
        dispatch_policy_revision: Some(400),
        dispatch_source_release: Some(500),
        dispatch_metadata_digest: Some("a".repeat(64)),
        parameter_contract_digest: None,
    };
    proof.validate_binding(300, &binding)?;
    assert!(proof.validate_binding(301, &binding).is_err());
    let mut changed = proof.clone();
    changed.trigger_id += 1;
    assert!(changed.validate_binding(300, &binding).is_err());
    changed = proof.clone();
    changed.sensor_id += 1;
    assert!(changed.validate_binding(300, &binding).is_err());
    changed = proof.clone();
    changed.parameter_contract_digest = Some("b".repeat(64));
    assert!(changed.validate_binding(300, &binding).is_err());
    changed = proof.clone();
    changed.dispatch_policy_revision = None;
    assert!(changed.validate_binding(300, &binding).is_err());
    let mut replacement = binding.clone();
    replacement.executable.triggers[0].id += 1;
    assert!(proof.validate_binding(300, &replacement).is_err());
    ctx.cleanup().await?;
    Ok(())
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
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    let action_ref = action(&ctx).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let action = ActionRepository::find_by_ref(&ctx.pool, &action_ref)
        .await?
        .ok_or("action missing")?;
    let binding = sensor_binding(&ctx).await?;
    let trigger = binding
        .executable
        .triggers
        .first()
        .ok_or("admitted trigger missing")?;
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
    let path = format!("/api/v1/artifacts/{log}/versions/1");
    assert_safe_runtime_log_header(&ctx, log).await?;
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
                        is_adhoc: false,
                        owner_identity: None,
                    },
                )
                .await?;
                last_rule = rule.id;
                let initial_rules = json!({rule.id.to_string(): {"ref":rule.r#ref, "pack_ref":rule.pack_ref, "config":{"unrelated":"x".repeat(64 * 1024)}}});
                let mut tx = ctx.pool.begin().await?;
                let generation = managed_fixture::retained_generation(
                    &mut tx,
                    sequence,
                    &binding,
                    initial_rules,
                )
                .await?;
                tx.commit().await?;
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
    // Do not let a generation-config performance failure skip rule-config or
    // delivery-config disclosure and original-rule-survival checks above.
    for (kind, samples) in &report {
        for (_, sample) in samples {
            assert_selected_content_query_safety(&sample.queries);
        }
        assert_eq!(
            samples[0].1.queries.source_scope_reads, samples[1].1.queries.source_scope_reads,
            "{kind}"
        );
        assert_eq!(
            samples[0].1.queries.graph_record_reads, samples[1].1.queries.graph_record_reads,
            "{kind}"
        );
        assert_eq!(
            samples[0].1.queries.application_statements(),
            samples[1].1.queries.application_statements(),
            "{kind}: within-batch application queries grew"
        );
        // Same selected-content fixed guard allowance as the non-sensor origin
        // fixture; source payload/ciphertext and batching gates are unchanged.
        assert!(
            samples[1].1.queries.application_statements() <= selected_content_query_ceiling(25),
            "{kind}: within-batch selected-content query budget exceeded: {}",
            serde_json::to_string(samples)?
        );
        assert!(
            samples[2].1.queries.application_statements() <= selected_content_query_ceiling(40),
            "{kind}: cross-batch selected-content query budget exceeded: {}",
            serde_json::to_string(samples)?
        );
        assert!(
            samples[2].1.queries.application_statements()
                <= samples[1].1.queries.application_statements() + 15,
            "{kind}: the 65th origin exceeded one additional frontier batch: {}",
            serde_json::to_string(samples)?
        );
    }
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
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    let path = format!("/api/v1/artifacts/{log}/versions/1");
    assert_safe_runtime_log_header(&ctx, log).await?;
    let trigger = trigger(&ctx).await?;
    sqlx::query("UPDATE trigger SET reference_visibility = 'private' WHERE id = $1")
        .bind(trigger.id)
        .execute(&ctx.pool)
        .await?;
    let event_id = event(&ctx, &trigger, None).await?;
    secret_origins(
        &ctx,
        destination,
        vec![entity_origin(EVENT_SOURCE_PAYLOAD, event_id)],
    )
    .await?;
    // Event authority needs its own resource grant plus original rule/trigger
    // visibility. Execution decrypt does not supply either permission.
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    grant(&ctx, "fixture.source_scope", json!([
        {"resource":"events","actions":["read","decrypt"],"constraints":{"refs":[trigger.r#ref]}},
        {"resource":"triggers","actions":["read"],"constraints":{"refs":[trigger.r#ref]}}
    ])).await?;
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    replace_grants(&ctx, "fixture.source_scope", json!([
        {"resource":"events","actions":["read","decrypt"],"constraints":{"refs":[trigger.r#ref]}}
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

    // Repository corruption is a content error, not a masked denial. Delete
    // and recreate only this fixture's immutable source with corrupt evidence.
    replace_grants(
        &ctx,
        "fixture.source_scope",
        json!([{"resource":"events","actions":["read","decrypt"]}]),
    )
    .await?;
    secret_origins(
        &ctx,
        destination,
        vec![entity_origin(EVENT_SOURCE_PAYLOAD, event_id)],
    )
    .await?;
    let retained: (i64, chrono::DateTime<chrono::Utc>, Value, String) = sqlx::query_as(
        "SELECT event,event_created,snapshot,snapshot_digest FROM event_origin_snapshot WHERE id = $1"
    ).bind(event_id).fetch_one(&ctx.pool).await?;
    sqlx::query("DELETE FROM event_origin_snapshot WHERE id = $1")
        .bind(event_id)
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    sqlx::query(
        "INSERT INTO event_origin_snapshot (id,event,event_created,snapshot,snapshot_digest,evidence,evidence_digest) VALUES ($1,$2,$3,$4,$5,'{}',$6)",
    )
    .bind(event_id)
    .bind(retained.0)
    .bind(retained.1)
    .bind(retained.2)
    .bind(retained.3)
    .bind("0".repeat(64))
    .execute(&ctx.pool)
    .await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::BAD_REQUEST
    );
    sqlx::query("ALTER TABLE event_origin_snapshot RENAME TO fixture_unavailable_event_binding")
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_safe_runtime_log_header(&ctx, log).await?;
    let metadata = ctx
        .get(
            &format!("/api/v1/executions/{destination}/artifacts"),
            ctx.token(),
        )
        .await?;
    assert_eq!(metadata.status(), StatusCode::OK);
    let metadata: Value = metadata.json().await?;
    assert_eq!(metadata["items"][0]["id"], log);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn distinct_public_action_fallbacks_are_batched_but_never_grant_decrypt() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    let path = format!("/api/v1/artifacts/{log}/versions/1");
    assert_safe_runtime_log_header(&ctx, log).await?;
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
    // PG18 measured 30 application statements at both 1 and 64 origins on the
    // selected-content route. Permit only the subsequent four structural-owner
    // survival reads, rather than its old metadata ceiling of 25. Run the
    // decrypt-denial assertion above before judging this performance budget.
    for (_, sample) in &measurements {
        assert_selected_content_query_safety(&sample.queries);
    }
    let queries = &measurements[1].1.queries;
    assert_eq!(
        measurements[0].1.queries.application_statements(),
        queries.application_statements()
    );
    assert!(queries.application_statements() <= 30 + SELECTED_CONTENT_OWNER_SURVIVAL_QUERIES,
        "public action fallback selected-content query budget exceeded: application={}, driver_metadata={}, total={}",
        queries.application_statements(), queries.driver_metadata_reads(), queries.statements);
    Ok(())
}

#[tokio::test]
async fn inquiry_origins_recheck_fresh_identity_attributes_after_a_parent_snapshot_is_cached(
) -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    let path = format!("/api/v1/artifacts/{log}/versions/1");
    assert_safe_runtime_log_header(&ctx, log).await?;
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
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    let path = format!("/api/v1/artifacts/{log}/versions/1");
    assert_safe_runtime_log_header(&ctx, log).await?;
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
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
        let path = format!("/api/v1/artifacts/{log}/versions/1");
        assert_safe_runtime_log_header(&ctx, log).await?;
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
    // This now opens protected selected content (30 application queries at both
    // 1/64 sources in PG18), not the historical metadata route (ceiling 20).
    // Fixed guard/owner work is enumerated at selected_content_query_ceiling;
    // neither driver discovery nor security work is hidden from the report.
    for (_, sample) in &samples {
        assert_selected_content_query_safety(&sample.queries);
    }
    assert_eq!(
        samples[0].1.queries.application_statements(),
        samples[1].1.queries.application_statements()
    );
    assert!(
        samples[1].1.queries.application_statements() <= selected_content_query_ceiling(20),
        "distinct ancestry statement ceiling exceeded: {}",
        serde_json::to_string(&samples[1])?
    );
    ctx.cleanup().await?;
    Ok(())
}
