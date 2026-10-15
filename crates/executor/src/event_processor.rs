//! Event Processor - Handles EventCreated messages and creates enforcements
//!
//! This component listens for EventCreated messages from the message queue,
//! finds matching rules for the event's trigger, evaluates conditions, and
//! creates enforcement records for rules that match.

use anyhow::Result;
use sqlx::{PgConnection, PgPool};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};

use attune_common::metadata_cache::MetadataCache;
use attune_common::{
    models::{EnforcementCondition, EnforcementStatus, Event, Rule},
    mq::{
        Consumer, EnforcementCreatedPayload, EventCreatedPayload, MessageEnvelope, MessageType,
        Publisher,
    },
    repositories::{
        action::ActionRepository,
        event::{CreateEnforcementInput, EnforcementRepository, EventRepository},
        execution_secret_value::ExecutionSecretValueRepository,
        pack::PackRepository,
        rule::RuleRepository,
        FindById, FindByRef, List,
    },
    secret_values::{
        prepare_secret_values, redacted_paths, restore_secret_values, RenderedJson,
        ENTITY_ENFORCEMENT_CONFIG, ENTITY_EVENT_PAYLOAD,
    },
    template_resolver::{resolve_templates_with_sensitivity, TemplateContext},
    workflow::expression::{eval_expression, is_truthy, EvalContext, EvalResult},
};
use std::time::Duration;

struct EventConditionContext {
    event: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::{
        config::Config,
        event_schema::EventSchemaBinding,
        repositories::{
            event::CreateEventInput, event_schema::EventSchemaRepository, pack::CreatePackInput,
            schema_type::SchemaTypeRepository, Create,
        },
        test_database::TestDatabase,
    };
    use serde_json::json;

    #[tokio::test]
    async fn rule_rendering_finishes_while_graph_reclamation_waits() -> Result<()> {
        let config = Config::load_from_file(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../config.test.yaml")
                .to_str()
                .unwrap(),
        )?;
        let database = TestDatabase::create(&config.database).await?;
        let pool = database.pool();
        let pack = PackRepository::create(
            pool,
            CreatePackInput {
                r#ref: "graph_renderer".into(),
                label: "Graph Renderer".into(),
                description: None,
                version: "1.0.0".into(),
                conf_schema: json!({"region":{"type":"string"}}),
                config: json!({"region":"fixture-region"}),
                meta: json!({}),
                tags: vec![],
                runtime_deps: vec![],
                dependencies: vec![],
                is_standard: false,
                installers: json!({}),
            },
        )
        .await?;
        let mut setup = pool.begin().await?;
        let event = EventRepository::create(
            &mut *setup,
            CreateEventInput {
                trigger: None,
                trigger_ref: "graph_renderer.input".into(),
                config: None,
                payload: Some(json!({"message":"fixture-message"})),
                trace_tag: None,
                source: None,
                source_ref: None,
                rule: None,
                rule_ref: None,
            },
        )
        .await?;
        EventSchemaRepository::attach(
            &mut setup,
            event.id,
            &EventSchemaBinding {
                trigger_id: None,
                trigger_ref: event.trigger_ref.clone(),
                payload: None,
                config: None,
                webhook: false,
            },
        )
        .await?;
        setup.commit().await?;
        let now = chrono::Utc::now();
        let rule = Rule {
            id: 1,
            r#ref: "graph_renderer.rule".into(),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Graph Renderer Rule".into(),
            description: None,
            action: None,
            action_ref: "graph_renderer.action".into(),
            trigger: None,
            trigger_ref: event.trigger_ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            sensor_worker_selector: json!({}),
            sensor_worker_tolerations: json!([]),
            sensor_worker_affinity: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            enabled_override: None,
            is_adhoc: true,
            owner_identity: Some(1),
            retired_at: None,
            created: now,
            updated: now,
        };
        let authority =
            attune_common::delegation::DelegationAuthority::new(1, Default::default(), vec![]);
        let mut admission = pool.begin().await?;
        SchemaTypeRepository::lock_selection(&mut admission).await?;
        let writer_pool = pool.clone();
        let (ready, pending) = tokio::sync::oneshot::channel();
        let writer = tokio::spawn(async move {
            let mut transaction = writer_pool.begin().await?;
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *transaction)
                .await?;
            let _ = ready.send(pid);
            SchemaTypeRepository::lock_reclamation(&mut transaction).await?;
            transaction.commit().await?;
            Ok::<_, anyhow::Error>(())
        });
        let pid = pending.await?;
        let queued = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE pid = $1 AND locktype = 'advisory' AND NOT granted)",
                ).bind(pid).fetch_one(pool).await?;
                if waiting {return Ok::<_, sqlx::Error>(());}
                tokio::task::yield_now().await;
            }
        }).await;
        let parameters =
            json!({"region":"{{ pack.config.region }}", "message":"{{ event.payload.message }}"});
        let rendered = if queued.as_ref().is_ok_and(|ready| ready.is_ok()) {
            Some(
                tokio::time::timeout(
                    Duration::from_secs(2),
                    EventProcessor::resolve_action_params(
                        &mut admission,
                        &None,
                        &rule,
                        &event,
                        event.payload.as_ref().unwrap(),
                        &authority,
                        &parameters,
                    ),
                )
                .await,
            )
        } else {
            None
        };
        admission.rollback().await?;
        tokio::time::timeout(Duration::from_secs(5), writer).await???;
        database.cleanup().await?;
        queued??;
        let rendered = rendered.unwrap().expect(
            "Rule rendering must not reacquire graph admission through a second connection",
        )?;
        assert_eq!(
            rendered.value,
            json!({"region":"fixture-region","message":"fixture-message"})
        );
        Ok(())
    }
}

