//! Enforcement Processor - Handles enforcement creation and processing
//!
//! This module is responsible for:
//! - Listening for EnforcementCreated messages
//! - Evaluating rule conditions and context
//! - Determining whether to create executions
//! - Applying execution policies (via PolicyEnforcer + QueueManager)
//! - Waiting for queue slot if concurrency limited
//! - Creating execution records
//! - Publishing ExecutionRequested messages

use anyhow::{bail, Result};
use attune_common::{
    models::{Enforcement, EnforcementStatus, Event, Rule},
    mq::{
        Consumer, EnforcementCreatedPayload, ExecutionRequestedPayload, MessageEnvelope, Publisher,
    },
    repositories::{
        action::ActionRepository,
        event::{EnforcementRepository, EventRepository, UpdateEnforcementInput},
        execution::{CreateExecutionInput, ExecutionRepository},
        execution_secret_value::ExecutionSecretValueRepository,
        rule::RuleRepository,
        FindById,
    },
    secret_values::{ENTITY_ENFORCEMENT_CONFIG, ENTITY_EXECUTION_CONFIG},
};

use sqlx::PgPool;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::policy_enforcer::PolicyEnforcer;
use crate::queue_manager::ExecutionQueueManager;

/// Enforcement processor that handles enforcement messages
pub struct EnforcementProcessor {
    pool: PgPool,
    publisher: Arc<Publisher>,
    consumer: Arc<Consumer>,
    policy_enforcer: Arc<PolicyEnforcer>,
    queue_manager: Arc<ExecutionQueueManager>,
}

impl EnforcementProcessor {
    /// Create a new enforcement processor
    pub fn new(
        pool: PgPool,
        publisher: Arc<Publisher>,
        consumer: Arc<Consumer>,
        policy_enforcer: Arc<PolicyEnforcer>,
        queue_manager: Arc<ExecutionQueueManager>,
    ) -> Self {
        Self {
            pool,
            publisher,
            consumer,
            policy_enforcer,
            queue_manager,
        }
    }

    /// Start processing enforcement messages
    pub async fn start(&self) -> Result<()> {
        info!("Starting enforcement processor");

        let pool = self.pool.clone();
        let publisher = self.publisher.clone();
        let policy_enforcer = self.policy_enforcer.clone();
        let queue_manager = self.queue_manager.clone();

        // Use the handler pattern to consume messages
        self.consumer
            .consume_with_handler(
                move |envelope: MessageEnvelope<EnforcementCreatedPayload>| {
                    let pool = pool.clone();
                    let publisher = publisher.clone();
                    let policy_enforcer = policy_enforcer.clone();
                    let queue_manager = queue_manager.clone();

                    async move {
                        if let Err(e) = Self::process_enforcement_created(
                            &pool,
                            &publisher,
                            &policy_enforcer,
                            &queue_manager,
                            &envelope,
                        )
                        .await
                        {
                            error!("Error processing enforcement: {}", e);
                            // Return error to trigger nack with requeue
                            return Err(format!("Failed to process enforcement: {}", e).into());
                        }
                        Ok(())
                    }
                },
            )
            .await?;

        Ok(())
    }

