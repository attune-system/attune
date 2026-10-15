use attune_common::{
    rbac::Grant, repositories::runtime_authority::ExternalApprovalRecord,
    runtime_authority::PermissionGrantDefinition,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueRuntimeApprovalRequest {
    /// Fixed approved grants. Human-relative scopes are not accepted.
    #[schema(value_type = Vec<Object>)]
    pub grants: Vec<PermissionGrantDefinition>,
    /// Omit for no automatic expiry. Explicit revocation remains available.
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RuntimeApprovalResponse {
    pub id: i64,
    pub subject_pack_id: i64,
    pub issuer_identity_id: i64,
    pub issuer_login: String,
    #[schema(value_type = Vec<Object>)]
    pub grants: Vec<Grant>,
    pub approval_digest: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub active: bool,
}

impl From<ExternalApprovalRecord> for RuntimeApprovalResponse {
    fn from(record: ExternalApprovalRecord) -> Self {
        let active = record.is_active(Utc::now());
        Self {
            id: record.id,
            subject_pack_id: record.subject_pack,
            issuer_identity_id: record.issuer_identity,
            issuer_login: record.issuer_login,
            grants: record.grants.0,
            approval_digest: record.approval_digest,
            expires_at: record.expires_at,
            revoked_at: record.revoked_at,
            active,
        }
    }
}
