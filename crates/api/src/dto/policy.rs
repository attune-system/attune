//! Policy DTOs for API requests and responses

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use attune_common::models::enums::PolicyMethod;

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    50
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PolicyScopeKind {
    Global,
    Pack,
    Action,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct PolicyListParams {
    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    #[serde(default = "default_page_size")]
    #[param(example = 50, minimum = 1, maximum = 100)]
    pub page_size: u32,

    #[param(example = "action")]
    pub scope: Option<PolicyScopeKind>,

    #[param(example = "core")]
    pub pack_ref: Option<String>,

    #[param(example = "core.echo")]
    pub action_ref: Option<String>,
}

impl PolicyListParams {
    pub fn offset(&self) -> usize {
        (self.page.saturating_sub(1) * self.limit()) as usize
    }

    pub fn limit(&self) -> u32 {
        self.page_size.min(100)
    }
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct CreatePolicyRequest {
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "core.echo_concurrency")]
    pub r#ref: String,

    #[schema(example = "core", nullable = true)]
    pub pack_ref: Option<String>,

    #[schema(example = "core.echo", nullable = true)]
    pub action_ref: Option<String>,

    #[serde(default)]
    #[schema(example = json!(["customer_id"]))]
    pub parameters: Vec<String>,

    #[schema(example = "enqueue")]
    pub method: PolicyMethod,

    #[validate(range(min = 1))]
    #[schema(example = 3)]
    pub threshold: i32,

    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Limit core.echo concurrency")]
    pub name: String,

    #[schema(
        example = "Keeps core.echo executions within downstream capacity",
        nullable = true
    )]
    pub description: Option<String>,

    #[serde(default)]
    #[schema(example = json!(["operator-managed"]))]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct UpdatePolicyRequest {
    #[schema(example = json!(["customer_id"]), nullable = true)]
    pub parameters: Option<Vec<String>>,

    #[schema(example = "enqueue", nullable = true)]
    pub method: Option<PolicyMethod>,

    #[validate(range(min = 1))]
    #[schema(example = 5, nullable = true)]
    pub threshold: Option<i32>,

    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Limit core.echo concurrency", nullable = true)]
    pub name: Option<String>,

    #[schema(example = "Updated policy description", nullable = true)]
    pub description: Option<String>,

    #[schema(example = json!(["operator-managed"]), nullable = true)]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PolicyResponse {
    #[schema(example = 1)]
    pub id: i64,

    #[schema(example = "core.echo_concurrency")]
    pub r#ref: String,

    #[schema(example = "action")]
    pub scope: PolicyScopeKind,

    #[schema(example = 1, nullable = true)]
    pub pack: Option<i64>,

    #[schema(example = "core", nullable = true)]
    pub pack_ref: Option<String>,

    #[schema(example = 1, nullable = true)]
    pub action: Option<i64>,

    #[schema(example = "core.echo", nullable = true)]
    pub action_ref: Option<String>,

    #[schema(example = json!(["customer_id"]))]
    pub parameters: Vec<String>,

    #[schema(example = "enqueue")]
    pub method: PolicyMethod,

    #[schema(example = 3)]
    pub threshold: i32,

    #[schema(example = "Limit core.echo concurrency")]
    pub name: String,

    #[schema(
        example = "Keeps core.echo executions within downstream capacity",
        nullable = true
    )]
    pub description: Option<String>,

    #[schema(example = json!(["operator-managed"]))]
    pub tags: Vec<String>,

    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

pub type PolicySummary = PolicyResponse;

impl From<attune_common::models::Policy> for PolicyResponse {
    fn from(policy: attune_common::models::Policy) -> Self {
        let scope = if policy.action.is_some() {
            PolicyScopeKind::Action
        } else if policy.pack.is_some() {
            PolicyScopeKind::Pack
        } else {
            PolicyScopeKind::Global
        };

        Self {
            id: policy.id,
            r#ref: policy.r#ref,
            scope,
            pack: policy.pack,
            pack_ref: policy.pack_ref,
            action: policy.action,
            action_ref: policy.action_ref,
            parameters: policy.parameters,
            method: policy.method,
            threshold: policy.threshold,
            name: policy.name,
            description: policy.description,
            tags: policy.tags,
            created: policy.created,
            updated: policy.updated,
        }
    }
}
