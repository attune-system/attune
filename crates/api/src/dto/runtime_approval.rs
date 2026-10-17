use attune_common::{
    rbac::Grant,
    repositories::{
        public_input_approval::ApprovalPurpose, runtime_authority::ExternalApprovalRecord,
    },
    runtime_authority::{InvocationBinding, PermissionGrantDefinition},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct DeclaredInputTargetListParams {
    /// Omit for this pack's current policy; explicit IDs select retained metadata.
    #[param(minimum = 1)]
    pub policy_revision: Option<i64>,
    #[serde(default = "target_page")]
    #[param(minimum = 1, example = 1)]
    pub page: u32,
    #[serde(default = "target_page_size")]
    #[param(minimum = 1, maximum = 100, example = 20)]
    pub page_size: u32,
}

fn target_page() -> u32 {
    1
}
fn target_page_size() -> u32 {
    20
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueRuntimeApprovalRequest {
    /// Fixed approved grants. Human-relative scopes are not accepted.
    pub grants: Vec<PermissionGrantDefinition>,
    /// Omit for no automatic expiry. Explicit revocation remains available.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Canonical flat JSON request. Exactly one target form is accepted; purpose is
/// bound by the server's retained declaration, never selected by this request.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum IssueDeclaredInputApprovalRequest {
    Renderer(RendererInputApprovalRequest),
    Policy(PolicyInputApprovalRequest),
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RendererInputApprovalRequest {
    /// Convenience target for an already admitted renderer, not a policy target.
    #[schema(minimum = 1)]
    pub renderer_execution: i64,
    pub declaration_revision: String,
    pub source_path: String,
    pub expression: String,
    pub grants: Vec<PermissionGrantDefinition>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyInputApprovalRequest {
    /// Pre-execution target, supplied with invocation_binding, without a renderer.
    #[schema(minimum = 1)]
    pub policy_revision: i64,
    pub invocation_binding: InvocationBinding,
    pub declaration_revision: String,
    pub source_path: String,
    pub expression: String,
    pub grants: Vec<PermissionGrantDefinition>,
    pub expires_at: Option<DateTime<Utc>>,
}

pub enum InputApprovalTarget {
    Renderer(i64),
    Policy(i64, InvocationBinding),
}

pub struct InputApprovalDeclaration {
    pub declaration_revision: String,
    pub source_path: String,
    pub expression: String,
    pub grants: Vec<PermissionGrantDefinition>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl IssueDeclaredInputApprovalRequest {
    pub fn into_parts(self) -> (InputApprovalTarget, InputApprovalDeclaration) {
        match self {
            Self::Renderer(request) => (
                InputApprovalTarget::Renderer(request.renderer_execution),
                InputApprovalDeclaration {
                    declaration_revision: request.declaration_revision,
                    source_path: request.source_path,
                    expression: request.expression,
                    grants: request.grants,
                    expires_at: request.expires_at,
                },
            ),
            Self::Policy(request) => (
                InputApprovalTarget::Policy(request.policy_revision, request.invocation_binding),
                InputApprovalDeclaration {
                    declaration_revision: request.declaration_revision,
                    source_path: request.source_path,
                    expression: request.expression,
                    grants: request.grants,
                    expires_at: request.expires_at,
                },
            ),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RuntimeApprovalResponse {
    pub id: i64,
    pub subject_pack_id: i64,
    pub issuer_identity_id: i64,
    pub issuer_login: String,
    pub grants: Vec<Grant>,
    pub approval_digest: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub active: bool,
    pub purpose: ApprovalPurpose,
}

impl RuntimeApprovalResponse {
    /// Purpose is mandatory retained metadata, not a default inferred by a DTO.
    pub fn from_record(record: ExternalApprovalRecord, purpose: ApprovalPurpose) -> Self {
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
            purpose,
        }
    }
}
