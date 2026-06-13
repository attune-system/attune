//! Read-through metadata repository facade backed by [`MetadataCache`].

use sqlx::PgPool;
use std::collections::HashMap;
use tracing::debug;

use crate::metadata_cache::{MetadataCache, MetadataEntity};
use crate::models::{
    Action, Id, PermissionSet, Policy, Rule, Runtime, RuntimeVersion, Sensor, Trigger, WorkQueue,
    WorkflowDefinition,
};
use crate::repositories::{
    action::{ActionRepository, PolicyRepository},
    identity::PermissionSetRepository,
    rule::RuleRepository,
    runtime::RuntimeRepository,
    runtime_version::RuntimeVersionRepository,
    trigger::{SensorRepository, TriggerRepository},
    work_queue::WorkQueueRepository,
    workflow::WorkflowDefinitionRepository,
    FindById, FindByRef, List,
};
use crate::Result;

/// Explicit cached facade for metadata reads.
///
/// The underlying SQL repositories remain the system of record and continue to
/// support transactions. This facade is intended for service/API read paths that
/// have a `PgPool` and can safely fall back to PostgreSQL on cache miss/error.
pub struct CachedMetadataRepository<'a> {
    db: &'a PgPool,
    cache: &'a MetadataCache,
}

impl<'a> CachedMetadataRepository<'a> {
    pub fn new(db: &'a PgPool, cache: &'a MetadataCache) -> Self {
        Self { db, cache }
    }

