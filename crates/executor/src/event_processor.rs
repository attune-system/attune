//! Event Processor - Handles EventCreated messages and creates enforcements
//!
//! This component listens for EventCreated messages from the message queue,
//! finds matching rules for the event's trigger, evaluates conditions, and
//! creates enforcement records for rules that match.

use anyhow::Result;
use sqlx::PgPool;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use attune_common::{
    metadata_cache::{repositories::CachedMetadataRepository, MetadataCache},
    models::{EnforcementCondition, EnforcementStatus, Event, Rule},
    mq::{
        Consumer, EnforcementCreatedPayload, EventCreatedPayload, MessageEnvelope, MessageType,
        Publisher,
    },
    repositories::{
        event::{CreateEnforcementInput, EnforcementRepository, EventRepository},
        execution_secret_value::ExecutionSecretValueRepository,
        pack::PackRepository,
        rule::RuleRepository,
        FindById, List,
    },
    secret_values::{
        merge_schema_secret_redactions, prepare_secret_values, redacted_paths,
        restore_secret_values, secret_paths_from_schema, validate_secret_destination_paths,
        RenderedJson, ENTITY_ENFORCEMENT_CONFIG, ENTITY_EVENT_PAYLOAD,
    },
    template_resolver::{resolve_templates_with_sensitivity, TemplateContext},
    workflow::expression::{eval_expression, is_truthy, EvalContext, EvalResult},
};

struct EventConditionContext {
    event: serde_json::Value,
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
    metadata_cache: MetadataCache,
}

impl EventProcessor {
    /// Create a new event processor
    pub fn new(
        pool: PgPool,
        publisher: Arc<Publisher>,
        consumer: Arc<Consumer>,
        encryption_key: Option<String>,
        metadata_cache: MetadataCache,
    ) -> Self {
        Self {
            pool,
            publisher,
            consumer,
            encryption_key,
            metadata_cache,
        }
    }