impl EvalContext for EventConditionContext {
    fn resolve_variable(&self, name: &str) -> EvalResult<serde_json::Value> {
        match name {
            "event" => Ok(self.event.clone()),
            other => Err(
                attune_common::workflow::expression::EvalError::VariableNotFound(other.to_string()),
            ),
        }
    }

    fn call_workflow_function(
        &self,
        _name: &str,
        _args: &[serde_json::Value],
    ) -> EvalResult<Option<serde_json::Value>> {
        Ok(None)
    }
}

/// Event processor that handles event-to-rule matching
pub struct EventProcessor {
    pool: PgPool,
    publisher: Arc<Publisher>,
    consumer: Arc<Consumer>,
    encryption_key: Option<String>,
    rule_cache_by_id: Arc<MetadataCache<String, Rule>>,
    rule_cache_by_trigger_ref: Arc<MetadataCache<String, Vec<Rule>>>,
}

const RULE_METADATA_CACHE_TTL: Duration = Duration::from_secs(30);
const RULE_METADATA_CACHE_MAX_ENTRIES: usize = 2048;

impl EventProcessor {
    /// Create a new event processor
    pub fn new(
        pool: PgPool,
        publisher: Arc<Publisher>,
        consumer: Arc<Consumer>,
        encryption_key: Option<String>,
    ) -> Self {
        Self {
            pool,
            publisher,
            consumer,
            encryption_key,
            rule_cache_by_id: Arc::new(MetadataCache::new(
                RULE_METADATA_CACHE_TTL,
                RULE_METADATA_CACHE_MAX_ENTRIES,
            )),
            rule_cache_by_trigger_ref: Arc::new(MetadataCache::new(
                RULE_METADATA_CACHE_TTL,
                RULE_METADATA_CACHE_MAX_ENTRIES,
            )),
        }
    }

    /// Start processing EventCreated messages
    pub async fn start(&self) -> Result<()> {
        info!("Starting event processor");

        let pool = self.pool.clone();
        let publisher = self.publisher.clone();
        let encryption_key = self.encryption_key.clone();
        let rule_cache_by_id = self.rule_cache_by_id.clone();
        let rule_cache_by_trigger_ref = self.rule_cache_by_trigger_ref.clone();

        // Use the handler pattern to consume messages
        self.consumer
            .consume_with_handler(move |envelope: MessageEnvelope<EventCreatedPayload>| {
                let pool = pool.clone();
                let publisher = publisher.clone();
                let encryption_key = encryption_key.clone();
                let rule_cache_by_id = rule_cache_by_id.clone();
                let rule_cache_by_trigger_ref = rule_cache_by_trigger_ref.clone();

                async move {
                    if let Err(e) = Self::process_event_created(
                        &pool,
                        &publisher,
                        &encryption_key,
                        &rule_cache_by_id,
                        &rule_cache_by_trigger_ref,
                        &envelope,
                    )
                    .await
                    {
                        error!("Error processing event: {}", e);
                        // Return error to trigger nack with requeue
                        return Err(format!("Failed to process event: {}", e).into());
                    }
                    Ok(())
                }
            })
            .await?;

        Ok(())
    }

