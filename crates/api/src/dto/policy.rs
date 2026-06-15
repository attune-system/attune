//! Policy DTOs for API requests and responses.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use attune_common::models::{enums::PolicyMethod, Policy};

use crate::dto::common::deserialize_double_option;

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    50
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyScopeType {
    Global,
    Pack,
    Action,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct PolicyScopeRequest {
    #[schema(example = "action")]
    pub r#type: PolicyScopeType,
    #[schema(example = "core", nullable = true)]
    pub pack_ref: Option<String>,
    #[schema(example = "core.echo", nullable = true)]
    pub action_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PolicyScopeResponse {
    #[schema(example = "action")]
    pub r#type: PolicyScopeType,
    #[schema(example = 1, nullable = true)]
    pub pack: Option<i64>,
    #[schema(example = "core", nullable = true)]
    pub pack_ref: Option<String>,
    #[schema(example = 1, nullable = true)]
    pub action: Option<i64>,
    #[schema(example = "core.echo", nullable = true)]
    pub action_ref: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Validate, ToSchema)]
pub struct ConcurrencyPolicyRequest {
    #[validate(range(min = 1))]
    #[schema(example = 5, minimum = 1)]
    pub limit: i32,
    #[schema(example = "enqueue")]
    pub method: PolicyMethod,
    #[serde(default)]
    #[schema(example = json!(["customer_id", "region"]), default = json!([]))]
    pub parameters: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Validate, ToSchema)]
pub struct RateLimitPolicyRequest {
    #[validate(range(min = 1))]
    #[schema(example = 100, minimum = 1)]
    pub max_executions: i32,
    #[validate(range(min = 1))]
    #[schema(example = 3600, minimum = 1)]
    pub window_seconds: i32,
}

