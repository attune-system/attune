//! Key/Secret data transfer objects

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use attune_common::models::{key::Key, Id, OwnerType};

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SignKeyJwtRequest {
    #[validate(length(min = 1, max = 255))]
    pub profile_ref: String,
    #[validate(length(min = 1, max = 255))]
    pub subject: String,
    #[validate(range(min = 1, max = 300))]
    pub ttl_seconds: u32,
}

#[derive(Serialize, ToSchema)]
pub struct SignKeyJwtResponse {
    pub assertion: String,
    pub expires_at: i64,
}

/// Full key response with all details (value redacted in list views)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct KeyResponse {
    /// Unique key ID
    #[schema(example = 1)]
    pub id: Id,

    /// Unique reference identifier
    #[schema(example = "system.github_token")]
    pub r#ref: String,

    /// Key identifier within its owner scope
    #[schema(example = "github_token")]
    pub local_ref: String,

    /// Type of owner
    pub owner_type: OwnerType,

    /// Authoritative owner reference
    #[schema(example = "github")]
    pub owner: Option<String>,

    /// Owner identity ID
    #[schema(example = 1)]
    pub owner_identity: Option<Id>,

    /// Owner pack ID
    #[schema(example = 1)]
    pub owner_pack: Option<Id>,

    /// Owner pack reference
    #[schema(example = "github")]
    pub owner_pack_ref: Option<String>,

    /// Owner action ID
    #[schema(example = 1)]
    pub owner_action: Option<Id>,

    /// Owner action reference
    #[schema(example = "github.create_issue")]
    pub owner_action_ref: Option<String>,

    /// Owner sensor ID
    #[schema(example = 1)]
    pub owner_sensor: Option<Id>,

    /// Owner sensor reference
    #[schema(example = "github.webhook")]
    pub owner_sensor_ref: Option<String>,

    /// Human-readable name
    #[schema(example = "GitHub API Token")]
    pub name: String,

    /// Whether the value is encrypted
    #[schema(example = true)]
    pub encrypted: bool,

    /// JSON value, or null when the caller cannot read and decrypt encrypted material.
    #[schema(value_type = Value, example = json!("ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"))]
    pub value: JsonValue,

    pub value_schema: Option<JsonValue>,
    pub schema_binding: Option<JsonValue>,
    pub schema_digest: Option<String>,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    /// Last update timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

impl From<Key> for KeyResponse {
    fn from(key: Key) -> Self {
        Self {
            id: key.id,
            r#ref: key.r#ref,
            local_ref: key.local_ref,
            owner_type: key.owner_type,
            owner: key.owner,
            owner_identity: key.owner_identity,
            owner_pack: key.owner_pack,
            owner_pack_ref: key.owner_pack_ref,
            owner_action: key.owner_action,
            owner_action_ref: key.owner_action_ref,
            owner_sensor: key.owner_sensor,
            owner_sensor_ref: key.owner_sensor_ref,
            name: key.name,
            encrypted: key.encrypted,
            value: key.value,
            value_schema: None,
            schema_binding: None,
            schema_digest: None,
            created: key.created,
            updated: key.updated,
        }
    }
}

impl KeyResponse {
    pub fn with_schema(
        mut self,
        assignment: Option<&attune_common::repositories::key_schema::KeySchemaAssignment>,
    ) -> attune_common::Result<Self> {
        if let Some(assignment) = assignment {
            self.value_schema = Some(assignment.authored_schema.clone());
            self.schema_binding =
                Some(serde_json::to_value(&assignment.contract).map_err(|_| {
                    attune_common::Error::validation("Invalid Key rendering contract")
                })?);
            self.schema_digest = Some(assignment.contract.semantic_digest().map_err(|error| {
                attune_common::Error::validation(format!("Invalid Key schema digest: {error}"))
            })?);
        }
        Ok(self)
    }
}

/// Summary key response for list views (value redacted)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct KeySummary {
    /// Unique key ID
    #[schema(example = 1)]
    pub id: Id,

    /// Unique reference identifier
    #[schema(example = "system.github_token")]
    pub r#ref: String,

    /// Key identifier within its owner scope
    #[schema(example = "github_token")]
    pub local_ref: String,

    /// Type of owner
    pub owner_type: OwnerType,

    /// Authoritative owner reference
    #[schema(example = "github")]
    pub owner: Option<String>,

    /// Human-readable name
    #[schema(example = "GitHub API Token")]
    pub name: String,

    /// Whether the value is encrypted
    #[schema(example = true)]
    pub encrypted: bool,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,
}

