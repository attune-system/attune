//! Policy Enforcer - Enforces execution policies
//!
//! This module is responsible for:
//! - Rate limiting: Limit executions per time window
//! - Concurrency control: Maximum concurrent executions
//! - Quota management: Resource limits per tenant/pack
//! - Policy evaluation before execution creation
//! - Policy enforcement during scheduling

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tracing::{debug, info, warn};

use attune_common::{
    models::{
        enums::{ExecutionStatus, PolicyMethod},
        Id, Policy,
    },
    repositories::action::PolicyRepository,
};

use crate::queue_manager::{
    ExecutionQueueManager, QueuedRemovalOutcome, SlotEnqueueOutcome, SlotReleaseOutcome,
};

/// Policy violation type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::enum_variant_names)]
pub enum PolicyViolation {
    /// Rate limit exceeded
    RateLimitExceeded {
        limit: u32,
        window_seconds: u32,
        current_count: u32,
    },
    /// Concurrency limit exceeded
    ConcurrencyLimitExceeded { limit: u32, current_count: u32 },
    /// Resource quota exceeded
    QuotaExceeded {
        quota_type: String,
        limit: u64,
        current_usage: u64,
    },
}

impl std::fmt::Display for PolicyViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyViolation::RateLimitExceeded {
                limit,
                window_seconds,
                current_count,
            } => {
                write!(
                    f,
                    "Rate limit exceeded: {} executions in {} seconds (limit: {})",
                    current_count, window_seconds, limit
                )
            }
            PolicyViolation::ConcurrencyLimitExceeded {
                limit,
                current_count,
            } => {
                write!(
                    f,
                    "Concurrency limit exceeded: {} running executions (limit: {})",
                    current_count, limit
                )
            }
            PolicyViolation::QuotaExceeded {
                quota_type,
                limit,
                current_usage,
            } => {
                write!(
                    f,
                    "{} quota exceeded: {} (limit: {})",
                    quota_type, current_usage, limit
                )
            }
        }
    }
}

/// Execution policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPolicy {
    /// Rate limit: maximum executions per time window
    pub rate_limit: Option<RateLimit>,
    /// Concurrency limit: maximum concurrent executions
    pub concurrency_limit: Option<u32>,
    /// How a concurrency violation should be handled.
    pub concurrency_method: PolicyMethod,
    /// Parameter paths used to scope concurrency grouping.
    pub concurrency_parameters: Vec<String>,
    /// Resource quotas
    pub quotas: Option<HashMap<String, u64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingPolicyOutcome {
    Ready,
    Queued,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            rate_limit: None,
            concurrency_limit: None,
            concurrency_method: PolicyMethod::Enqueue,
            concurrency_parameters: Vec::new(),
            quotas: None,
        }
    }
}

/// Rate limit configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    /// Maximum number of executions
    pub max_executions: u32,
    /// Time window in seconds
    pub window_seconds: u32,
}

#[derive(Debug, Clone)]
struct ResolvedConcurrencyPolicy {
    limit: u32,
    method: PolicyMethod,
    parameters: Vec<String>,
}

impl From<Policy> for ExecutionPolicy {
    fn from(policy: Policy) -> Self {
        let rate_limit = match (
            policy.rate_limit_max_executions,
            policy.rate_limit_window_seconds,
        ) {
            (Some(max_executions), Some(window_seconds)) => Some(RateLimit {
                max_executions: max_executions as u32,
                window_seconds: window_seconds as u32,
            }),
            _ => None,
        };
        let quotas = parse_policy_quotas(&policy.quotas);

        Self {
            rate_limit,
            concurrency_limit: policy.threshold.map(|threshold| threshold as u32),
            concurrency_method: policy.method.unwrap_or(PolicyMethod::Enqueue),
            concurrency_parameters: policy.parameters,
            quotas,
        }
    }
}

/// Policy enforcement scope
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Used in tests
pub enum PolicyScope {
    /// Global policy (all executions)
    Global,
    /// Per-pack policy
    Pack(Id),
    /// Per-action policy
    Action(Id),
    /// Per-identity policy (tenant)
    Identity(Id),
}