    /// Start processing EventCreated messages
    pub async fn start(&self) -> Result<()> {
        info!("Starting event processor");

        let pool = self.pool.clone();
        let publisher = self.publisher.clone();
        let encryption_key = self.encryption_key.clone();
        let metadata_cache = self.metadata_cache.clone();

        // Use the handler pattern to consume messages
        self.consumer
            .consume_with_handler(move |envelope: MessageEnvelope<EventCreatedPayload>| {
                let pool = pool.clone();
                let publisher = publisher.clone();
                let encryption_key = encryption_key.clone();
                let metadata_cache = metadata_cache.clone();

                async move {
                    if let Err(e) = Self::process_event_created(
                        &pool,
                        &publisher,
                        &encryption_key,
                        &metadata_cache,
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
        metadata_cache: &MetadataCache,
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
        let matching_rules = Self::find_matching_rules(pool, metadata_cache, &event).await?;

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
            if let Err(e) = Self::create_enforcement(
                pool,
                publisher,
                encryption_key,
                metadata_cache,
                &rule,
                &event,
            )
            .await
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
        metadata_cache: &MetadataCache,
        event: &Event,
    ) -> Result<Vec<Rule>> {
        let cached = CachedMetadataRepository::new(pool, metadata_cache);

        // Check if event is associated with a specific rule
        if let Some(rule_id) = event.rule {
            // Event is for a specific rule - only match that rule
            info!(
                "Event {} is associated with specific rule ID: {}",
                event.id, rule_id
            );
            match cached.find_rule_by_id(rule_id).await? {
                Some(rule) => {
                    if rule.enabled {
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
            // No specific rule - match all enabled rules for trigger
            let all_rules = if let Some(trigger_id) = event.trigger {
                cached.find_rules_by_trigger(trigger_id).await?
            } else {
                RuleRepository::list(pool).await?
            };
            let matching_rules: Vec<Rule> = all_rules
                .into_iter()
                .filter(|r| r.enabled && r.trigger_ref == event.trigger_ref)
                .collect();

            Ok(matching_rules)
        }
    }

    /// Create an enforcement for a rule and event
    async fn create_enforcement(
        pool: &PgPool,
        publisher: &Publisher,
        encryption_key: &Option<String>,
        metadata_cache: &MetadataCache,
        rule: &Rule,
        event: &Event,
    ) -> Result<()> {
        // Evaluate rule conditions
        let conditions_pass = Self::evaluate_conditions(rule, event)?;

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

        // Resolve action parameters using the template resolver, then move
        // parameters marked `secret: true` into encrypted per-enforcement rows.
        let rendered_params =
            Self::resolve_action_params(pool, encryption_key, rule, event, &payload).await?;
        let cached = CachedMetadataRepository::new(pool, metadata_cache);
        let action = match rule.action {
            Some(action_id) => cached.find_action_by_id(action_id).await?,
            None => cached.find_action_by_ref(&rule.action_ref).await?,
        }
        .ok_or_else(|| anyhow::anyhow!("Action '{}' not found", rule.action_ref))?;
        validate_secret_destination_paths(
            action.param_schema.as_ref(),
            &rendered_params.secret_paths,
        )?;
        let (redacted_params, secret_inputs) = merge_schema_secret_redactions(
            rendered_params.value,
            &rendered_params.secret_path_sources,
            action.param_schema.as_ref(),
        );
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

        let enforcement_result =
            EnforcementRepository::create_or_get_by_rule_event(pool, create_input).await?;
        let enforcement = enforcement_result.enforcement;
        if enforcement_result.created && !prepared_secrets.is_empty() {
            ExecutionSecretValueRepository::upsert_many(
                pool,
                ENTITY_ENFORCEMENT_CONFIG,
                enforcement.id,
                &prepared_secrets,
            )
            .await?;
        }

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
        pool: &PgPool,
        encryption_key: &Option<String>,
        rule: &Rule,
        event: &Event,
        event_payload: &serde_json::Value,
    ) -> Result<RenderedJson> {
        let action_params = &rule.action_params;

        // If there are no action params, return empty
        if action_params.is_null() || action_params.as_object().is_none_or(|o| o.is_empty()) {
            return Ok(RenderedJson::plain(serde_json::json!({})));
        }

        // Load pack config from database for pack.config.* resolution
        let (pack_ref, pack_config, pack_secret_paths) = match PackRepository::find_by_id(
            pool, rule.pack,
        )
        .await
        {
            Ok(Some(pack)) => (
                Some(pack.r#ref),
                pack.config,
                secret_paths_from_schema(Some(&pack.conf_schema)),
            ),
            Ok(None) => {
                warn!(
                    "Pack {} not found for rule {} — pack.config.* templates will resolve to null",
                    rule.pack, rule.r#ref
                );
                (None, serde_json::json!({}), Vec::new())
            }
            Err(e) => {
                warn!("Failed to load pack {} for rule {}: {} — pack.config.* templates will resolve to null", rule.pack, rule.r#ref, e);
                (None, serde_json::json!({}), Vec::new())
            }
        };

        let event_payload_secret_paths = redacted_paths(event_payload);
        let event_payload_for_templates = Self::restore_event_payload_for_templates(
            pool,
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
        .with_event_payload_secret_paths(
            Some(event.trigger_ref.clone()),
            event_payload_secret_paths,
        )
        .with_pack_config_secret_paths(pack_ref, pack_secret_paths);

        let rendered = resolve_templates_with_sensitivity(action_params, &context)?;

        if rendered.value.as_object().is_some() {
            Ok(rendered)
        } else {
            Ok(RenderedJson::plain(serde_json::json!({})))
        }
    }

    async fn restore_event_payload_for_templates(
        pool: &PgPool,
        encryption_key: &Option<String>,
        event_id: i64,
        event_payload: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
            pool,
            ENTITY_EVENT_PAYLOAD,
            event_id,
        )
        .await?;
        if secrets.is_empty() {
            return Ok(event_payload);
        }

        let encryption_key = encryption_key.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "Cannot restore event payload secrets for rule templates without security.encryption_key"
            )
        })?;
        restore_secret_values(event_payload, &secrets, encryption_key).map_err(Into::into)
    }
}