    pub async fn find_action_by_id(&self, id: Id) -> Result<Option<Action>> {
        self.find_by_id(
            MetadataEntity::Action,
            id,
            || async { ActionRepository::find_by_id(self.db, id).await },
            |row| async move { self.put_action_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_action_by_ref(&self, ref_str: &str) -> Result<Option<Action>> {
        self.find_by_ref(
            MetadataEntity::Action,
            ref_str,
            || async { ActionRepository::find_by_ref(self.db, ref_str).await },
            |row| async move { self.put_action_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_actions_by_pack(&self, pack_id: Id) -> Result<Vec<Action>> {
        if let Some(actions) = self
            .try_find_cached_index::<Action>(
                MetadataEntity::Action,
                &format!("pack:{pack_id}:refs"),
                "action.find_by_pack",
            )
            .await?
        {
            return Ok(actions);
        }

        let actions = ActionRepository::find_by_pack(self.db, pack_id).await?;
        for action in &actions {
            self.put_action_best_effort(action).await;
        }
        Ok(actions)
    }

    pub async fn find_action_by_workflow_def(&self, workflow_def_id: Id) -> Result<Option<Action>> {
        if let Some(mut actions) = self
            .try_find_cached_index::<Action>(
                MetadataEntity::Action,
                &format!("workflow_def:{workflow_def_id}:refs"),
                "action.find_by_workflow_def",
            )
            .await?
        {
            return Ok(actions.pop());
        }

        if let Some(action) =
            ActionRepository::find_by_workflow_def(self.db, workflow_def_id).await?
        {
            self.put_action_best_effort(&action).await;
            return Ok(Some(action));
        }
        Ok(None)
    }

    pub async fn put_action_best_effort(&self, action: &Action) {
        self.put_row_best_effort(
            MetadataEntity::Action,
            action.id,
            &action.r#ref,
            action,
            action_indexes(action),
        )
        .await;
    }

    pub async fn evict_action_best_effort(&self, action: &Action) {
        self.evict_row_best_effort(
            MetadataEntity::Action,
            action.id,
            &action.r#ref,
            action_indexes(action),
        )
        .await;
    }

    pub async fn find_rule_by_id(&self, id: Id) -> Result<Option<Rule>> {
        self.find_by_id(
            MetadataEntity::Rule,
            id,
            || async { RuleRepository::find_by_id(self.db, id).await },
            |row| async move { self.put_rule_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_rule_by_ref(&self, ref_str: &str) -> Result<Option<Rule>> {
        self.find_by_ref(
            MetadataEntity::Rule,
            ref_str,
            || async { RuleRepository::find_by_ref(self.db, ref_str).await },
            |row| async move { self.put_rule_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_rules_by_trigger(&self, trigger_id: Id) -> Result<Vec<Rule>> {
        if let Some(rules) = self
            .try_find_cached_index::<Rule>(
                MetadataEntity::Rule,
                &format!("trigger:{trigger_id}:refs"),
                "rule.find_by_trigger",
            )
            .await?
        {
            return Ok(rules);
        }

        let rules = RuleRepository::find_by_trigger(self.db, trigger_id).await?;
        for rule in &rules {
            self.put_rule_best_effort(rule).await;
        }
        Ok(rules)
    }

    pub async fn put_rule_best_effort(&self, rule: &Rule) {
        self.put_row_best_effort(
            MetadataEntity::Rule,
            rule.id,
            &rule.r#ref,
            rule,
            rule_indexes(rule),
        )
        .await;
    }

    pub async fn evict_rule_best_effort(&self, rule: &Rule) {
        self.evict_row_best_effort(
            MetadataEntity::Rule,
            rule.id,
            &rule.r#ref,
            rule_indexes(rule),
        )
        .await;
    }

    pub async fn find_trigger_by_id(&self, id: Id) -> Result<Option<Trigger>> {
        self.find_by_id(
            MetadataEntity::Trigger,
            id,
            || async { TriggerRepository::find_by_id(self.db, id).await },
            |row| async move { self.put_trigger_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_trigger_by_ref(&self, ref_str: &str) -> Result<Option<Trigger>> {
        self.find_by_ref(
            MetadataEntity::Trigger,
            ref_str,
            || async { TriggerRepository::find_by_ref(self.db, ref_str).await },
            |row| async move { self.put_trigger_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_trigger_by_webhook_key(&self, webhook_key: &str) -> Result<Option<Trigger>> {
        let index = format!("webhook_key:{webhook_key}:refs");
        if let Some(mut triggers) = self
            .try_find_cached_index::<Trigger>(
                MetadataEntity::Trigger,
                &index,
                "trigger.find_by_webhook_key",
            )
            .await?
        {
            return Ok(triggers.pop());
        }

        if let Some(trigger) = TriggerRepository::find_by_webhook_key(self.db, webhook_key).await? {
            self.put_trigger_best_effort(&trigger).await;
            return Ok(Some(trigger));
        }
        Ok(None)
    }

    pub async fn find_triggers_by_sensor(&self, sensor_id: Id) -> Result<Vec<Trigger>> {
        if let Some(triggers) = self
            .try_find_cached_index::<Trigger>(
                MetadataEntity::Trigger,
                &format!("sensor:{sensor_id}:refs"),
                "trigger.find_by_sensor",
            )
            .await?
        {
            return Ok(triggers);
        }

        let triggers = TriggerRepository::find_by_sensor(self.db, sensor_id).await?;
        for trigger in &triggers {
            self.put_trigger_best_effort(trigger).await;
        }
        Ok(triggers)
    }

    pub async fn put_trigger_best_effort(&self, trigger: &Trigger) {
        self.put_row_best_effort(
            MetadataEntity::Trigger,
            trigger.id,
            &trigger.r#ref,
            trigger,
            trigger_indexes(trigger),
        )
        .await;
    }

    pub async fn evict_trigger_best_effort(&self, trigger: &Trigger) {
        self.evict_row_best_effort(
            MetadataEntity::Trigger,
            trigger.id,
            &trigger.r#ref,
            trigger_indexes(trigger),
        )
        .await;
    }

    pub async fn find_sensor_by_id(&self, id: Id) -> Result<Option<Sensor>> {
        self.find_by_id(
            MetadataEntity::Sensor,
            id,
            || async { SensorRepository::find_by_id(self.db, id).await },
            |row| async move { self.put_sensor_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_sensor_by_ref(&self, ref_str: &str) -> Result<Option<Sensor>> {
        self.find_by_ref(
            MetadataEntity::Sensor,
            ref_str,
            || async { SensorRepository::find_by_ref(self.db, ref_str).await },
            |row| async move { self.put_sensor_best_effort(&row).await },
        )
        .await
    }

    pub async fn put_sensor_best_effort(&self, sensor: &Sensor) {
        self.put_row_best_effort(
            MetadataEntity::Sensor,
            sensor.id,
            &sensor.r#ref,
            sensor,
            sensor_indexes(sensor),
        )
        .await;
    }

    pub async fn evict_sensor_best_effort(&self, sensor: &Sensor) {
        self.evict_row_best_effort(
            MetadataEntity::Sensor,
            sensor.id,
            &sensor.r#ref,
            sensor_indexes(sensor),
        )
        .await;
    }

    pub async fn find_work_queue_by_id(&self, id: Id) -> Result<Option<WorkQueue>> {
        self.find_by_id(
            MetadataEntity::WorkQueue,
            id,
            || async { WorkQueueRepository::find_by_id(self.db, id).await },
            |row| async move { self.put_work_queue_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_work_queue_by_ref(&self, ref_str: &str) -> Result<Option<WorkQueue>> {
        self.find_by_ref(
            MetadataEntity::WorkQueue,
            ref_str,
            || async { WorkQueueRepository::find_by_ref(self.db, ref_str).await },
            |row| async move { self.put_work_queue_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_work_queues_by_pack(&self, pack_id: Id) -> Result<Vec<WorkQueue>> {
        if let Some(queues) = self
            .try_find_cached_index(
                MetadataEntity::WorkQueue,
                &format!("pack:{pack_id}:refs"),
                "work_queue.find_by_pack",
            )
            .await?
        {
            return Ok(queues);
        }

        let queues = WorkQueueRepository::find_by_pack(self.db, pack_id).await?;
        for queue in &queues {
            self.put_work_queue_best_effort(queue).await;
        }
        Ok(queues)
    }

    pub async fn find_enabled_work_queues(&self) -> Result<Vec<WorkQueue>> {
        if let Some(queues) = self
            .try_find_cached_index::<WorkQueue>(
                MetadataEntity::WorkQueue,
                "enabled_refs",
                "work_queue.find_enabled",
            )
            .await?
        {
            return Ok(queues.into_iter().filter(|queue| queue.enabled).collect());
        }

        let queues = WorkQueueRepository::search(
            self.db,
            &crate::repositories::work_queue::WorkQueueSearchFilters {
                enabled: Some(true),
                limit: u32::MAX,
                ..Default::default()
            },
        )
        .await?
        .rows;
        for queue in &queues {
            self.put_work_queue_best_effort(queue).await;
        }
        Ok(queues)
    }

    pub async fn put_work_queue_best_effort(&self, queue: &WorkQueue) {
        self.put_row_best_effort(
            MetadataEntity::WorkQueue,
            queue.id,
            &queue.r#ref,
            queue,
            work_queue_indexes(queue),
        )
        .await;
    }

    pub async fn evict_work_queue_best_effort(&self, queue: &WorkQueue) {
        self.evict_row_best_effort(
            MetadataEntity::WorkQueue,
            queue.id,
            &queue.r#ref,
            work_queue_indexes(queue),
        )
        .await;
    }

    pub async fn find_workflow_definition_by_id(
        &self,
        id: Id,
    ) -> Result<Option<WorkflowDefinition>> {
        self.find_by_id(
            MetadataEntity::WorkflowDefinition,
            id,
            || async { WorkflowDefinitionRepository::find_by_id(self.db, id).await },
            |row| async move { self.put_workflow_definition_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_workflow_definition_by_ref(
        &self,
        ref_str: &str,
    ) -> Result<Option<WorkflowDefinition>> {
        self.find_by_ref(
            MetadataEntity::WorkflowDefinition,
            ref_str,
            || async { WorkflowDefinitionRepository::find_by_ref(self.db, ref_str).await },
            |row| async move { self.put_workflow_definition_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_workflow_definitions_by_pack(
        &self,
        pack_id: Id,
    ) -> Result<Vec<WorkflowDefinition>> {
        if let Some(workflows) = self
            .try_find_cached_index(
                MetadataEntity::WorkflowDefinition,
                &format!("pack:{pack_id}:refs"),
                "workflow_definition.find_by_pack",
            )
            .await?
        {
            return Ok(workflows);
        }

        let workflows = WorkflowDefinitionRepository::find_by_pack(self.db, pack_id).await?;
        for workflow in &workflows {
            self.put_workflow_definition_best_effort(workflow).await;
        }
        Ok(workflows)
    }

    pub async fn put_workflow_definition_best_effort(&self, workflow: &WorkflowDefinition) {
        self.put_row_best_effort(
            MetadataEntity::WorkflowDefinition,
            workflow.id,
            &workflow.r#ref,
            workflow,
            pack_indexes(workflow.pack),
        )
        .await;
    }

    pub async fn evict_workflow_definition_best_effort(&self, workflow: &WorkflowDefinition) {
        self.evict_row_best_effort(
            MetadataEntity::WorkflowDefinition,
            workflow.id,
            &workflow.r#ref,
            pack_indexes(workflow.pack),
        )
        .await;
    }

    pub async fn find_policy_by_id(&self, id: Id) -> Result<Option<Policy>> {
        self.find_by_id(
            MetadataEntity::Policy,
            id,
            || async { PolicyRepository::find_by_id(self.db, id).await },
            |row| async move { self.put_policy_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_policy_by_ref(&self, ref_str: &str) -> Result<Option<Policy>> {
        self.find_by_ref(
            MetadataEntity::Policy,
            ref_str,
            || async { PolicyRepository::find_by_ref(self.db, ref_str).await },
            |row| async move { self.put_policy_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_policies_by_action(&self, action_id: Id) -> Result<Vec<Policy>> {
        let index = format!("action:{action_id}:refs");
        if let Some(policies) = self
            .try_find_cached_index_with_empty_marker(
                MetadataEntity::Policy,
                &index,
                "policy.find_by_action",
            )
            .await?
        {
            return Ok(policies);
        }

        let policies = PolicyRepository::find_by_action(self.db, action_id).await?;
        for policy in &policies {
            self.put_policy_best_effort(policy).await;
        }
        if policies.is_empty() {
            self.mark_index_empty_best_effort(MetadataEntity::Policy, &index)
                .await;
        }
        Ok(policies)
    }

    pub async fn find_latest_policy_by_action(&self, action_id: Id) -> Result<Option<Policy>> {
        let policies = self.find_policies_by_action(action_id).await?;
        if !policies.is_empty() {
            return Ok(policies.into_iter().max_by_key(|policy| policy.created));
        }

        Ok(None)
    }

    pub async fn find_latest_policy_by_pack(&self, pack_id: Id) -> Result<Option<Policy>> {
        let index = format!("pack:{pack_id}:refs");
        if let Some(policies) = self
            .try_find_cached_index_with_empty_marker::<Policy>(
                MetadataEntity::Policy,
                &index,
                "policy.find_latest_by_pack",
            )
            .await?
        {
            return Ok(policies
                .into_iter()
                .filter(|policy| policy.action.is_none())
                .max_by_key(|policy| policy.created));
        }

        if let Some(policy) = PolicyRepository::find_latest_by_pack(self.db, pack_id).await? {
            self.put_policy_best_effort(&policy).await;
            return Ok(Some(policy));
        }

        self.mark_index_empty_best_effort(MetadataEntity::Policy, &index)
            .await;
        Ok(None)
    }

    pub async fn find_latest_global_policy(&self) -> Result<Option<Policy>> {
        let index = "global_refs";
        if let Some(policies) = self
            .try_find_cached_index_with_empty_marker::<Policy>(
                MetadataEntity::Policy,
                index,
                "policy.find_latest_global",
            )
            .await?
        {
            return Ok(policies
                .into_iter()
                .filter(|policy| policy.pack.is_none() && policy.action.is_none())
                .max_by_key(|policy| policy.created));
        }

        if let Some(policy) = PolicyRepository::find_latest_global(self.db).await? {
            self.put_policy_best_effort(&policy).await;
            return Ok(Some(policy));
        }

        self.mark_index_empty_best_effort(MetadataEntity::Policy, index)
            .await;
        Ok(None)
    }

    pub async fn find_effective_policy(
        &self,
        action_id: Id,
        pack_id: Option<Id>,
    ) -> Result<Option<Policy>> {
        if let Some(policy) = self
            .try_find_effective_policy_cached(action_id, pack_id)
            .await?
        {
            return Ok(policy);
        }

        if let Some(policy) = PolicyRepository::find_by_action(self.db, action_id)
            .await?
            .into_iter()
            .max_by_key(|policy| policy.created)
        {
            self.put_policy_best_effort(&policy).await;
            return Ok(Some(policy));
        }
        self.mark_index_empty_best_effort(
            MetadataEntity::Policy,
            &format!("action:{action_id}:refs"),
        )
        .await;

        if let Some(pack_id) = pack_id {
            if let Some(policy) = PolicyRepository::find_latest_by_pack(self.db, pack_id).await? {
                self.put_policy_best_effort(&policy).await;
                return Ok(Some(policy));
            }
            self.mark_index_empty_best_effort(
                MetadataEntity::Policy,
                &format!("pack:{pack_id}:refs"),
            )
            .await;
        }

        if let Some(policy) = PolicyRepository::find_latest_global(self.db).await? {
            self.put_policy_best_effort(&policy).await;
            return Ok(Some(policy));
        }

        self.mark_index_empty_best_effort(MetadataEntity::Policy, "global_refs")
            .await;
        Ok(None)
    }

    pub async fn put_policy_best_effort(&self, policy: &Policy) {
        self.put_row_best_effort(
            MetadataEntity::Policy,
            policy.id,
            &policy.r#ref,
            policy,
            policy_indexes(policy),
        )
        .await;
    }

    pub async fn evict_policy_best_effort(&self, policy: &Policy) {
        self.evict_row_best_effort(
            MetadataEntity::Policy,
            policy.id,
            &policy.r#ref,
            policy_indexes(policy),
        )
        .await;
    }

    pub async fn find_permission_set_by_id(&self, id: Id) -> Result<Option<PermissionSet>> {
        self.find_by_id(
            MetadataEntity::PermissionSet,
            id,
            || async { PermissionSetRepository::find_by_id(self.db, id).await },
            |row| async move { self.put_permission_set_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_permission_set_by_ref(&self, ref_str: &str) -> Result<Option<PermissionSet>> {
        self.find_by_ref(
            MetadataEntity::PermissionSet,
            ref_str,
            || async { PermissionSetRepository::find_by_ref(self.db, ref_str).await },
            |row| async move { self.put_permission_set_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_permission_sets_by_refs(
        &self,
        refs: &[String],
    ) -> Result<Vec<PermissionSet>> {
        if refs.is_empty() {
            return Ok(Vec::new());
        }
        if self.cache.entity_enabled(MetadataEntity::PermissionSet) {
            let keys: Vec<String> = refs
                .iter()
                .filter_map(|ref_str| {
                    self.cache
                        .key_for_ref(MetadataEntity::PermissionSet, ref_str)
                })
                .collect();
            if keys.len() == refs.len() {
                match self.cache.get_json_many::<PermissionSet, _, _>(&keys).await {
                    Ok(rows) if rows.len() == refs.len() && rows.iter().all(Option::is_some) => {
                        let mut permission_sets: Vec<_> = rows.into_iter().flatten().collect();
                        permission_sets.sort_by(|a, b| a.r#ref.cmp(&b.r#ref));
                        return Ok(permission_sets);
                    }
                    Ok(_) => {}
                    Err(e) => self
                        .cache
                        .log_best_effort_error("permission_set.find_by_refs.multi_get", &e),
                }
            }
        }

        let permission_sets = PermissionSetRepository::find_by_refs(self.db, refs).await?;
        for permission_set in &permission_sets {
            self.put_permission_set_best_effort(permission_set).await;
        }
        Ok(permission_sets)
    }

    pub async fn find_permission_sets_by_identity(
        &self,
        identity_id: Id,
    ) -> Result<Vec<PermissionSet>> {
        let index = format!("identity:{identity_id}:refs");
        if let Some(permission_sets) = self
            .try_find_cached_index_with_empty_marker::<PermissionSet>(
                MetadataEntity::PermissionSet,
                &index,
                "permission_set.find_by_identity",
            )
            .await?
        {
            return Ok(permission_sets);
        }

        let permission_sets =
            PermissionSetRepository::find_by_identity(self.db, identity_id).await?;
        for permission_set in &permission_sets {
            self.put_permission_set_best_effort(permission_set).await;
        }
        self.put_index_members_or_empty_best_effort(
            MetadataEntity::PermissionSet,
            &index,
            permission_sets
                .iter()
                .map(|permission_set| permission_set.r#ref.as_str()),
        )
        .await;
        Ok(permission_sets)
    }

    pub async fn find_permission_sets_by_roles(
        &self,
        roles: &[String],
    ) -> Result<Vec<PermissionSet>> {
        if roles.is_empty() {
            return Ok(Vec::new());
        }

        if self.cache.entity_enabled(MetadataEntity::PermissionSet) {
            let mut refs = Vec::new();
            let mut all_indexes_hit = true;
            for role in roles {
                let index = format!("role:{role}:refs");
                match self
                    .try_find_cached_index_with_empty_marker::<PermissionSet>(
                        MetadataEntity::PermissionSet,
                        &index,
                        "permission_set.find_by_roles",
                    )
                    .await?
                {
                    Some(permission_sets) => {
                        refs.extend(
                            permission_sets
                                .into_iter()
                                .map(|permission_set| permission_set.r#ref),
                        );
                    }
                    None => {
                        all_indexes_hit = false;
                        break;
                    }
                }
            }

            if all_indexes_hit {
                refs.sort();
                refs.dedup();
                return self.find_permission_sets_by_refs(&refs).await;
            }
        }

        let mut permission_sets = Vec::new();
        for role in roles {
            let role_permission_sets =
                PermissionSetRepository::find_by_roles(self.db, std::slice::from_ref(role)).await?;
            for permission_set in &role_permission_sets {
                self.put_permission_set_best_effort(permission_set).await;
            }
            self.put_index_members_or_empty_best_effort(
                MetadataEntity::PermissionSet,
                &format!("role:{role}:refs"),
                role_permission_sets
                    .iter()
                    .map(|permission_set| permission_set.r#ref.as_str()),
            )
            .await;
            permission_sets.extend(role_permission_sets);
        }
        permission_sets.sort_by(|a, b| a.r#ref.cmp(&b.r#ref));
        permission_sets.dedup_by(|a, b| a.r#ref == b.r#ref);
        Ok(permission_sets)
    }

    pub async fn put_permission_set_best_effort(&self, permission_set: &PermissionSet) {
        self.put_row_best_effort(
            MetadataEntity::PermissionSet,
            permission_set.id,
            &permission_set.r#ref,
            permission_set,
            optional_pack_indexes(permission_set.pack),
        )
        .await;
    }

    pub async fn evict_permission_set_best_effort(&self, permission_set: &PermissionSet) {
        self.evict_row_best_effort(
            MetadataEntity::PermissionSet,
            permission_set.id,
            &permission_set.r#ref,
            optional_pack_indexes(permission_set.pack),
        )
        .await;
    }

    pub async fn find_runtime_by_id(&self, id: Id) -> Result<Option<Runtime>> {
        self.find_by_id(
            MetadataEntity::Runtime,
            id,
            || async { RuntimeRepository::find_by_id(self.db, id).await },
            |row| async move { self.put_runtime_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_runtime_by_ref(&self, ref_str: &str) -> Result<Option<Runtime>> {
        self.find_by_ref(
            MetadataEntity::Runtime,
            ref_str,
            || async { RuntimeRepository::find_by_ref(self.db, ref_str).await },
            |row| async move { self.put_runtime_best_effort(&row).await },
        )
        .await
    }

    pub async fn find_runtime_by_name(&self, name: &str) -> Result<Option<Runtime>> {
        let index = format!("name:{}:refs", normalized_index_value(name));
        if let Some(mut runtimes) = self
            .try_find_cached_index::<Runtime>(
                MetadataEntity::Runtime,
                &index,
                "runtime.find_by_name",
            )
            .await?
        {
            return Ok(runtimes.pop());
        }

        if let Some(runtime) = RuntimeRepository::find_by_name(self.db, name).await? {
            self.put_runtime_best_effort(&runtime).await;
            return Ok(Some(runtime));
        }
        Ok(None)
    }

    pub async fn find_runtime_by_alias(&self, alias: &str) -> Result<Option<Runtime>> {
        let index = format!("alias:{}:refs", normalized_index_value(alias));
        if let Some(mut runtimes) = self
            .try_find_cached_index::<Runtime>(
                MetadataEntity::Runtime,
                &index,
                "runtime.find_by_alias",
            )
            .await?
        {
            return Ok(runtimes.pop());
        }

        if let Some(runtime) = RuntimeRepository::find_by_alias(self.db, alias).await? {
            self.put_runtime_best_effort(&runtime).await;
            return Ok(Some(runtime));
        }
        Ok(None)
    }

    pub async fn find_runtimes_by_pack(&self, pack_id: Id) -> Result<Vec<Runtime>> {
        if let Some(runtimes) = self
            .try_find_cached_index::<Runtime>(
                MetadataEntity::Runtime,
                &format!("pack:{pack_id}:refs"),
                "runtime.find_by_pack",
            )
            .await?
        {
            return Ok(runtimes);
        }

        let runtimes = RuntimeRepository::find_by_pack(self.db, pack_id).await?;
        for runtime in &runtimes {
            self.put_runtime_best_effort(runtime).await;
        }
        Ok(runtimes)
    }

    pub async fn list_runtimes(&self) -> Result<Vec<Runtime>> {
        if let Some(runtimes) = self
            .try_find_cached_index::<Runtime>(MetadataEntity::Runtime, "all_refs", "runtime.list")
            .await?
        {
            return Ok(runtimes);
        }

        let runtimes = RuntimeRepository::list(self.db).await?;
        for runtime in &runtimes {
            self.put_runtime_best_effort(runtime).await;
        }
        Ok(runtimes)
    }

    pub async fn put_runtime_best_effort(&self, runtime: &Runtime) {
        self.put_row_best_effort(
            MetadataEntity::Runtime,
            runtime.id,
            &runtime.r#ref,
            runtime,
            runtime_indexes(runtime),
        )
        .await;
    }

    pub async fn evict_runtime_best_effort(&self, runtime: &Runtime) {
        self.evict_row_best_effort(
            MetadataEntity::Runtime,
            runtime.id,
            &runtime.r#ref,
            runtime_indexes(runtime),
        )
        .await;
    }

    pub async fn find_runtime_version_by_id(&self, id: Id) -> Result<Option<RuntimeVersion>> {
        self.find_by_id(
            MetadataEntity::RuntimeVersion,
            id,
            || async { RuntimeVersionRepository::find_by_id(self.db, id).await },
            |row| async move { self.put_runtime_version_best_effort(&row).await },
        )
        .await
    }

    pub async fn list_runtime_versions(&self) -> Result<Vec<RuntimeVersion>> {
        if let Some(versions) = self
            .try_find_cached_index::<RuntimeVersion>(
                MetadataEntity::RuntimeVersion,
                "all_refs",
                "runtime_version.list",
            )
            .await?
        {
            return Ok(sort_runtime_versions(versions));
        }

        let versions = RuntimeVersionRepository::list(self.db).await?;
        for version in &versions {
            self.put_runtime_version_best_effort(version).await;
        }
        Ok(versions)
    }

    pub async fn find_runtime_versions_by_runtime(
        &self,
        runtime_id: Id,
    ) -> Result<Vec<RuntimeVersion>> {
        if let Some(versions) = self
            .try_find_cached_index::<RuntimeVersion>(
                MetadataEntity::RuntimeVersion,
                &format!("runtime:{runtime_id}:refs"),
                "runtime_version.find_by_runtime",
            )
            .await?
        {
            return Ok(sort_runtime_versions(versions));
        }

        let versions = RuntimeVersionRepository::find_by_runtime(self.db, runtime_id).await?;
        for version in &versions {
            self.put_runtime_version_best_effort(version).await;
        }
        Ok(versions)
    }

    pub async fn put_runtime_version_best_effort(&self, version: &RuntimeVersion) {
        self.put_row_best_effort(
            MetadataEntity::RuntimeVersion,
            version.id,
            &runtime_version_cache_ref(version),
            version,
            runtime_version_indexes(version),
        )
        .await;
    }

    pub async fn evict_runtime_version_best_effort(&self, version: &RuntimeVersion) {
        self.evict_row_best_effort(
            MetadataEntity::RuntimeVersion,
            version.id,
            &runtime_version_cache_ref(version),
            runtime_version_indexes(version),
        )
        .await;
    }

    async fn find_by_id<T, Load, LoadFut, Put, PutFut>(
        &self,
        entity: MetadataEntity,
        id: Id,
        load: Load,
        put: Put,
    ) -> Result<Option<T>>
    where
        T: serde::Serialize + serde::de::DeserializeOwned + Clone,
        Load: FnOnce() -> LoadFut,
        LoadFut: std::future::Future<Output = Result<Option<T>>>,
        Put: FnOnce(T) -> PutFut,
        PutFut: std::future::Future<Output = ()>,
    {
        if self.cache.entity_enabled(entity) {
            if let Some(key) = self.cache.key_for_id(entity, id) {
                match self.cache.get_json::<T>(&key).await {
                    Ok(Some(row)) => return Ok(Some(row)),
                    Ok(None) => debug!(entity = %entity, id, "Metadata cache miss by id"),
                    Err(e) => self.cache.log_best_effort_error("metadata.find_by_id", &e),
                }
            }
        }

        let row = load().await?;
        if let Some(ref row) = row {
            put(row.clone()).await;
        }
        Ok(row)
    }

    async fn try_find_cached_index<T>(
        &self,
        entity: MetadataEntity,
        index: &str,
        operation: &str,
    ) -> Result<Option<Vec<T>>>
    where
        T: serde::de::DeserializeOwned,
    {
        if !self.cache.entity_enabled(entity) {
            return Ok(None);
        }
        let Some(index_key) = self.cache.index_key(entity, index) else {
            return Ok(None);
        };
        let mut refs = match self.cache.get_index_members(&index_key).await {
            Ok(refs) if !refs.is_empty() => refs,
            Ok(_) => return Ok(None),
            Err(e) => {
                self.cache
                    .log_best_effort_error(&format!("{operation}.index_read"), &e);
                return Ok(None);
            }
        };
        refs.sort();

        let keys: Vec<String> = refs
            .iter()
            .filter_map(|ref_str| self.cache.key_for_ref(entity, ref_str))
            .collect();
        if keys.len() != refs.len() {
            return Ok(None);
        }

        match self.cache.get_json_many::<T, _, _>(&keys).await {
            Ok(rows) if rows.len() == refs.len() && rows.iter().all(Option::is_some) => {
                Ok(Some(rows.into_iter().flatten().collect()))
            }
            Ok(_) => Ok(None),
            Err(e) => {
                self.cache
                    .log_best_effort_error(&format!("{operation}.multi_get"), &e);
                Ok(None)
            }
        }
    }

    async fn try_find_cached_index_with_empty_marker<T>(
        &self,
        entity: MetadataEntity,
        index: &str,
        operation: &str,
    ) -> Result<Option<Vec<T>>>
    where
        T: serde::de::DeserializeOwned,
    {
        if !self.cache.entity_enabled(entity) {
            return Ok(None);
        }
        if let Some(empty_key) = self.cache.empty_index_key(entity, index) {
            match self.cache.is_index_marked_empty(&empty_key).await {
                Ok(true) => return Ok(Some(Vec::new())),
                Ok(false) => {}
                Err(e) => self
                    .cache
                    .log_best_effort_error(&format!("{operation}.empty_index_read"), &e),
            }
        }
        self.try_find_cached_index(entity, index, operation).await
    }

    async fn find_by_ref<T, Load, LoadFut, Put, PutFut>(
        &self,
        entity: MetadataEntity,
        ref_str: &str,
        load: Load,
        put: Put,
    ) -> Result<Option<T>>
    where
        T: serde::Serialize + serde::de::DeserializeOwned + Clone,
        Load: FnOnce() -> LoadFut,
        LoadFut: std::future::Future<Output = Result<Option<T>>>,
        Put: FnOnce(T) -> PutFut,
        PutFut: std::future::Future<Output = ()>,
    {
        if self.cache.entity_enabled(entity) {
            if let Some(key) = self.cache.key_for_ref(entity, ref_str) {
                match self.cache.get_json::<T>(&key).await {
                    Ok(Some(row)) => return Ok(Some(row)),
                    Ok(None) => {
                        debug!(entity = %entity, ref = ref_str, "Metadata cache miss by ref")
                    }
                    Err(e) => self.cache.log_best_effort_error("metadata.find_by_ref", &e),
                }
            }
        }

        let row = load().await?;
        if let Some(ref row) = row {
            put(row.clone()).await;
        }
        Ok(row)
    }

    async fn put_index_members_or_empty_best_effort<'b, I>(
        &self,
        entity: MetadataEntity,
        index: &str,
        refs: I,
    ) where
        I: IntoIterator<Item = &'b str>,
    {
        if !self.cache.entity_enabled(entity) {
            return;
        }
        let refs = refs
            .into_iter()
            .filter(|ref_str| !ref_str.is_empty())
            .collect::<Vec<_>>();
        if refs.is_empty() {
            self.mark_index_empty_best_effort(entity, index).await;
            return;
        }
        let Some(index_key) = self.cache.index_key(entity, index) else {
            return;
        };
        if let Err(e) = self.cache.add_index_members(&index_key, refs).await {
            self.cache
                .log_best_effort_error("metadata.put_index_members", &e);
        }
    }

    async fn mark_index_empty_best_effort(&self, entity: MetadataEntity, index: &str) {
        if !self.cache.entity_enabled(entity) {
            return;
        }
        let Some(empty_key) = self.cache.empty_index_key(entity, index) else {
            return;
        };
        if let Err(e) = self.cache.mark_index_empty(&empty_key).await {
            self.cache
                .log_best_effort_error("metadata.mark_empty_index", &e);
        }
    }

    async fn put_row_best_effort<T>(
        &self,
        entity: MetadataEntity,
        id: Id,
        ref_str: &str,
        row: &T,
        index_specs: Vec<String>,
    ) where
        T: serde::Serialize + ?Sized,
    {
        if !self.cache.entity_enabled(entity) {
            return;
        }
        let mut keys = Vec::new();
        if let Some(key) = self.cache.key_for_id(entity, id) {
            keys.push(key);
        }
        if let Some(key) = self.cache.key_for_ref(entity, ref_str) {
            keys.push(key);
        }

        if let Err(e) = self.cache.set_json_for_keys(keys, row).await {
            self.cache.log_best_effort_error("metadata.put_row", &e);
        }

        let mut indexes = index_specs;
        indexes.push("all_refs".to_string());
        let index_keys = indexes
            .into_iter()
            .filter_map(|index| self.cache.index_key(entity, &index))
            .collect::<Vec<_>>();
        if let Err(e) = self.cache.add_member_to_indexes(index_keys, ref_str).await {
            self.cache.log_best_effort_error("metadata.add_index", &e);
        }
    }

    async fn evict_row_best_effort(
        &self,
        entity: MetadataEntity,
        id: Id,
        ref_str: &str,
        index_specs: Vec<String>,
    ) {
        if !self.cache.entity_enabled(entity) {
            return;
        }

        let keys = [
            self.cache.key_for_id(entity, id),
            self.cache.key_for_ref(entity, ref_str),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if let Err(e) = self.cache.delete_keys(keys).await {
            self.cache.log_best_effort_error("metadata.evict_row", &e);
        }

        let mut indexes = index_specs;
        indexes.push("all_refs".to_string());
        let index_keys = indexes
            .into_iter()
            .filter_map(|index| self.cache.index_key(entity, &index))
            .collect::<Vec<_>>();
        if let Err(e) = self
            .cache
            .remove_member_from_indexes(index_keys, ref_str)
            .await
        {
            self.cache
                .log_best_effort_error("metadata.remove_index", &e);
        }
    }

    async fn try_find_effective_policy_cached(
        &self,
        action_id: Id,
        pack_id: Option<Id>,
    ) -> Result<Option<Option<Policy>>> {
        if !self.cache.entity_enabled(MetadataEntity::Policy) {
            return Ok(None);
        }

        let mut scoped_indexes = vec![format!("action:{action_id}:refs")];
        if let Some(pack_id) = pack_id {
            scoped_indexes.push(format!("pack:{pack_id}:refs"));
        }
        scoped_indexes.push("global_refs".to_string());

        let empty_keys = scoped_indexes
            .iter()
            .filter_map(|index| self.cache.empty_index_key(MetadataEntity::Policy, index))
            .collect::<Vec<_>>();
        let empty_markers = match self.cache.get_json_many::<bool, _, _>(&empty_keys).await {
            Ok(values) if values.len() == scoped_indexes.len() => values,
            Ok(_) => return Ok(None),
            Err(e) => {
                self.cache
                    .log_best_effort_error("policy.find_effective.empty_index_multi_get", &e);
                return Ok(None);
            }
        };

        let mut unresolved_indexes = Vec::new();
        let mut unresolved_positions = Vec::new();
        let mut refs_by_position: Vec<Vec<String>> = vec![Vec::new(); scoped_indexes.len()];
        for (position, marker) in empty_markers.into_iter().enumerate() {
            if marker.unwrap_or(false) {
                continue;
            }
            let Some(index_key) = self
                .cache
                .index_key(MetadataEntity::Policy, &scoped_indexes[position])
            else {
                return Ok(None);
            };
            unresolved_positions.push(position);
            unresolved_indexes.push(index_key);
        }

        if !unresolved_indexes.is_empty() {
            let member_sets = match self.cache.get_index_members_many(&unresolved_indexes).await {
                Ok(member_sets) if member_sets.len() == unresolved_indexes.len() => member_sets,
                Ok(_) => return Ok(None),
                Err(e) => {
                    self.cache
                        .log_best_effort_error("policy.find_effective.index_pipeline", &e);
                    return Ok(None);
                }
            };

            for (position, mut refs) in unresolved_positions.into_iter().zip(member_sets) {
                if refs.is_empty() {
                    return Ok(None);
                }
                refs.sort();
                refs.dedup();
                refs_by_position[position] = refs;
            }
        }

        let mut all_refs = refs_by_position
            .iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        all_refs.sort();
        all_refs.dedup();

        let mut policies_by_ref: HashMap<String, Policy> = HashMap::new();
        if !all_refs.is_empty() {
            let keys = all_refs
                .iter()
                .filter_map(|ref_str| self.cache.key_for_ref(MetadataEntity::Policy, ref_str))
                .collect::<Vec<_>>();
            if keys.len() != all_refs.len() {
                return Ok(None);
            }
            let rows = match self.cache.get_json_many::<Policy, _, _>(&keys).await {
                Ok(rows) if rows.len() == all_refs.len() && rows.iter().all(Option::is_some) => {
                    rows
                }
                Ok(_) => return Ok(None),
                Err(e) => {
                    self.cache
                        .log_best_effort_error("policy.find_effective.multi_get", &e);
                    return Ok(None);
                }
            };
            for policy in rows.into_iter().flatten() {
                policies_by_ref.insert(policy.r#ref.clone(), policy);
            }
        }

        let action_policy = refs_by_position
            .first()
            .into_iter()
            .flatten()
            .filter_map(|ref_str| policies_by_ref.get(ref_str).cloned())
            .max_by_key(|policy| policy.created);
        if action_policy.is_some() {
            return Ok(Some(action_policy));
        }

        let mut global_position = 1;
        if pack_id.is_some() {
            let pack_policy = refs_by_position
                .get(1)
                .into_iter()
                .flatten()
                .filter_map(|ref_str| policies_by_ref.get(ref_str).cloned())
                .filter(|policy| policy.action.is_none())
                .max_by_key(|policy| policy.created);
            if pack_policy.is_some() {
                return Ok(Some(pack_policy));
            }
            global_position = 2;
        }

        let global_policy = refs_by_position
            .get(global_position)
            .into_iter()
            .flatten()
            .filter_map(|ref_str| policies_by_ref.get(ref_str).cloned())
            .filter(|policy| policy.pack.is_none() && policy.action.is_none())
            .max_by_key(|policy| policy.created);
        Ok(Some(global_policy))
    }
}

fn pack_indexes(pack_id: Id) -> Vec<String> {
    vec![format!("pack:{pack_id}:refs")]
}

fn optional_pack_indexes(pack_id: Option<Id>) -> Vec<String> {
    pack_id.map(pack_indexes).unwrap_or_default()
}

fn action_indexes(action: &Action) -> Vec<String> {
    let mut indexes = pack_indexes(action.pack);
    if let Some(workflow_def_id) = action.workflow_def {
        indexes.push(format!("workflow_def:{workflow_def_id}:refs"));
    }
    indexes
}

fn rule_indexes(rule: &Rule) -> Vec<String> {
    let mut indexes = pack_indexes(rule.pack);
    if rule.enabled {
        indexes.push("enabled_refs".to_string());
    }
    if let Some(action_id) = rule.action {
        indexes.push(format!("action:{action_id}:refs"));
    }
    if let Some(trigger_id) = rule.trigger {
        indexes.push(format!("trigger:{trigger_id}:refs"));
    }
    indexes
}

fn trigger_indexes(trigger: &Trigger) -> Vec<String> {
    let mut indexes = optional_pack_indexes(trigger.pack);
    if trigger.enabled {
        indexes.push("enabled_refs".to_string());
    }
    if let Some(sensor_id) = trigger.sensor {
        indexes.push(format!("sensor:{sensor_id}:refs"));
    }
    if let Some(webhook_key) = trigger
        .webhook_key
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        indexes.push(format!("webhook_key:{webhook_key}:refs"));
    }
    indexes
}

fn sensor_indexes(sensor: &Sensor) -> Vec<String> {
    let mut indexes = optional_pack_indexes(sensor.pack);
    if sensor.enabled {
        indexes.push("enabled_refs".to_string());
    }
    indexes
}

fn policy_indexes(policy: &Policy) -> Vec<String> {
    let mut indexes = optional_pack_indexes(policy.pack);
    if let Some(action_id) = policy.action {
        indexes.push(format!("action:{action_id}:refs"));
    }
    if policy.pack.is_none() && policy.action.is_none() {
        indexes.push("global_refs".to_string());
    }
    indexes
}

fn work_queue_indexes(queue: &WorkQueue) -> Vec<String> {
    let mut indexes = optional_pack_indexes(queue.pack);
    if queue.enabled {
        indexes.push("enabled_refs".to_string());
    }
    if queue.accepting_new_items {
        indexes.push("accepting_new_items_refs".to_string());
    }
    indexes
}

fn runtime_indexes(runtime: &Runtime) -> Vec<String> {
    let mut indexes = optional_pack_indexes(runtime.pack);
    indexes.push(format!(
        "name:{}:refs",
        normalized_index_value(&runtime.name)
    ));
    for alias in &runtime.aliases {
        indexes.push(format!("alias:{}:refs", normalized_index_value(alias)));
    }
    indexes
}

fn runtime_version_indexes(version: &RuntimeVersion) -> Vec<String> {
    let mut indexes = vec![
        format!("runtime:{}:refs", version.runtime),
        format!("runtime_ref:{}:refs", version.runtime_ref),
    ];
    if version.available {
        indexes.push("available_refs".to_string());
    }
    if version.is_default {
        indexes.push(format!("default_runtime:{}:refs", version.runtime));
    }
    indexes
}

fn runtime_version_cache_ref(version: &RuntimeVersion) -> String {
    format!("{}:{}", version.runtime_ref, version.version)
}

fn normalized_index_value(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn sort_runtime_versions(mut versions: Vec<RuntimeVersion>) -> Vec<RuntimeVersion> {
    versions.sort_by(|a, b| {
        b.version_major
            .cmp(&a.version_major)
            .then_with(|| b.version_minor.cmp(&a.version_minor))
            .then_with(|| b.version_patch.cmp(&a.version_patch))
            .then_with(|| a.runtime_ref.cmp(&b.runtime_ref))
            .then_with(|| a.version.cmp(&b.version))
    });
    versions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::enums::PolicyMethod;
    use chrono::Utc;

    #[test]
    fn policy_indexes_include_action_scope() {
        let policy = Policy {
            id: 1,
            r#ref: "core.echo_limit".to_string(),
            pack: Some(2),
            pack_ref: Some("core".to_string()),
            action: Some(3),
            action_ref: Some("core.echo".to_string()),
            parameters: Vec::new(),
            method: PolicyMethod::Enqueue,
            threshold: 1,
            name: "Limit".to_string(),
            description: None,
            tags: Vec::new(),
            created: Utc::now(),
            updated: Utc::now(),
        };

        let indexes = policy_indexes(&policy);
        assert!(indexes.contains(&"pack:2:refs".to_string()));
        assert!(indexes.contains(&"action:3:refs".to_string()));
    }

    #[test]
    fn policy_indexes_include_global_scope() {
        let policy = Policy {
            id: 1,
            r#ref: "global.limit".to_string(),
            pack: None,
            pack_ref: None,
            action: None,
            action_ref: None,
            parameters: Vec::new(),
            method: PolicyMethod::Enqueue,
            threshold: 1,
            name: "Global Limit".to_string(),
            description: None,
            tags: Vec::new(),
            created: Utc::now(),
            updated: Utc::now(),
        };

        assert_eq!(policy_indexes(&policy), vec!["global_refs".to_string()]);
    }

    #[test]
    fn optional_pack_indexes_ignore_none() {
        assert!(optional_pack_indexes(None).is_empty());
        assert_eq!(optional_pack_indexes(Some(42)), vec!["pack:42:refs"]);
    }

    #[test]
    fn trigger_indexes_include_sensor_scope() {
        let trigger = Trigger {
            id: 1,
            r#ref: "core.tick".to_string(),
            pack: Some(2),
            pack_ref: Some("core".to_string()),
            label: "Tick".to_string(),
            description: None,
            enabled: true,
            param_schema: None,
            out_schema: None,
            webhook_enabled: true,
            webhook_key: Some("hook-123".to_string()),
            webhook_config: None,
            sensor: Some(7),
            sensor_ref: Some("core.timer".to_string()),
            is_adhoc: false,
            reference_visibility: crate::models::ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: Vec::new(),
            created: Utc::now(),
            updated: Utc::now(),
        };

        let indexes = trigger_indexes(&trigger);
        assert!(indexes.contains(&"pack:2:refs".to_string()));
        assert!(indexes.contains(&"enabled_refs".to_string()));
        assert!(indexes.contains(&"sensor:7:refs".to_string()));
        assert!(indexes.contains(&"webhook_key:hook-123:refs".to_string()));
    }

    #[test]
    fn runtime_indexes_include_name_alias_and_pack_scope() {
        let runtime = Runtime {
            id: 1,
            r#ref: "core.python".to_string(),
            pack: Some(2),
            pack_ref: Some("core".to_string()),
            description: None,
            name: "Python".to_string(),
            aliases: vec!["python3".to_string(), "Py".to_string()],
            distributions: serde_json::json!({}),
            installation: None,
            installers: serde_json::json!({}),
            execution_config: serde_json::json!({}),
            auto_detected: false,
            detection_config: serde_json::json!({}),
            created: Utc::now(),
            updated: Utc::now(),
        };

        let indexes = runtime_indexes(&runtime);
        assert!(indexes.contains(&"pack:2:refs".to_string()));
        assert!(indexes.contains(&"name:python:refs".to_string()));
        assert!(indexes.contains(&"alias:python3:refs".to_string()));
        assert!(indexes.contains(&"alias:py:refs".to_string()));
    }

    #[test]
    fn runtime_version_indexes_include_runtime_and_availability_scope() {
        let version = RuntimeVersion {
            id: 1,
            runtime: 9,
            runtime_ref: "core.python".to_string(),
            version: "3.12.1".to_string(),
            version_major: Some(3),
            version_minor: Some(12),
            version_patch: Some(1),
            execution_config: serde_json::json!({}),
            distributions: serde_json::json!({}),
            is_default: true,
            available: true,
            verified_at: None,
            meta: serde_json::json!({}),
            created: Utc::now(),
            updated: Utc::now(),
        };

        let indexes = runtime_version_indexes(&version);
        assert_eq!(runtime_version_cache_ref(&version), "core.python:3.12.1");
        assert!(indexes.contains(&"runtime:9:refs".to_string()));
        assert!(indexes.contains(&"runtime_ref:core.python:refs".to_string()));
        assert!(indexes.contains(&"available_refs".to_string()));
        assert!(indexes.contains(&"default_runtime:9:refs".to_string()));
    }
}