/// Policy enforcer that validates execution policies
pub struct PolicyEnforcer {
    pool: PgPool,
    /// Global execution policy
    global_policy: ExecutionPolicy,
    /// Per-pack policies
    pack_policies: HashMap<Id, ExecutionPolicy>,
    /// Per-action policies
    action_policies: HashMap<Id, ExecutionPolicy>,
    /// Queue manager for FIFO execution ordering
    queue_manager: Option<Arc<ExecutionQueueManager>>,
}

impl PolicyEnforcer {
    /// Create a new policy enforcer
    #[allow(dead_code)]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            global_policy: ExecutionPolicy::default(),
            pack_policies: HashMap::new(),
            action_policies: HashMap::new(),
            queue_manager: None,
        }
    }

    /// Create a new policy enforcer with queue manager
    pub fn with_queue_manager(pool: PgPool, queue_manager: Arc<ExecutionQueueManager>) -> Self {
        Self {
            pool,
            global_policy: ExecutionPolicy::default(),
            pack_policies: HashMap::new(),
            action_policies: HashMap::new(),
            queue_manager: Some(queue_manager),
        }
    }

    /// Create with global policy
    #[allow(dead_code)]
    pub fn with_global_policy(pool: PgPool, policy: ExecutionPolicy) -> Self {
        Self {
            pool,
            global_policy: policy,
            pack_policies: HashMap::new(),
            action_policies: HashMap::new(),
            queue_manager: None,
        }
    }

    /// Set the queue manager
    #[allow(dead_code)]
    pub fn set_queue_manager(&mut self, queue_manager: Arc<ExecutionQueueManager>) {
        self.queue_manager = Some(queue_manager);
    }

    /// Set global execution policy
    #[allow(dead_code)]
    pub fn set_global_policy(&mut self, policy: ExecutionPolicy) {
        self.global_policy = policy;
    }

    /// Set policy for a specific pack
    #[allow(dead_code)]
    pub fn set_pack_policy(&mut self, pack_id: Id, policy: ExecutionPolicy) {
        self.pack_policies.insert(pack_id, policy);
    }

    /// Set policy for a specific action
    #[allow(dead_code)]
    pub fn set_action_policy(&mut self, action_id: Id, policy: ExecutionPolicy) {
        self.action_policies.insert(action_id, policy);
    }

    /// Best-effort release for a slot acquired during scheduling when the
    /// execution never reaches the worker/completion path.
    pub async fn release_execution_slot(
        &self,
        execution_id: Id,
    ) -> Result<Option<SlotReleaseOutcome>> {
        match &self.queue_manager {
            Some(queue_manager) => queue_manager.release_active_slot(execution_id).await,
            None => Ok(None),
        }
    }

    pub async fn restore_execution_slot(
        &self,
        execution_id: Id,
        outcome: &SlotReleaseOutcome,
    ) -> Result<()> {
        match &self.queue_manager {
            Some(queue_manager) => {
                queue_manager
                    .restore_active_slot(execution_id, outcome)
                    .await
            }
            None => Ok(()),
        }
    }

    pub async fn remove_queued_execution(
        &self,
        execution_id: Id,
    ) -> Result<Option<QueuedRemovalOutcome>> {
        match &self.queue_manager {
            Some(queue_manager) => queue_manager.remove_queued_execution(execution_id).await,
            None => Ok(None),
        }
    }

    pub async fn restore_queued_execution(&self, outcome: &QueuedRemovalOutcome) -> Result<()> {
        match &self.queue_manager {
            Some(queue_manager) => queue_manager.restore_queued_execution(outcome).await,
            None => Ok(()),
        }
    }

    pub async fn enforce_for_scheduling(
        &self,
        action_id: Id,
        pack_id: Option<Id>,
        execution_id: Id,
        config: Option<&JsonValue>,
    ) -> Result<SchedulingPolicyOutcome> {
        if let Some(violation) = self
            .check_policies_except_concurrency(action_id, pack_id)
            .await?
        {
            warn!("Policy violation for action {}: {}", action_id, violation);
            return Err(anyhow::anyhow!("Policy violation: {}", violation));
        }

        if let Some(concurrency) = self.resolve_concurrency_policy(action_id, pack_id).await? {
            let group_key = self.build_parameter_group_key(&concurrency.parameters, config);

            if let Some(queue_manager) = &self.queue_manager {
                match concurrency.method {
                    PolicyMethod::Enqueue => {
                        return match queue_manager
                            .enqueue(action_id, execution_id, concurrency.limit, group_key)
                            .await?
                        {
                            SlotEnqueueOutcome::Acquired => Ok(SchedulingPolicyOutcome::Ready),
                            SlotEnqueueOutcome::Enqueued => Ok(SchedulingPolicyOutcome::Queued),
                        };
                    }
                    PolicyMethod::Cancel => {
                        let outcome = queue_manager
                            .try_acquire(
                                action_id,
                                execution_id,
                                concurrency.limit,
                                group_key.clone(),
                            )
                            .await?;

                        if !outcome.acquired {
                            let violation = PolicyViolation::ConcurrencyLimitExceeded {
                                limit: concurrency.limit,
                                current_count: outcome.current_count,
                            };
                            warn!("Policy violation for action {}: {}", action_id, violation);
                            return Err(anyhow::anyhow!("Policy violation: {}", violation));
                        }
                    }
                }
            } else {
                let scope = PolicyScope::Action(action_id);
                if let Some(violation) = self
                    .check_concurrency_limit(concurrency.limit, &scope)
                    .await?
                {
                    return Err(anyhow::anyhow!("Policy violation: {}", violation));
                }
            }
        }

        Ok(SchedulingPolicyOutcome::Ready)
    }

    async fn resolve_policy(&self, action_id: Id, pack_id: Option<Id>) -> Result<ExecutionPolicy> {
        if let Some(policy) = self.action_policies.get(&action_id) {
            return Ok(policy.clone());
        }

        if let Some(policy) = PolicyRepository::find_latest_by_action(&self.pool, action_id).await?
        {
            return Ok(policy.into());
        }

        if let Some(pack_id) = pack_id {
            if let Some(policy) = self.pack_policies.get(&pack_id) {
                return Ok(policy.clone());
            }

            if let Some(policy) = PolicyRepository::find_latest_by_pack(&self.pool, pack_id).await?
            {
                return Ok(policy.into());
            }
        }

        if let Some(policy) = PolicyRepository::find_latest_global(&self.pool).await? {
            return Ok(policy.into());
        }

        Ok(self.global_policy.clone())
    }

    async fn resolve_concurrency_policy(
        &self,
        action_id: Id,
        pack_id: Option<Id>,
    ) -> Result<Option<ResolvedConcurrencyPolicy>> {
        let policy = self.resolve_policy(action_id, pack_id).await?;

        Ok(policy
            .concurrency_limit
            .map(|limit| ResolvedConcurrencyPolicy {
                limit,
                method: policy.concurrency_method,
                parameters: policy.concurrency_parameters,
            }))
    }

    fn build_parameter_group_key(
        &self,
        parameter_paths: &[String],
        config: Option<&JsonValue>,
    ) -> Option<String> {
        if parameter_paths.is_empty() {
            return None;
        }

        let values: BTreeMap<String, JsonValue> = parameter_paths
            .iter()
            .map(|path| (path.clone(), extract_parameter_value(config, path)))
            .collect();

        serde_json::to_string(&values).ok()
    }

    /// Get the concurrency limit for a specific action
    ///
    /// Returns the most specific concurrency limit found:
    /// 1. Action-specific policy
    /// 2. Pack policy
    /// 3. Global policy
    /// 4. None (unlimited)
    #[allow(dead_code)]
    pub fn get_concurrency_limit(&self, action_id: Id, pack_id: Option<Id>) -> Option<u32> {
        // Check action-specific policy first
        if let Some(policy) = self.action_policies.get(&action_id) {
            if let Some(limit) = policy.concurrency_limit {
                return Some(limit);
            }
        }

        // Check pack policy
        if let Some(pack_id) = pack_id {
            if let Some(policy) = self.pack_policies.get(&pack_id) {
                if let Some(limit) = policy.concurrency_limit {
                    return Some(limit);
                }
            }
        }

        // Check global policy
        self.global_policy.concurrency_limit
    }

    /// Check policies except concurrency (which is handled by queue)
    async fn check_policies_except_concurrency(
        &self,
        action_id: Id,
        pack_id: Option<Id>,
    ) -> Result<Option<PolicyViolation>> {
        // Check action-specific policy first
        if let Some(policy) = self.action_policies.get(&action_id) {
            if let Some(violation) = self
                .evaluate_policy_except_concurrency(policy, PolicyScope::Action(action_id))
                .await?
            {
                return Ok(Some(violation));
            }
        }

        // Check pack policy
        if let Some(pack_id) = pack_id {
            if let Some(policy) = self.pack_policies.get(&pack_id) {
                if let Some(violation) = self
                    .evaluate_policy_except_concurrency(policy, PolicyScope::Pack(pack_id))
                    .await?
                {
                    return Ok(Some(violation));
                }
            }
        }

        // Check global policy
        if let Some(violation) = self
            .evaluate_policy_except_concurrency(&self.global_policy, PolicyScope::Global)
            .await?
        {
            return Ok(Some(violation));
        }

        Ok(None)
    }

    /// Evaluate a policy against current state (except concurrency)
    async fn evaluate_policy_except_concurrency(
        &self,
        policy: &ExecutionPolicy,
        scope: PolicyScope,
    ) -> Result<Option<PolicyViolation>> {
        // Check rate limit
        if let Some(rate_limit) = &policy.rate_limit {
            if let Some(violation) = self.check_rate_limit(rate_limit, &scope).await? {
                return Ok(Some(violation));
            }
        }

        // Skip concurrency check - handled by queue

        // Check quotas
        if let Some(quotas) = &policy.quotas {
            for (quota_type, limit) in quotas {
                if let Some(violation) = self.check_quota(quota_type, *limit, &scope).await? {
                    return Ok(Some(violation));
                }
            }
        }

        Ok(None)
    }

    /// Check if execution is allowed under policies
    #[allow(dead_code)]
    pub async fn check_policies(
        &self,
        action_id: Id,
        pack_id: Option<Id>,
    ) -> Result<Option<PolicyViolation>> {
        // Check action-specific policy first
        if let Some(policy) = self.action_policies.get(&action_id) {
            if let Some(violation) = self
                .evaluate_policy(policy, PolicyScope::Action(action_id))
                .await?
            {
                return Ok(Some(violation));
            }
        }

        // Check pack policy
        if let Some(pack_id) = pack_id {
            if let Some(policy) = self.pack_policies.get(&pack_id) {
                if let Some(violation) = self
                    .evaluate_policy(policy, PolicyScope::Pack(pack_id))
                    .await?
                {
                    return Ok(Some(violation));
                }
            }
        }

        // Check global policy
        if let Some(violation) = self
            .evaluate_policy(&self.global_policy, PolicyScope::Global)
            .await?
        {
            return Ok(Some(violation));
        }

        Ok(None)
    }

    /// Evaluate a policy against current state
    #[allow(dead_code)]
    async fn evaluate_policy(
        &self,
        policy: &ExecutionPolicy,
        scope: PolicyScope,
    ) -> Result<Option<PolicyViolation>> {
        // Check rate limit
        if let Some(rate_limit) = &policy.rate_limit {
            if let Some(violation) = self.check_rate_limit(rate_limit, &scope).await? {
                return Ok(Some(violation));
            }
        }

        // Check concurrency limit
        if let Some(concurrency_limit) = policy.concurrency_limit {
            if let Some(violation) = self
                .check_concurrency_limit(concurrency_limit, &scope)
                .await?
            {
                return Ok(Some(violation));
            }
        }

        // Check quotas
        if let Some(quotas) = &policy.quotas {
            for (quota_type, limit) in quotas {
                if let Some(violation) = self.check_quota(quota_type, *limit, &scope).await? {
                    return Ok(Some(violation));
                }
            }
        }

        Ok(None)
    }

    /// Check rate limit for a scope
    async fn check_rate_limit(
        &self,
        rate_limit: &RateLimit,
        scope: &PolicyScope,
    ) -> Result<Option<PolicyViolation>> {
        let window_start = Utc::now() - Duration::seconds(rate_limit.window_seconds as i64);

        let count = self.count_executions_since(scope, window_start).await?;

        if count >= rate_limit.max_executions {
            info!(
                "Rate limit exceeded for {:?}: {} executions in {} seconds (limit: {})",
                scope, count, rate_limit.window_seconds, rate_limit.max_executions
            );

            return Ok(Some(PolicyViolation::RateLimitExceeded {
                limit: rate_limit.max_executions,
                window_seconds: rate_limit.window_seconds,
                current_count: count,
            }));
        }

        debug!(
            "Rate limit check passed for {:?}: {} / {} executions in {} seconds",
            scope, count, rate_limit.max_executions, rate_limit.window_seconds
        );

        Ok(None)
    }

    /// Check concurrency limit for a scope
    async fn check_concurrency_limit(
        &self,
        limit: u32,
        scope: &PolicyScope,
    ) -> Result<Option<PolicyViolation>> {
        let count = self.count_running_executions(scope).await?;

        if count >= limit {
            info!(
                "Concurrency limit exceeded for {:?}: {} running executions (limit: {})",
                scope, count, limit
            );

            return Ok(Some(PolicyViolation::ConcurrencyLimitExceeded {
                limit,
                current_count: count,
            }));
        }

        debug!(
            "Concurrency limit check passed for {:?}: {} / {} running executions",
            scope, count, limit
        );

        Ok(None)
    }

    /// Check resource quota for a scope
    async fn check_quota(
        &self,
        quota_type: &str,
        limit: u64,
        scope: &PolicyScope,
    ) -> Result<Option<PolicyViolation>> {
        let current_usage = match quota_type {
            "running_executions" => self.count_running_executions(scope).await? as u64,
            "executions_total" => self.count_executions(scope).await? as u64,
            other => {
                warn!(
                    "Unsupported policy quota type '{}'; treating as not violated",
                    other
                );
                return Ok(None);
            }
        };

        if current_usage >= limit {
            return Ok(Some(PolicyViolation::QuotaExceeded {
                quota_type: quota_type.to_string(),
                limit,
                current_usage,
            }));
        }

        debug!(
            "Quota check passed for {:?}: {}={} / {}",
            scope, quota_type, current_usage, limit
        );

        Ok(None)
    }

    /// Count executions created since a specific time
    async fn count_executions_since(
        &self,
        scope: &PolicyScope,
        since: DateTime<Utc>,
    ) -> Result<u32> {
        let count: i64 = match scope {
            PolicyScope::Global => {
                sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE created >= $1")
                    .bind(since)
                    .fetch_one(&self.pool)
                    .await?
            }
            PolicyScope::Pack(pack_id) => {
                sqlx::query_scalar(
                    r#"
                    SELECT COUNT(*)
                    FROM execution e
                    JOIN action a ON e.action = a.id
                    WHERE a.pack = $1 AND e.created >= $2
                    "#,
                )
                .bind(pack_id)
                .bind(since)
                .fetch_one(&self.pool)
                .await?
            }
            PolicyScope::Action(action_id) => {
                sqlx::query_scalar(
                    "SELECT COUNT(*) FROM execution WHERE action = $1 AND created >= $2",
                )
                .bind(action_id)
                .bind(since)
                .fetch_one(&self.pool)
                .await?
            }
            PolicyScope::Identity(_identity_id) => {
                // TODO: Track executions by identity/tenant
                // For now, treat as global
                sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE created >= $1")
                    .bind(since)
                    .fetch_one(&self.pool)
                    .await?
            }
        };

        Ok(count as u32)
    }

    /// Count executions for a scope across all time.
    async fn count_executions(&self, scope: &PolicyScope) -> Result<u32> {
        let count: i64 = match scope {
            PolicyScope::Global => {
                sqlx::query_scalar("SELECT COUNT(*) FROM execution")
                    .fetch_one(&self.pool)
                    .await?
            }
            PolicyScope::Pack(pack_id) => {
                sqlx::query_scalar(
                    r#"
                    SELECT COUNT(*)
                    FROM execution e
                    JOIN action a ON e.action = a.id
                    WHERE a.pack = $1
                    "#,
                )
                .bind(pack_id)
                .fetch_one(&self.pool)
                .await?
            }
            PolicyScope::Action(action_id) => {
                sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE action = $1")
                    .bind(action_id)
                    .fetch_one(&self.pool)
                    .await?
            }
            PolicyScope::Identity(_identity_id) => {
                sqlx::query_scalar("SELECT COUNT(*) FROM execution")
                    .fetch_one(&self.pool)
                    .await?
            }
        };

        Ok(count as u32)
    }

    /// Count currently running executions
    async fn count_running_executions(&self, scope: &PolicyScope) -> Result<u32> {
        let count: i64 = match scope {
            PolicyScope::Global => {
                sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE status = $1")
                    .bind(ExecutionStatus::Running)
                    .fetch_one(&self.pool)
                    .await?
            }
            PolicyScope::Pack(pack_id) => {
                sqlx::query_scalar(
                    r#"
                    SELECT COUNT(*)
                    FROM execution e
                    JOIN action a ON e.action = a.id
                    WHERE a.pack = $1 AND e.status = $2
                    "#,
                )
                .bind(pack_id)
                .bind(ExecutionStatus::Running)
                .fetch_one(&self.pool)
                .await?
            }
            PolicyScope::Action(action_id) => {
                sqlx::query_scalar(
                    "SELECT COUNT(*) FROM execution WHERE action = $1 AND status = $2",
                )
                .bind(action_id)
                .bind(ExecutionStatus::Running)
                .fetch_one(&self.pool)
                .await?
            }
            PolicyScope::Identity(_identity_id) => {
                // TODO: Track executions by identity/tenant
                // For now, treat as global
                sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE status = $1")
                    .bind(ExecutionStatus::Running)
                    .fetch_one(&self.pool)
                    .await?
            }
        };

        Ok(count as u32)
    }

    /// Wait for policy compliance (block until policies allow execution)
    #[allow(dead_code)]
    pub async fn wait_for_policy_compliance(
        &self,
        action_id: Id,
        pack_id: Option<Id>,
        max_wait_seconds: u32,
    ) -> Result<bool> {
        let start = Utc::now();
        let max_wait = Duration::seconds(max_wait_seconds as i64);

        loop {
            // Check if policies allow execution
            if self.check_policies(action_id, pack_id).await?.is_none() {
                return Ok(true);
            }

            // Check if we've exceeded max wait time
            if Utc::now() - start > max_wait {
                warn!(
                    "Policy compliance timeout after {} seconds for action {}",
                    max_wait_seconds, action_id
                );
                return Ok(false);
            }

            // Wait a bit before checking again
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }
}