impl From<Key> for KeySummary {
    fn from(key: Key) -> Self {
        Self {
            id: key.id,
            r#ref: key.r#ref,
            local_ref: key.local_ref,
            owner_type: key.owner_type,
            owner: key.owner,
            name: key.name,
            encrypted: key.encrypted,
            created: key.created,
        }
    }
}

/// Request to create a new key/secret
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct CreateKeyRequest {
    /// Identifier within the selected owner scope. The server uses it to construct the canonical ref.
    #[validate(length(min = 1, max = 63))]
    #[schema(example = "github_token")]
    pub local_ref: String,

    /// Type of owner (system, identity, pack, action, sensor)
    pub owner_type: OwnerType,

    /// Optional owner identity login
    #[validate(length(max = 255))]
    #[schema(example = "alice@example.com")]
    pub owner_identity_login: Option<String>,

    /// Optional owner pack reference
    #[validate(length(max = 255))]
    #[schema(example = "github")]
    pub owner_pack_ref: Option<String>,

    /// Optional owner action reference
    #[validate(length(max = 255))]
    #[schema(example = "github.create_issue")]
    pub owner_action_ref: Option<String>,

    /// Optional owner sensor reference
    #[validate(length(max = 255))]
    #[schema(example = "github.webhook")]
    pub owner_sensor_ref: Option<String>,

    /// Human-readable name for the key
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "GitHub API Token")]
    pub name: String,

    /// The secret value to store. Can be a string, object, array, number, or boolean.
    #[schema(value_type = Value, example = json!("ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"))]
    pub value: JsonValue,

    /// Whether to encrypt the value at rest (default: false; use --encrypt / -e from CLI)
    #[serde(default)]
    #[schema(example = false)]
    pub encrypted: bool,
    /// Optional inline or named single-value schema. Selection is server-owned.
    pub value_schema: Option<JsonValue>,
    /// Optional preview comparison, never a contract/revision selector.
    pub schema_digest: Option<String>,
}

/// Request to update an existing key/secret
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct UpdateKeyRequest {
    /// Update the human-readable name
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "GitHub API Token (Updated)")]
    pub name: Option<String>,

    /// Update the secret value. Can be a string, object, array, number, or boolean.
    #[schema(value_type = Option<Value>, example = json!("ghp_new_token_xxxxxxxxxxxxxxxxxxxxxxxx"))]
    #[serde(default, deserialize_with = "deserialize_present_json")]
    pub value: Option<JsonValue>,

    /// Update encryption status (re-encrypts if changing from false to true)
    #[schema(example = true)]
    pub encrypted: Option<bool>,
    /// Omit to retain the assigned schema. Null removes it.
    #[serde(default, deserialize_with = "deserialize_present_json")]
    pub value_schema: Option<JsonValue>,
    /// Explicitly select current installed named revisions and revalidate.
    #[serde(default)]
    pub rebind_schema: bool,
    pub schema_digest: Option<String>,
}

fn deserialize_present_json<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<JsonValue>, D::Error> {
    JsonValue::deserialize(deserializer).map(Some)
}

/// Query parameters for filtering keys
#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
pub struct KeyQueryParams {
    /// Filter by owner type
    #[param(example = "pack")]
    pub owner_type: Option<OwnerType>,

    /// Filter by owner string
    #[param(example = "github-integration")]
    pub owner: Option<String>,

    /// Page number (1-indexed)
    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    /// Items per page
    #[serde(default = "default_per_page")]
    #[param(example = 50, minimum = 1, maximum = 100)]
    pub per_page: u32,
}

fn default_page() -> u32 {
    1
}

fn default_per_page() -> u32 {
    50
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    #[test]
    fn update_distinguishes_missing_values_and_schemas_from_explicit_null() {
        let missing: UpdateKeyRequest = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(missing.value.is_none());
        assert!(missing.value_schema.is_none());
        assert!(!missing.rebind_schema);
        let explicit: UpdateKeyRequest =
            serde_json::from_value(serde_json::json!({"value": null, "value_schema": null}))
                .unwrap();
        assert_eq!(explicit.value, Some(JsonValue::Null));
        assert_eq!(explicit.value_schema, Some(JsonValue::Null));
    }
}

impl KeyQueryParams {
    /// Get the offset for pagination
    pub fn offset(&self) -> u32 {
        (self.page - 1) * self.per_page
    }

    /// Get the limit for pagination
    pub fn limit(&self) -> u32 {
        self.per_page
    }
}
