use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use attune_common::{
    models::ManagementOriginKind,
    repositories::identity::{
        DirectPermissionBindingRow, PermissionBindingRow, PermissionSetAdminRow,
    },
};

#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct IdentityQueryParams {
    /// Exact identity login.
    pub login: Option<String>,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct PermissionSetQueryParams {
    #[serde(default)]
    pub pack_ref: Option<String>,
    /// Include retired permission sets in administrative results.
    #[serde(default)]
    pub include_retired: bool,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct PermissionBindingQueryParams {
    pub identity_id: Option<i64>,
    pub identity_login: Option<String>,
    pub role: Option<String>,
    pub permission_set_ref: Option<String>,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct PermissionUpdateQueryParams {
    /// Validate and preview the update without persisting it.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(untagged)]
pub enum PermissionBindingTarget {
    Identity(PermissionIdentityTarget),
    Role(PermissionRoleTarget),
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PermissionIdentityTarget {
    pub identity_id: i64,
    pub login: String,
    #[serde(rename = "type")]
    pub target_type: PermissionIdentityTargetType,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionIdentityTargetType {
    Identity,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PermissionRoleTarget {
    pub role: String,
    #[serde(rename = "type")]
    pub target_type: PermissionRoleTargetType,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRoleTargetType {
    Role,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PermissionBindingResponse {
    pub id: i64,
    pub permission_set_id: i64,
    pub permission_set_ref: String,
    pub target: PermissionBindingTarget,
    pub created: chrono::DateTime<chrono::Utc>,
}

impl TryFrom<PermissionBindingRow> for PermissionBindingResponse {
    type Error = attune_common::Error;

    fn try_from(row: PermissionBindingRow) -> Result<Self, Self::Error> {
        let target = match (row.identity_id, row.identity_login, row.role) {
            (Some(identity_id), Some(login), None) => {
                PermissionBindingTarget::Identity(PermissionIdentityTarget {
                    identity_id,
                    login,
                    target_type: PermissionIdentityTargetType::Identity,
                })
            }
            (None, None, Some(role)) => PermissionBindingTarget::Role(PermissionRoleTarget {
                role,
                target_type: PermissionRoleTargetType::Role,
            }),
            _ => {
                return Err(attune_common::Error::invalid_state(
                    "Invalid permission binding target",
                ))
            }
        };
        Ok(Self {
            id: row.id,
            permission_set_id: row.permission_set_id,
            permission_set_ref: row.permission_set_ref,
            target,
            created: row.created,
        })
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct IdentitySummary {
    pub id: i64,
    pub login: String,
    pub display_name: Option<String>,
    pub frozen: bool,
    pub attributes: JsonValue,
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct IdentityRoleAssignmentResponse {
    pub id: i64,
    pub identity_id: i64,
    pub role: String,
    pub source: String,
    pub managed: bool,
    pub created: chrono::DateTime<chrono::Utc>,
    pub updated: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct IdentityResponse {
    pub id: i64,
    pub login: String,
    pub display_name: Option<String>,
    pub frozen: bool,
    pub attributes: JsonValue,
    pub roles: Vec<IdentityRoleAssignmentResponse>,
    pub direct_permissions: Vec<PermissionAssignmentResponse>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PermissionSetSummary {
    pub id: i64,
    pub r#ref: String,
    pub pack_ref: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub grants: JsonValue,
    pub retired_at: Option<chrono::DateTime<chrono::Utc>>,
    pub management_origin: ManagementOriginKind,
    pub roles: Vec<PermissionSetRoleAssignmentResponse>,
}

impl From<PermissionSetAdminRow> for PermissionSetSummary {
    fn from(row: PermissionSetAdminRow) -> Self {
        let set = row.permission_set;
        Self {
            roles: row
                .roles
                .0
                .into_iter()
                .map(|role| PermissionSetRoleAssignmentResponse {
                    id: role.id,
                    permission_set_id: role.permset,
                    permission_set_ref: Some(set.r#ref.clone()),
                    role: role.role,
                    created: role.created,
                })
                .collect(),
            id: set.id,
            r#ref: set.r#ref,
            pack_ref: set.pack_ref,
            label: set.label,
            description: set.description,
            grants: set.grants,
            retired_at: set.retired_at,
            management_origin: row.management_origin,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct UpdatePermissionSetRequest {
    #[serde(default)]
    #[validate(length(max = 255))]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub grants: JsonValue,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PermissionAssignmentResponse {
    pub id: i64,
    pub identity_id: i64,
    pub permission_set_id: i64,
    pub permission_set_ref: String,
    pub created: chrono::DateTime<chrono::Utc>,
}

impl From<DirectPermissionBindingRow> for PermissionAssignmentResponse {
    fn from(row: DirectPermissionBindingRow) -> Self {
        Self {
            id: row.id,
            identity_id: row.identity_id,
            permission_set_id: row.permission_set_id,
            permission_set_ref: row.permission_set_ref,
            created: row.created,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PermissionSetRoleAssignmentResponse {
    pub id: i64,
    pub permission_set_id: i64,
    pub permission_set_ref: Option<String>,
    pub role: String,
    pub created: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreatePermissionAssignmentRequest {
    pub identity_id: Option<i64>,
    pub identity_login: Option<String>,
    pub permission_set_ref: String,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct CreateIdentityRoleAssignmentRequest {
    #[validate(length(min = 1, max = 255))]
    pub role: String,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct CreatePermissionSetRoleAssignmentRequest {
    #[validate(length(min = 1, max = 255))]
    pub role: String,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct CreateIdentityRequest {
    #[validate(length(min = 3, max = 255))]
    pub login: String,
    #[validate(length(max = 255))]
    pub display_name: Option<String>,
    #[validate(length(min = 8, max = 128))]
    pub password: Option<String>,
    #[serde(default)]
    pub attributes: JsonValue,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct UpdateIdentityRequest {
    pub display_name: Option<String>,
    pub password: Option<String>,
    pub attributes: Option<JsonValue>,
    pub frozen: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct CreateIntegrationTokenRequest {
    #[validate(length(min = 1, max = 255))]
    pub label: String,
    #[validate(length(max = 2000))]
    pub description: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct RevokeIntegrationTokenRequest {
    #[validate(length(max = 2000))]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct IntegrationTokenResponse {
    pub id: i64,
    pub identity_id: i64,
    pub label: String,
    pub description: Option<String>,
    pub token_prefix: String,
    pub token_suffix: String,
    pub created_by: Option<i64>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_used_ip: Option<String>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub revoked_by: Option<i64>,
    pub revocation_reason: Option<String>,
    pub active: bool,
    pub created: chrono::DateTime<chrono::Utc>,
    pub updated: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CreateIntegrationTokenResponse {
    pub token: String,
    pub integration_token: IntegrationTokenResponse,
}

impl From<attune_common::models::IntegrationToken> for IntegrationTokenResponse {
    fn from(value: attune_common::models::IntegrationToken) -> Self {
        let now = chrono::Utc::now();
        let active = value.revoked_at.is_none()
            && value
                .expires_at
                .map(|expires_at| expires_at > now)
                .unwrap_or(true);

        Self {
            id: value.id,
            identity_id: value.identity,
            label: value.label,
            description: value.description,
            token_prefix: value.token_prefix,
            token_suffix: value.token_suffix,
            created_by: value.created_by,
            expires_at: value.expires_at,
            last_used_at: value.last_used_at,
            last_used_ip: value.last_used_ip,
            revoked_at: value.revoked_at,
            revoked_by: value.revoked_by,
            revocation_reason: value.revocation_reason,
            active,
            created: value.created,
            updated: value.updated,
        }
    }
}
