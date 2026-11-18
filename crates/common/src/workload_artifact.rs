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

/// Bounds for the retained output tree, not a live filesystem quota. Entries
/// include distinct directory prefixes and regular files; the root is excluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactLimits {
    pub max_total_bytes: u64,
    pub max_entries: u64,
}
impl Default for ArtifactLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 256 * 1024 * 1024,
            max_entries: 4096,
        }
    }
}

/// Validate the CP-selected inventory using the same tree accounting as the
/// controller: shared directory prefixes count once, and a file cannot be a
/// parent of another file. No path contents enter the error or audit channel.
pub fn validate_inventory(
    files: &[PreparedArtifactFile],
    limits: ArtifactLimits,
) -> crate::Result<()> {
    use std::collections::BTreeSet;
    let invalid = || {
        crate::Error::validation("Artifact inventory exceeds its policy or has invalid bindings")
    };
    if limits.max_total_bytes == 0
        || limits.max_entries == 0
        || limits.max_total_bytes > i64::MAX as u64
        || limits.max_entries > i64::MAX as u64
    {
        return Err(invalid());
    }
    let mut paths = BTreeSet::new();
    let mut entries = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut bytes = 0u64;
    for file in files {
        crate::artifact_transport::ValidatedRelativePath::new(&file.relative_path)
            .map_err(|_| invalid())?;
        if file.version_id <= 0
            || file.max_bytes == 0
            || !ids.insert(file.version_id)
            || !paths.insert(file.relative_path.as_str())
        {
            return Err(invalid());
        }
        bytes = bytes
            .checked_add(file.max_bytes)
            .filter(|value| *value <= limits.max_total_bytes)
            .ok_or_else(invalid)?;
        for (offset, _) in file.relative_path.match_indices('/') {
            entries.insert(&file.relative_path[..offset]);
        }
        entries.insert(file.relative_path.as_str());
        if entries.len() as u64 > limits.max_entries {
            return Err(invalid());
        }
    }
    for file in files {
        for (offset, _) in file.relative_path.match_indices('/') {
            if paths.contains(&file.relative_path[..offset]) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

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
        limits: ArtifactLimits,
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

/// Historical acknowledgement of this exact reserved operation. The receipt
/// does not authorize another write, nor cleanup of the whole staging root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactUploadReceipt {
    pub operation_id: i64,
    pub version_id: i64,
    pub size_bytes: u64,
    pub sha256: Sha256Digest,
    pub publication_receipt_id: i64,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_accounts_shared_directories_and_rejects_ambiguous_tree() {
        let file = |id, path: &str| PreparedArtifactFile {
            version_id: id,
            relative_path: path.into(),
            max_bytes: 8,
        };
        let limits = ArtifactLimits {
            max_total_bytes: 16,
            max_entries: 4,
        };
        let files = [file(1, "files/versions/a"), file(2, "files/versions/b")];
        assert!(validate_inventory(&files, limits).is_ok());
        assert!(validate_inventory(
            &files,
            ArtifactLimits {
                max_entries: 3,
                ..limits
            }
        )
        .is_err());
        assert!(validate_inventory(
            &files,
            ArtifactLimits {
                max_total_bytes: 15,
                ..limits
            }
        )
        .is_err());
        assert!(validate_inventory(&[file(1, "files"), file(2, "files/a")], limits).is_err());
        assert!(validate_inventory(&[file(1, "../private")], limits).is_err());
        assert!(validate_inventory(&[file(1, "files/a"), file(1, "files/b")], limits).is_err());
    }
}
