use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use attune_common::models::identity::ExternalIdentityMapping;

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateExternalIdentityMappingRequest {
    #[validate(range(min = 1))]
    pub mapped_identity: i64,
    #[validate(length(min = 1, max = 64))]
    pub provider: String,
    #[validate(length(min = 1, max = 255))]
    pub tenant: String,
    #[validate(length(min = 1, max = 255))]
    pub external_subject: String,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateExternalIdentityMappingRequest {
    #[validate(range(min = 1))]
    pub mapped_identity: i64,
    #[validate(length(min = 1, max = 64))]
    pub provider: String,
    #[validate(length(min = 1, max = 255))]
    pub tenant: String,
    #[validate(length(min = 1, max = 255))]
    pub external_subject: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ExternalIdentityMappingResponse {
    pub id: i64,
    pub integration_identity: i64,
    pub mapped_identity: i64,
    pub provider: String,
    pub tenant: String,
    pub external_subject: String,
    pub created_by: Option<i64>,
    pub created: chrono::DateTime<chrono::Utc>,
    pub updated: chrono::DateTime<chrono::Utc>,
}

impl From<ExternalIdentityMapping> for ExternalIdentityMappingResponse {
    fn from(value: ExternalIdentityMapping) -> Self {
        Self {
            id: value.id,
            integration_identity: value.integration_identity,
            mapped_identity: value.mapped_identity,
            provider: value.provider,
            tenant: value.tenant,
            external_subject: value.external_subject,
            created_by: value.created_by,
            created: value.created,
            updated: value.updated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_request_rejects_credentials_and_server_owned_fields() {
        for field in ["token", "secret", "password", "created_by"] {
            let mut value = serde_json::json!({
                "mapped_identity": 2,
                "provider": "github",
                "tenant": "acme",
                "external_subject": "user-42"
            });
            value[field] = serde_json::json!("must-not-be-accepted");

            assert!(serde_json::from_value::<CreateExternalIdentityMappingRequest>(value).is_err());
        }
    }

    #[test]
    fn request_bounds_are_validated() {
        let request = CreateExternalIdentityMappingRequest {
            mapped_identity: 0,
            provider: "x".repeat(65),
            tenant: String::new(),
            external_subject: "x".repeat(256),
        };

        let errors = request.validate().expect_err("invalid request must fail");
        assert!(errors.field_errors().contains_key("mapped_identity"));
        assert!(errors.field_errors().contains_key("provider"));
        assert!(errors.field_errors().contains_key("tenant"));
        assert!(errors.field_errors().contains_key("external_subject"));
    }
}