    /// Process an EventCreated message
    async fn process_event_created(
        pool: &PgPool,
        publisher: &Publisher,
        encryption_key: &Option<String>,
        rule_cache_by_id: &MetadataCache<String, Rule>,
        rule_cache_by_trigger_ref: &MetadataCache<String, Vec<Rule>>,
        envelope: &MessageEnvelope<EventCreatedPayload>,
    ) -> Result<()> {
        let payload = &envelope.payload;

        info!(
            "Processing EventCreated for event {} (trigger: {})",
            payload.event_id, payload.trigger_ref
        );

        // Fetch the event from database
        let event = EventRepository::find_by_id(pool, payload.event_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Event {} not found", payload.event_id))?;

        // Find matching rules for this trigger
        let matching_rules =
            Self::find_matching_rules(pool, rule_cache_by_id, rule_cache_by_trigger_ref, &event)
                .await?;

        if matching_rules.is_empty() {
            debug!(
                "No matching rules found for event {} (trigger: {})",
                event.id, event.trigger_ref
            );
            return Ok(());
        }

        info!(
            "Found {} matching rule(s) for event {}",
            matching_rules.len(),
            event.id
        );

        // Create enforcements for each matching rule
        for rule in matching_rules {
            if let Err(e) =
                Self::create_enforcement(pool, publisher, encryption_key, &rule, &event).await
            {
                error!(
                    "Failed to create enforcement for rule {} and event {}: {}",
                    rule.r#ref, event.id, e
                );
                // Continue with other rules even if one fails
            }
        }

        Ok(())
    }

    /// Find all enabled rules that match the event's trigger
    async fn find_matching_rules(
        pool: &PgPool,
        rule_cache_by_id: &MetadataCache<String, Rule>,
        rule_cache_by_trigger_ref: &MetadataCache<String, Vec<Rule>>,
        event: &Event,
    ) -> Result<Vec<Rule>> {
        let started = Instant::now();
        // Check if event is associated with a specific rule
        if let Some(rule_id) = event.rule {
            let id_key = rule_id.to_string();
            if let Some(rule) = rule_cache_by_id.get(&id_key).await {
                let result_count = if rule.enabled { 1u64 } else { 0u64 };
                debug!(
                    entity = "rule",
                    operation = "find_matching_rules",
                    caller = "executor.event_processor",
                    trigger_ref = event.trigger_ref,
                    event_id = event.id,
                    cache_hit = true,
                    cache_key = "id",
                    result_count,
                    latency_ms = started.elapsed().as_millis() as u64,
                    "metadata read"
                );
                return Ok(if rule.enabled && rule.retired_at.is_none() {
                    vec![rule]
                } else {
                    vec![]
                });
            }

            // Event is for a specific rule - only match that rule
            info!(
                "Event {} is associated with specific rule ID: {}",
                event.id, rule_id
            );
            match RuleRepository::find_by_id(pool, rule_id).await? {
                Some(rule) => {
                    rule_cache_by_id.insert(id_key, rule.clone()).await;
                    if rule.enabled && rule.retired_at.is_none() {
                        debug!(
                            entity = "rule",
                            operation = "find_matching_rules",
                            caller = "executor.event_processor",
                            trigger_ref = event.trigger_ref,
                            event_id = event.id,
                            cache_hit = false,
                            cache_key = "id",
                            result_count = 1u64,
                            latency_ms = started.elapsed().as_millis() as u64,
                            "metadata read"
                        );
                        Ok(vec![rule])
                    } else {
                        debug!("Rule {} is disabled, skipping", rule.r#ref);
                        Ok(vec![])
                    }
                }
                None => {
                    warn!(
                        "Event {} references non-existent rule {}",
                        event.id, rule_id
                    );
                    Ok(vec![])
                }
            }
        } else {
            let trigger_key = event.trigger_ref.clone();
            if let Some(rules) = rule_cache_by_trigger_ref.get(&trigger_key).await {
                let matching_rules: Vec<Rule> = rules
                    .into_iter()
                    .filter(|rule| rule.enabled && rule.retired_at.is_none())
                    .collect();
                debug!(
                    entity = "rule",
                    operation = "find_matching_rules",
                    caller = "executor.event_processor",
                    trigger_ref = event.trigger_ref,
                    event_id = event.id,
                    cache_hit = true,
                    cache_key = "trigger_ref",
                    result_count = matching_rules.len() as u64,
                    latency_ms = started.elapsed().as_millis() as u64,
                    "metadata read"
                );
                return Ok(matching_rules);
            }

            // No specific rule - match all rules for this trigger
            let matching_rules: Vec<Rule> = if let Some(trigger_id) = event.trigger {
                RuleRepository::find_by_trigger(pool, trigger_id).await?
            } else {
                RuleRepository::list(pool)
                    .await?
                    .into_iter()
                    .filter(|r| r.trigger_ref == event.trigger_ref)
                    .collect()
            };
            rule_cache_by_trigger_ref
                .insert(trigger_key, matching_rules.clone())
                .await;
            let matching_rules: Vec<Rule> = matching_rules
                .into_iter()
                .filter(|rule| rule.enabled && rule.retired_at.is_none())
                .collect();
            debug!(
                entity = "rule",
                operation = "find_matching_rules",
                caller = "executor.event_processor",
                trigger_ref = event.trigger_ref,
                event_id = event.id,
                cache_hit = false,
                cache_key = "trigger_ref",
                result_count = matching_rules.len() as u64,
                latency_ms = started.elapsed().as_millis() as u64,
                "metadata read"
            );

            Ok(matching_rules)
        }
    }

    /// Create an enforcement for a rule and event
    async fn create_enforcement(
        pool: &PgPool,
        publisher: &Publisher,
        encryption_key: &Option<String>,
        rule: &Rule,
        event: &Event,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(&mut tx)
            .await?;
        let rule = RuleRepository::find_by_id_for_share(&mut tx, rule.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Rule is no longer active"))?;
        let identity_id = rule.owner_identity.ok_or_else(|| {
            anyhow::anyhow!("Rule evaluation requires an explicit owner identity")
        })?;
        let authority =
            attune_common::delegation::DelegationAuthority::load_for_share(&mut tx, identity_id)
                .await?;
        let input_snapshot =
            attune_common::repositories::rule_authority::RuleAuthorityRepository::current(
                &mut tx, &rule,
            )
            .await?;
        if !rule.is_adhoc && input_snapshot.is_none() {
            return Err(anyhow::anyhow!(
                "Deployed rule input authority is unavailable; redeploy its pack"
            ));
        }
        let input_authority = match &input_snapshot {
            Some(snapshot) => snapshot.input_authority(&mut tx).await?,
            None => authority.clone(),
        };
        // Evaluate rule conditions
        let conditions_pass = Self::evaluate_conditions(&rule, event)?;

        if !conditions_pass {
            debug!(
                "Rule {} conditions did not match event {}",
                rule.r#ref, event.id
            );
            return Ok(());
        }

        info!(
            "Rule {} matched event {} - creating enforcement",
            rule.r#ref, event.id
        );

        // Prepare payload for enforcement
        let payload = event
            .payload
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));

        // Convert payload to dict if it's an object
        let payload_dict = payload
            .as_object()
            .cloned()
            .unwrap_or_else(serde_json::Map::new);

        let action = match rule.action {
            Some(action_id) => ActionRepository::find_by_id(&mut *tx, action_id).await?,
            None => ActionRepository::find_by_ref(&mut *tx, &rule.action_ref).await?,
        }
        .ok_or_else(|| anyhow::anyhow!("Action '{}' not found", rule.action_ref))?;
        if !action.enabled || action.retired_at.is_some() {
            debug!(
                "Rule {} references inactive action {}, skipping",
                rule.r#ref, action.r#ref
            );
            return Ok(());
        }
        // Select code and contracts before defaults, rendering, or secret
        // persistence. Execution handoff inherits this choice rather than
        // resolving external revisions a second time.
        let snapshot = attune_common::repositories::executable_snapshot::ExecutableSnapshotRepository::resolve_for_action(
            &mut *tx, action.id,
        ).await?;
        let action = &snapshot.executable.action;
        let binding = attune_common::repositories::schema_contract::SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
        let input_contract = attune_common::value_schema::CompiledContract::new(&binding.input)?;
        input_contract.validate_rule_secret_templates(&rule.action_params)?;
        let parameters = if rule.action_params.is_null() {
            serde_json::json!({})
        } else {
            rule.action_params.clone()
        };
        let parameters = input_contract.apply_defaults(&parameters)?;
        let rendered_params = Self::resolve_action_params(
            &mut tx,
            encryption_key,
            &rule,
            event,
            &payload,
            &input_authority,
            &parameters,
        )
        .await?;
        input_contract.validate(&rendered_params.value)?;
        let refs = rule
            .permission_set_refs
            .as_deref()
            .unwrap_or(&action.default_execution_permission_set_refs);
        authority.require_refs_for_share(&mut tx, refs).await?;
        attune_common::secret_values::validate_contract_secret_destination_paths(
            &rendered_params.value,
            &input_contract,
            &rendered_params.secret_paths,
        )?;
        let (redacted_params, secret_inputs) =
            attune_common::secret_values::merge_contract_secret_redactions(
                rendered_params.value,
                &rendered_params.secret_path_sources,
                &input_contract,
            )?;
        let redacted_params = redacted_params
            .as_object()
            .cloned()
            .unwrap_or_else(serde_json::Map::new);
        let prepared_secrets = if secret_inputs.is_empty() {
            Vec::new()
        } else {
            let encryption_key = encryption_key.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "Cannot store secret enforcement parameters without security.encryption_key"
                )
            })?;
            prepare_secret_values(secret_inputs, encryption_key)?
        };

        let create_input = CreateEnforcementInput {
            rule: Some(rule.id),
            rule_ref: rule.r#ref.clone(),
            trigger_ref: rule.trigger_ref.clone(),
            config: Some(serde_json::Value::Object(redacted_params)),
            event: Some(event.id),
            status: EnforcementStatus::Created,
            payload: serde_json::Value::Object(payload_dict),
            condition: EnforcementCondition::All,
            conditions: rule.conditions.clone(),
        };

        let enforcement_result = EnforcementRepository::create_or_get_by_rule_event_pinned(
            &mut tx,
            create_input,
            &snapshot,
        )
        .await?;
        let enforcement = enforcement_result.enforcement;
        if enforcement_result.created {
            if let Some(snapshot) = &input_snapshot {
                attune_common::repositories::rule_authority::RuleAuthorityRepository::attach_runtime(&mut tx, "enforcement", enforcement.id, snapshot).await?;
            }
            attune_common::repositories::schema_contract::SchemaContractRepository::attach(
                &mut tx,
                attune_common::repositories::schema_contract::BindingOwner::Enforcement,
                enforcement.id,
                &binding,
            )
            .await?;
        }
        if enforcement_result.created && !prepared_secrets.is_empty() {
            ExecutionSecretValueRepository::upsert_many_with_conn(
                &mut tx,
                ENTITY_ENFORCEMENT_CONFIG,
                enforcement.id,
                &prepared_secrets,
            )
            .await?;
        }
        tx.commit().await?;

        if enforcement_result.created {
            info!(
                "Enforcement {} created for rule {} (event: {})",
                enforcement.id, rule.r#ref, event.id
            );
        } else {
            info!(
                "Reusing enforcement {} for rule {} (event: {})",
                enforcement.id, rule.r#ref, event.id
            );
        }

        if enforcement_result.created || enforcement.status == EnforcementStatus::Created {
            let enforcement_payload = EnforcementCreatedPayload {
                enforcement_id: enforcement.id,
                rule_id: Some(rule.id),
                rule_ref: rule.r#ref.clone(),
                event_id: Some(event.id),
                trigger_ref: event.trigger_ref.clone(),
                payload: payload.clone(),
                release_id: enforcement.pack_release,
                release_digest: enforcement.pack_release_digest.clone(),
            };

            let envelope =
                MessageEnvelope::new(MessageType::EnforcementCreated, enforcement_payload)
                    .with_source("event-processor");

            publisher.publish_envelope(&envelope).await?;

            debug!(
                "Published EnforcementCreated message for enforcement {}",
                enforcement.id
            );
        }

        Ok(())
    }

    /// Evaluate rule conditions against event payload
    fn evaluate_conditions(rule: &Rule, event: &Event) -> Result<bool> {
        // If no payload, conditions cannot be evaluated (default to match)
        let payload = match &event.payload {
            Some(p) => p,
            None => {
                debug!("Event {} has no payload, matching by default", event.id);
                return Ok(true);
            }
        };

        // If rule has no conditions, it always matches
        if rule.conditions.is_null()
            || rule.conditions.as_object().is_some_and(|o| o.is_empty())
            || rule.conditions.as_array().is_some_and(|a| a.is_empty())
        {
            debug!("Rule {} has no conditions, matching by default", rule.r#ref);
            return Ok(true);
        }

        if let Some(criteria) = rule.conditions.get("expression").and_then(|v| v.as_str()) {
            return Ok(Self::evaluate_criteria_expression(criteria, event, payload));
        }

        // Parse conditions array
        let conditions = match rule.conditions.as_array() {
            Some(conds) => conds,
            None => {
                warn!("Rule {} conditions are not an array", rule.r#ref);
                return Ok(false);
            }
        };

        // Evaluate each condition (simplified - full evaluation logic would go here)
        let mut results = Vec::new();
        for condition in conditions {
            let result = Self::evaluate_single_condition(condition, payload)?;
            results.push(result);
        }

        // Apply logical operator (default to "all" = AND)
        let matches = results.iter().all(|&r| r);

        debug!(
            "Rule {} condition evaluation result: {} ({} condition(s))",
            rule.r#ref,
            matches,
            results.len()
        );

        Ok(matches)
    }

    fn evaluate_criteria_expression(
        criteria: &str,
        event: &Event,
        payload: &serde_json::Value,
    ) -> bool {
        let expression = criteria
            .trim()
            .strip_prefix("{{")
            .and_then(|s| s.strip_suffix("}}"))
            .map(str::trim)
            .unwrap_or_else(|| criteria.trim());
        let context = EventConditionContext {
            event: serde_json::json!({
                "id": event.id,
                "trigger": event.trigger_ref,
                "trigger_ref": event.trigger_ref,
                "payload": payload,
                "created": event.created.to_rfc3339(),
            }),
        };
        match eval_expression(expression, &context) {
            Ok(value) => is_truthy(&value),
            Err(error) => {
                warn!(
                    "Failed to evaluate rule criteria '{}' for event {}: {}",
                    criteria, event.id, error
                );
                false
            }
        }
    }

    /// Evaluate a single condition (simplified implementation)
    fn evaluate_single_condition(
        condition: &serde_json::Value,
        payload: &serde_json::Value,
    ) -> Result<bool> {
        // Expected condition format:
        // {
        //   "field": "payload.field_name",
        //   "operator": "equals",
        //   "value": "expected_value"
        // }

        let field = condition["field"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Condition missing 'field'"))?;

        let operator = condition["operator"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Condition missing 'operator'"))?;

        let expected_value = &condition["value"];

        // Extract field value from payload using dot notation
        let field_value = Self::extract_field_value(payload, field)?;

        // Apply operator
        let result = match operator {
            "equals" => field_value == expected_value,
            "not_equals" => field_value != expected_value,
            "contains" => {
                if let (Some(haystack), Some(needle)) =
                    (field_value.as_str(), expected_value.as_str())
                {
                    haystack.contains(needle)
                } else {
                    false
                }
            }
            _ => {
                warn!("Unknown operator '{}', defaulting to false", operator);
                false
            }
        };

        debug!(
            "Condition evaluation: field='{}', operator='{}', result={}",
            field, operator, result
        );

        Ok(result)
    }

    /// Extract field value from payload using dot notation
    fn extract_field_value<'a>(
        payload: &'a serde_json::Value,
        field: &str,
    ) -> Result<&'a serde_json::Value> {
        let mut current = payload;

        for part in field.split('.') {
            current = current
                .get(part)
                .ok_or_else(|| anyhow::anyhow!("Field '{}' not found in payload", field))?;
        }

        Ok(current)
    }

    /// Resolve action parameters by applying template variable substitution.
    ///
    /// Replaces `{{ event.payload.* }}`, `{{ event.id }}`, `{{ event.trigger }}`,
    /// `{{ event.created }}`, `{{ pack.config.* }}`, and `{{ system.* }}` references
    /// in the rule's `action_params` with values from the event and context.
    async fn resolve_action_params(
        connection: &mut PgConnection,
        encryption_key: &Option<String>,
        rule: &Rule,
        event: &Event,
        event_payload: &serde_json::Value,
        authority: &attune_common::delegation::DelegationAuthority,
        action_params: &serde_json::Value,
    ) -> Result<RenderedJson> {
        // If there are no action params, return empty
        if action_params.is_null() || action_params.as_object().is_none_or(|o| o.is_empty()) {
            return Ok(RenderedJson::plain(serde_json::json!({})));
        }

        // Load pack config from database for pack.config.* resolution
        let (pack_config, pack_secret_paths, pack_revision) = match PackRepository::find_by_id(
            &mut *connection, rule.pack,
        )
        .await
        {
            Ok(Some(pack)) => {
                attune_common::repositories::component_configuration::ComponentConfigurationRepository::pack_template_values(
                    connection, &pack, Some(authority), encryption_key.as_deref()).await?
            }
            Ok(None) => {
                warn!(
                    "Pack {} not found for rule {} — pack.config.* templates will resolve to null",
                    rule.pack, rule.r#ref
                );
                (serde_json::json!({}), Vec::new(), None)
            }
            Err(e) => {
                warn!("Failed to load pack {} for rule {}: {} — pack.config.* templates will resolve to null", rule.pack, rule.r#ref, e);
                (serde_json::json!({}), Vec::new(), None)
            }
        };

        let event_payload_secret_paths = redacted_paths(event_payload);
        let event_payload_for_templates = Self::restore_event_payload_for_templates(
            connection,
            encryption_key,
            event.id,
            event_payload.clone(),
        )
        .await?;

        // Build template context from the event
        let context = TemplateContext::new(
            event_payload_for_templates,
            pack_config,
            serde_json::json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "rule": {
                    "id": rule.id,
                    "ref": rule.r#ref,
                },
            }),
        )
        .with_event_id(event.id)
        .with_event_trigger(&event.trigger_ref)
        .with_event_created(&event.created.to_rfc3339())
        .with_event_payload_secret_paths(event.id, event_payload_secret_paths)
        .with_origin("rule", &rule.r#ref);
        let context = if let Some(revision) = pack_revision {
            context.with_bound_pack_config_secret_paths(revision, pack_secret_paths)
        } else {
            context
        };

        let expressions = attune_common::template_resolver::referenced_keystore_expressions(
            action_params,
            &context,
        )?;
        let context = if expressions.is_empty() {
            context
        } else {
            let refs = expressions.values().cloned().collect::<Vec<_>>();
            let keys = attune_common::key_access::resolve_explicit_keys(
                &mut *connection,
                authority,
                &refs,
                encryption_key.as_deref(),
            )
            .await?;
            context.with_resolved_keys(&expressions, keys)
        };

        let rendered = resolve_templates_with_sensitivity(action_params, &context)?;

        if rendered.value.as_object().is_some() {
            Ok(rendered)
        } else {
            Ok(RenderedJson::plain(serde_json::json!({})))
        }
    }

    async fn restore_event_payload_for_templates(
        connection: &mut PgConnection,
        encryption_key: &Option<String>,
        event_id: i64,
        event_payload: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *connection,
            ENTITY_EVENT_PAYLOAD,
            event_id,
        )
        .await?;
        let restored = if secrets.is_empty() {
            event_payload
        } else {
            let encryption_key = encryption_key.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "Cannot restore event payload secrets for rule templates without security.encryption_key"
                )
            })?;
            restore_secret_values(event_payload, &secrets, encryption_key)?
        };
        let binding = attune_common::repositories::event_schema::EventSchemaRepository::find(
            connection, event_id,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("Rule event is missing its admission schema binding"))?;
        binding.validate_payload_restored(Some(&restored))?;
        Ok(restored)
    }
}