    /// Process an enforcement created message
    async fn process_enforcement_created(
        pool: &PgPool,
        publisher: &Publisher,
        policy_enforcer: &PolicyEnforcer,
        queue_manager: &ExecutionQueueManager,
        envelope: &MessageEnvelope<EnforcementCreatedPayload>,
    ) -> Result<()> {
        debug!("Processing enforcement message: {:?}", envelope);

        let enforcement_id = envelope.payload.enforcement_id;
        info!("Processing enforcement: {}", enforcement_id);

        // Fetch enforcement from database
        let enforcement = EnforcementRepository::find_by_id(pool, enforcement_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Enforcement not found: {}", enforcement_id))?;

        if enforcement.status != EnforcementStatus::Created {
            debug!(
                "Enforcement {} already left Created state ({:?}), skipping duplicate processing",
                enforcement_id, enforcement.status
            );
            return Ok(());
        }

        // Fetch associated rule
        let rule = RuleRepository::find_by_id(
            pool,
            enforcement.rule.ok_or_else(|| {
                anyhow::anyhow!("Enforcement {} has no associated rule", enforcement_id)
            })?,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("Rule not found for enforcement: {}", enforcement_id))?;

        // Fetch associated event if present
        let event = if let Some(event_id) = enforcement.event {
            EventRepository::find_by_id(pool, event_id).await?
        } else {
            None
        };

        // Evaluate whether to create execution
        if Self::should_create_execution(&enforcement, &rule, event.as_ref())? {
            let execution_created = Self::create_execution(
                pool,
                publisher,
                policy_enforcer,
                queue_manager,
                &enforcement,
                &rule,
            )
            .await?;

            let updated = EnforcementRepository::update_loaded_if_status(
                pool,
                &enforcement,
                EnforcementStatus::Created,
                UpdateEnforcementInput {
                    status: Some(EnforcementStatus::Processed),
                    payload: None,
                    resolved_at: Some(chrono::Utc::now()),
                },
            )
            .await?;

            if updated.is_some() {
                debug!(
                    "Updated enforcement {} status to Processed after {} execution path",
                    enforcement_id,
                    if execution_created {
                        "new"
                    } else {
                        "idempotent"
                    }
                );
            }
        } else {
            info!(
                "Skipping execution creation for enforcement: {}",
                enforcement_id
            );

            let updated = EnforcementRepository::update_loaded_if_status(
                pool,
                &enforcement,
                EnforcementStatus::Created,
                UpdateEnforcementInput {
                    status: Some(EnforcementStatus::Disabled),
                    payload: None,
                    resolved_at: Some(chrono::Utc::now()),
                },
            )
            .await?;

            if updated.is_some() {
                debug!(
                    "Updated enforcement {} status to Disabled (skipped)",
                    enforcement_id
                );
            }
        }

        Ok(())
    }

    /// Determine if an execution should be created for this enforcement
    fn should_create_execution(
        enforcement: &Enforcement,
        rule: &Rule,
        _event: Option<&Event>,
    ) -> Result<bool> {
        // Check if rule is enabled
        if !rule.enabled {
            warn!("Rule {} is disabled, skipping execution", rule.id);
            return Ok(false);
        }

        // Check if the rule's action still exists (may have been deleted with its pack)
        if rule.action.is_none() {
            warn!(
                "Rule {} references a deleted action (action_ref: {}), skipping execution",
                rule.id, rule.action_ref
            );
            return Ok(false);
        }

        // Check if the rule's trigger still exists
        if rule.trigger.is_none() {
            warn!(
                "Rule {} references a deleted trigger (trigger_ref: {}), skipping execution",
                rule.id, rule.trigger_ref
            );
            return Ok(false);
        }

        // TODO: Evaluate rule conditions against event payload
        // For now, we'll create executions for all valid enforcements

        debug!(
            "Enforcement {} passed validation, will create execution",
            enforcement.id
        );

        Ok(true)
    }

    /// Create an execution record for the enforcement
    async fn create_execution(
        pool: &PgPool,
        publisher: &Publisher,
        _policy_enforcer: &PolicyEnforcer,
        _queue_manager: &ExecutionQueueManager,
        enforcement: &Enforcement,
        rule: &Rule,
    ) -> Result<bool> {
        // Extract action ID — should_create_execution already verified it's Some,
        // but guard defensively here as well.
        let action_id = match rule.action {
            Some(id) => id,
            None => {
                error!(
                    "Rule {} has no action ID (deleted?), cannot create execution for enforcement {}",
                    rule.id, enforcement.id
                );
                bail!(
                    "Rule {} references a deleted action (action_ref: {})",
                    rule.id,
                    rule.action_ref
                );
            }
        };

        info!(
            "Creating execution for enforcement: {}, rule: {}, action: {}",
            enforcement.id, rule.id, action_id
        );

        let action_ref = &rule.action_ref;
        let action = ActionRepository::find_by_id(pool, action_id).await?;
        let action_default_permission_set_refs = action
            .as_ref()
            .map(|action| action.default_execution_permission_set_refs.clone())
            .unwrap_or_default();
        let permission_set_refs = rule
            .permission_set_refs
            .clone()
            .unwrap_or(action_default_permission_set_refs);
        let artifact_retention_policy = action
            .as_ref()
            .and_then(|action| action.artifact_retention_policy);
        let artifact_retention_limit = action
            .as_ref()
            .and_then(|action| action.artifact_retention_limit);
        let timeout_seconds = Some(
            action
                .as_ref()
                .and_then(|action| action.timeout_seconds)
                .unwrap_or(attune_common::config::app_default_execution_timeout_seconds() as i32),
        );

        // Create the execution row first; scheduler-side policy enforcement
        // now handles both rule-triggered and manual executions uniformly.
        //
        // SECURITY: Attribute the execution to the rule's owner identity (the
        // user who registered/authored the rule). Legacy or system-loaded
        // rules with NULL `owner_identity` fall back to the system identity
        // (id 1) so the worker still mints a callback token with a known
        // `sub` claim. This is intentionally permissive for the init-pack
        // loader path; new rules created via the API always carry the
        // authenticated user's identity.
        const SYSTEM_IDENTITY_ID: i64 = 1;
        let executor_identity = rule.owner_identity.unwrap_or(SYSTEM_IDENTITY_ID);
        let execution_input = CreateExecutionInput {
            action: Some(action_id),
            action_ref: action_ref.clone(),
            config: enforcement.config.clone(),
            env_vars: None, // No custom env vars for rule-triggered executions
            parent: None,   // TODO: Handle workflow parent-child relationships
            enforcement: Some(enforcement.id),
            executor: Some(executor_identity),
            permission_set_refs,
            artifact_retention_policy,
            artifact_retention_limit,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            timeout_seconds,
            result: None,
            workflow_task: None, // Non-workflow execution
        };

        let execution_result = ExecutionRepository::create_top_level_for_enforcement_if_absent(
            pool,
            execution_input,
            enforcement.id,
        )
        .await?;
        let execution = execution_result.execution;
        if execution_result.created {
            ExecutionSecretValueRepository::copy_entity(
                pool,
                ENTITY_ENFORCEMENT_CONFIG,
                enforcement.id,
                ENTITY_EXECUTION_CONFIG,
                execution.id,
            )
            .await?;
        }

        if execution_result.created {
            info!(
                "Created execution: {} for enforcement: {}",
                execution.id, enforcement.id
            );
        } else {
            info!(
                "Reusing execution: {} for enforcement: {}",
                execution.id, enforcement.id
            );
        }

        if execution_result.created
            || execution.status == attune_common::models::enums::ExecutionStatus::Requested
        {
            let payload = ExecutionRequestedPayload {
                execution_id: execution.id,
                action_id: Some(action_id),
                action_ref: action_ref.clone(),
                parent_id: None,
                enforcement_id: Some(enforcement.id),
                config: execution.config.clone(),
            };

            let envelope =
                MessageEnvelope::new(attune_common::mq::MessageType::ExecutionRequested, payload)
                    .with_source("executor");

            // Publish to execution requests queue with routing key
            let routing_key = "execution.requested";
            let exchange = "attune.executions";

            publisher
                .publish_envelope_with_routing(&envelope, exchange, routing_key)
                .await?;

            info!(
                "Published execution.requested message for execution: {} (enforcement: {}, action: {})",
                execution.id, enforcement.id, action_id
            );
        }

        // NOTE: Queue slot will be released when worker publishes execution.completed
        // and CompletionListener calls queue_manager.notify_completion(action_id)

        Ok(execution_result.created)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_create_execution_disabled_rule() {
        use serde_json::json;

        let enforcement = Enforcement {
            id: 1,
            rule: Some(1),
            rule_ref: "test.rule".to_string(),
            trigger_ref: "test.trigger".to_string(),
            event: Some(1),
            config: None,
            status: attune_common::models::enums::EnforcementStatus::Processed,
            payload: json!({}),
            condition: attune_common::models::enums::EnforcementCondition::Any,
            conditions: json!({}),
            created: chrono::Utc::now(),
            resolved_at: Some(chrono::Utc::now()),
        };

        let mut rule = Rule {
            id: 1,
            r#ref: "test.rule".to_string(),
            pack: 1,
            pack_ref: "test".to_string(),
            label: "Test Rule".to_string(),
            description: Some("Test rule description".to_string()),
            trigger_ref: "test.trigger".to_string(),
            trigger: Some(1),
            action_ref: "test.action".to_string(),
            action: Some(1),
            enabled: false, // Disabled
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            is_adhoc: false,
            owner_identity: None,
            created: chrono::Utc::now(),
            updated: chrono::Utc::now(),
        };

        let result = EnforcementProcessor::should_create_execution(&enforcement, &rule, None);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Should not create execution

        // Test with enabled rule
        rule.enabled = true;
        let result = EnforcementProcessor::should_create_execution(&enforcement, &rule, None);
        assert!(result.is_ok());
        assert!(result.unwrap()); // Should create execution
    }
}
