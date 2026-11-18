//! Canonical v1 publication of files written in a launch's private artifacts
//! directory. This is distinct from runtime-stage publication and raw log I/O.
//! Only the CP's sealed allocation inventory supplies paths. A terminal state
//! alone never authorizes staging deletion: an authenticated cleanup receipt
//! also proves settlement of owned provider operations.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::workload_launch::{LaunchRequest, LaunchToken, Sha256Digest};

pub const ARTIFACT_PUBLICATION_HEADER: &str = "x-attune-artifact-publication";
pub const ARTIFACT_UPLOAD_HEADER: &str = "x-attune-artifact-upload";
pub const ARTIFACTS_ROOT: &str = "/tmp/artifacts";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactPublicationState {
    /// File allocation is permitted only while the associated Run is open.
    Open,
    /// Run is closed; the immutable inventory awaits acknowledged removal.
    Pending,
    Publishing,
    Complete,
    Incomplete,
}

/// Server-selected exact prepared version. Paths are relative to ARTIFACTS_ROOT,
/// never host paths, and never supplied by the controller's upload request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreparedArtifactFile {
    pub version_id: i64,
    pub relative_path: String,
    pub max_bytes: u64,
}

/// Independent publication authority. It grants no start, invocation, input,
/// runtime-stage or log access, and is bound to a fresh controller incarnation.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPublicationLease {
    pub request: LaunchRequest,
    pub publication_id: i64,
    pub generation: i64,
    pub lease_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub token: LaunchToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPublicationReceipt {
    pub request: LaunchRequest,
    pub publication_id: i64,
    /// Only Complete or Incomplete. Historical acknowledgement, not authority.
    pub state: ArtifactPublicationState,
    /// Issued only after all native upload ownership has settled. Retain this
    /// receipt to retry local cleanup without claiming a timed-out write absent.
    pub cleanup_receipt_id: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClaimedArtifactPublication {
    NotReady {
        request: LaunchRequest,
        state: ArtifactPublicationState,
    },
    Transfer {
        lease: ArtifactPublicationLease,
        files: Vec<PreparedArtifactFile>,
        /// Fixed on Run close; renewing a claim cannot extend this bound.
        deadline: DateTime<Utc>,
    },
    Settled {
        receipt: ArtifactPublicationReceipt,
    },
}

/// The controller measures the exact selected file after engine removal. An
/// immutable per-version operation must be reserved before uploading its body.
/// Different size/digest retries cannot replace the original operation.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReserveArtifactUpload {
    pub request: LaunchRequest,
    pub publication_id: i64,
    pub lease_id: Uuid,
    pub generation: i64,
    pub version_id: i64,
    pub size_bytes: u64,
    pub sha256: Sha256Digest,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactUploadGrant {
    pub operation_id: i64,
    pub version_id: i64,
    pub size_bytes: u64,
    pub sha256: Sha256Digest,
    pub expires_at: DateTime<Utc>,
    pub token: LaunchToken,
}

/// Missing files are explicit incomplete results, never silently omitted from
/// the sealed inventory. This does not assert that a pending PUT is absent.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MissingArtifactFile {
    pub request: LaunchRequest,
    pub publication_id: i64,
    pub lease_id: Uuid,
    pub generation: i64,
    pub version_id: i64,
}