#[derive(Debug, Clone, Deserialize, Serialize, Validate, ToSchema)]
pub struct QuotaPolicyRequest {
    #[validate(length(min = 1, max = 64))]
    #[schema(example = "running_executions")]
    pub quota_type: String,
    #[validate(range(min = 1))]
    #[schema(example = 10, minimum = 1)]
    pub limit: u64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConcurrencyPolicyResponse {
    #[schema(example = 5)]
    pub limit: i32,
    #[schema(example = "enqueue")]
    pub method: PolicyMethod,
    #[schema(example = json!(["customer_id"]))]
    pub parameters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RateLimitPolicyResponse {
    #[schema(example = 100)]
    pub max_executions: i32,
    #[schema(example = 3600)]
    pub window_seconds: i32,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct QuotaPolicyResponse {
    #[schema(example = "running_executions")]
    pub quota_type: String,
    #[schema(example = 10)]
    pub limit: u64,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct PolicyListParams {
    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    #[serde(default = "default_page_size")]
    #[param(example = 50, minimum = 1, maximum = 100)]
    pub page_size: u32,

    #[param(example = "core")]
    pub pack_ref: Option<String>,

    #[param(example = "core.echo")]
    pub action_ref: Option<String>,

    #[param(example = "action")]
    pub scope: Option<PolicyScopeType>,

    #[param(example = true)]
    pub enabled: Option<bool>,

    #[param(example = "production")]
    pub tag: Option<String>,
}

impl PolicyListParams {
    pub fn offset(&self) -> u32 {
        (self.page.saturating_sub(1)) * self.page_size
    }

    pub fn limit(&self) -> u32 {
        self.page_size.min(100)
    }
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct CreatePolicyRequest {
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "core.limit_echo")]
    pub r#ref: String,

    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Limit echo executions")]
    pub name: String,

    #[schema(example = "Limit concurrent echo executions by customer.")]
    pub description: Option<String>,

    #[serde(default = "default_true")]
    #[schema(example = true, default = true)]
    pub enabled: bool,

    #[serde(default)]
    #[schema(example = 10, default = 0)]
    pub priority: i32,

    pub scope: PolicyScopeRequest,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<ConcurrencyPolicyRequest>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimitPolicyRequest>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quotas: Vec<QuotaPolicyRequest>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(example = json!(["production"]), default = json!([]))]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct UpdatePolicyRequest {
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Limit echo executions")]
    pub name: Option<String>,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_double_option"
    )]
    #[schema(
        example = "Limit concurrent echo executions by customer.",
        nullable = true
    )]
    pub description: Option<Option<String>>,

    #[schema(example = true)]
    pub enabled: Option<bool>,

    #[schema(example = 10)]
    pub priority: Option<i32>,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_double_option"
    )]
    pub concurrency: Option<Option<ConcurrencyPolicyRequest>>,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_double_option"
    )]
    pub rate_limit: Option<Option<RateLimitPolicyRequest>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quotas: Option<Vec<QuotaPolicyRequest>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PolicySummary {
    pub id: i64,
    pub r#ref: String,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub priority: i32,
    pub scope: PolicyScopeResponse,
    pub concurrency: Option<ConcurrencyPolicyResponse>,
    pub rate_limit: Option<RateLimitPolicyResponse>,
    pub quotas: Vec<QuotaPolicyResponse>,
    pub tags: Vec<String>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PolicyResponse {
    pub id: i64,
    pub r#ref: String,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub priority: i32,
    pub scope: PolicyScopeResponse,
    pub concurrency: Option<ConcurrencyPolicyResponse>,
    pub rate_limit: Option<RateLimitPolicyResponse>,
    pub quotas: Vec<QuotaPolicyResponse>,
    pub tags: Vec<String>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
}

impl From<Policy> for PolicySummary {
    fn from(policy: Policy) -> Self {
        let detail = PolicyResponse::from(policy);
        Self {
            id: detail.id,
            r#ref: detail.r#ref,
            name: detail.name,
            description: detail.description,
            enabled: detail.enabled,
            priority: detail.priority,
            scope: detail.scope,
            concurrency: detail.concurrency,
            rate_limit: detail.rate_limit,
            quotas: detail.quotas,
            tags: detail.tags,
            created: detail.created,
            updated: detail.updated,
        }
    }
}

impl From<Policy> for PolicyResponse {
    fn from(policy: Policy) -> Self {
        let scope_type = if policy.action.is_some() {
            PolicyScopeType::Action
        } else if policy.pack.is_some() {
            PolicyScopeType::Pack
        } else {
            PolicyScopeType::Global
        };

        Self {
            id: policy.id,
            r#ref: policy.r#ref,
            name: policy.name,
            description: policy.description,
            enabled: policy.enabled,
            priority: policy.priority,
            scope: PolicyScopeResponse {
                r#type: scope_type,
                pack: policy.pack,
                pack_ref: policy.pack_ref,
                action: policy.action,
                action_ref: policy.action_ref,
            },
            concurrency: match (policy.threshold, policy.method) {
                (Some(limit), Some(method)) => Some(ConcurrencyPolicyResponse {
                    limit,
                    method,
                    parameters: policy.parameters,
                }),
                _ => None,
            },
            rate_limit: match (
                policy.rate_limit_max_executions,
                policy.rate_limit_window_seconds,
            ) {
                (Some(max_executions), Some(window_seconds)) => Some(RateLimitPolicyResponse {
                    max_executions,
                    window_seconds,
                }),
                _ => None,
            },
            quotas: quotas_from_json(&policy.quotas),
            tags: policy.tags,
            created: policy.created,
            updated: policy.updated,
        }
    }
}

pub fn quotas_to_json(quotas: &[QuotaPolicyRequest]) -> JsonValue {
    JsonValue::Array(
        quotas
            .iter()
            .map(|quota| json!({"quota_type": quota.quota_type, "limit": quota.limit}))
            .collect(),
    )
}

fn quotas_from_json(value: &JsonValue) -> Vec<QuotaPolicyResponse> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(QuotaPolicyResponse {
                        quota_type: item.get("quota_type")?.as_str()?.to_string(),
                        limit: item.get("limit")?.as_u64()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}