fn extract_parameter_value(config: Option<&JsonValue>, path: &str) -> JsonValue {
    let mut current = match config {
        Some(value) => value,
        None => return JsonValue::Null,
    };

    for segment in path.split('.') {
        match current {
            JsonValue::Object(map) => match map.get(segment) {
                Some(next) => current = next,
                None => return JsonValue::Null,
            },
            _ => return JsonValue::Null,
        }
    }

    current.clone()
}

fn parse_policy_quotas(value: &JsonValue) -> Option<HashMap<String, u64>> {
    let array = value.as_array()?;
    let mut quotas = HashMap::new();

    for quota in array {
        let Some(quota_type) = quota.get("quota_type").and_then(JsonValue::as_str) else {
            continue;
        };
        let Some(limit) = quota.get("limit").and_then(JsonValue::as_u64) else {
            continue;
        };
        if limit > 0 {
            quotas.insert(quota_type.to_string(), limit);
        }
    }

    if quotas.is_empty() {
        None
    } else {
        Some(quotas)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_violation_display() {
        let violation = PolicyViolation::RateLimitExceeded {
            limit: 10,
            window_seconds: 60,
            current_count: 15,
        };
        assert!(violation.to_string().contains("Rate limit exceeded"));

        let violation = PolicyViolation::ConcurrencyLimitExceeded {
            limit: 5,
            current_count: 7,
        };
        assert!(violation.to_string().contains("Concurrency limit exceeded"));

        let violation = PolicyViolation::QuotaExceeded {
            quota_type: "cpu".to_string(),
            limit: 100,
            current_usage: 150,
        };
        assert!(violation.to_string().contains("cpu quota exceeded"));
    }

    #[test]
    fn test_execution_policy_default() {
        let policy = ExecutionPolicy::default();
        assert!(policy.rate_limit.is_none());
        assert!(policy.concurrency_limit.is_none());
        assert_eq!(policy.concurrency_method, PolicyMethod::Enqueue);
        assert!(policy.concurrency_parameters.is_empty());
        assert!(policy.quotas.is_none());
    }

    #[test]
    fn test_rate_limit() {
        let rate_limit = RateLimit {
            max_executions: 10,
            window_seconds: 60,
        };
        assert_eq!(rate_limit.max_executions, 10);
        assert_eq!(rate_limit.window_seconds, 60);
    }

    #[test]
    fn test_policy_scope_equality() {
        assert_eq!(PolicyScope::Global, PolicyScope::Global);
        assert_eq!(PolicyScope::Pack(1), PolicyScope::Pack(1));
        assert_ne!(PolicyScope::Pack(1), PolicyScope::Pack(2));
        assert_eq!(PolicyScope::Action(1), PolicyScope::Action(1));
        assert_ne!(PolicyScope::Action(1), PolicyScope::Action(2));
    }

    #[tokio::test]
    async fn test_get_concurrency_limit_action_specific() {
        let pool = sqlx::PgPool::connect_lazy("postgresql://localhost/test").unwrap();
        let mut enforcer = PolicyEnforcer::new(pool);

        // Set action-specific policy
        let policy = ExecutionPolicy {
            concurrency_limit: Some(5),
            ..Default::default()
        };
        enforcer.set_action_policy(1, policy);

        assert_eq!(enforcer.get_concurrency_limit(1, None), Some(5));
        assert_eq!(enforcer.get_concurrency_limit(2, None), None);
    }

    #[tokio::test]
    async fn test_get_concurrency_limit_pack() {
        let pool = sqlx::PgPool::connect_lazy("postgresql://localhost/test").unwrap();
        let mut enforcer = PolicyEnforcer::new(pool);

        // Set pack policy
        let policy = ExecutionPolicy {
            concurrency_limit: Some(10),
            ..Default::default()
        };
        enforcer.set_pack_policy(100, policy);

        assert_eq!(enforcer.get_concurrency_limit(1, Some(100)), Some(10));
        assert_eq!(enforcer.get_concurrency_limit(1, Some(200)), None);
    }

    #[tokio::test]
    async fn test_get_concurrency_limit_global() {
        let pool = sqlx::PgPool::connect_lazy("postgresql://localhost/test").unwrap();
        let policy = ExecutionPolicy {
            concurrency_limit: Some(20),
            ..Default::default()
        };
        let enforcer = PolicyEnforcer::with_global_policy(pool, policy);

        assert_eq!(enforcer.get_concurrency_limit(1, None), Some(20));
    }

    #[tokio::test]
    async fn test_get_concurrency_limit_precedence() {
        let pool = sqlx::PgPool::connect_lazy("postgresql://localhost/test").unwrap();
        let mut enforcer = PolicyEnforcer::new(pool);

        // Set all levels
        enforcer.set_global_policy(ExecutionPolicy {
            concurrency_limit: Some(20),
            ..Default::default()
        });

        enforcer.set_pack_policy(
            100,
            ExecutionPolicy {
                concurrency_limit: Some(10),
                ..Default::default()
            },
        );

        enforcer.set_action_policy(
            1,
            ExecutionPolicy {
                concurrency_limit: Some(5),
                ..Default::default()
            },
        );

        // Action-specific should take precedence
        assert_eq!(enforcer.get_concurrency_limit(1, Some(100)), Some(5));

        // Without action policy, pack should take precedence
        assert_eq!(enforcer.get_concurrency_limit(2, Some(100)), Some(10));

        // Without action or pack policy, global should apply
        assert_eq!(enforcer.get_concurrency_limit(2, Some(200)), Some(20));
    }

    #[tokio::test]
    async fn test_build_parameter_group_key_uses_exact_values() {
        let pool = sqlx::PgPool::connect_lazy("postgresql://localhost/test").unwrap();
        let enforcer = PolicyEnforcer::new(pool);
        let config = serde_json::json!({
            "environment": "prod",
            "target": {
                "region": "us-east-1"
            }
        });

        let group_key = enforcer.build_parameter_group_key(
            &["target.region".to_string(), "environment".to_string()],
            Some(&config),
        );

        assert_eq!(
            group_key.as_deref(),
            Some("{\"environment\":\"prod\",\"target.region\":\"us-east-1\"}")
        );
    }

    // Integration tests would require database setup
    // Those should be in a separate integration test file
}
