//! Transport mode auto-detection.
//!
//! The API writes a sentinel file to the artifacts directory on startup.
//! Workers and sensors check for this file to determine whether they share
//! the same volume as the API (fast path) or need to use HTTP transport.

use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{debug, info};

/// Configured or detected transport mode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportMode {
    /// Use shared filesystem (fast path).
    Volume,
    /// Use HTTP API for file transfer (remote workers).
    Api,
    /// Auto-detect based on sentinel file presence.
    #[default]
    Auto,
}

impl std::fmt::Display for TransportMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Volume => write!(f, "volume"),
            Self::Api => write!(f, "api"),
            Self::Auto => write!(f, "auto"),
        }
    }
}

/// Name of the sentinel file the API writes into the artifacts directory.
pub const SENTINEL_FILENAME: &str = ".attune-api-sentinel";

/// Content written into the sentinel file by the API on startup.
#[derive(Debug, Serialize, Deserialize)]
pub struct SentinelInfo {
    pub api_url: String,
    pub instance_id: String,
    pub timestamp: String,
}

/// Detect whether the artifacts directory is a shared volume with the API.
///
/// Returns [`TransportMode::Volume`] if the sentinel file is present and readable,
/// [`TransportMode::Api`] otherwise.
pub fn detect_transport_mode(artifacts_dir: &str) -> TransportMode {
    let sentinel_path = Path::new(artifacts_dir).join(SENTINEL_FILENAME); // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- artifacts_dir is a trusted deployment root and the sentinel filename is constant.
    debug!("Checking for API sentinel at {}", sentinel_path.display());

    if sentinel_path.exists() {
        match std::fs::read_to_string(&sentinel_path) {
            Ok(content) => {
                if let Ok(info) = serde_json::from_str::<SentinelInfo>(&content) {
                    info!(
                        api_url = %info.api_url,
                        "Shared volume detected (API sentinel found) — using volume transport"
                    );
                    TransportMode::Volume
                } else {
                    // Sentinel exists but is malformed — still indicates shared volume
                    info!("Shared volume detected (sentinel present but unparseable) — using volume transport");
                    TransportMode::Volume
                }
            }
            Err(e) => {
                debug!("Cannot read sentinel file: {e} — falling back to API transport");
                TransportMode::Api
            }
        }
    } else {
        info!(
            "No API sentinel found at {} — using API transport",
            sentinel_path.display()
        );
        TransportMode::Api
    }
}

/// Write the sentinel file from the API service.
#[allow(dead_code)] // Used by API service (crates/api)
pub fn write_sentinel(artifacts_dir: &str, api_url: &str) -> std::io::Result<()> {
    let sentinel_path = Path::new(artifacts_dir).join(SENTINEL_FILENAME); // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- artifacts_dir is a trusted deployment root and the sentinel filename is constant.

    // Ensure directory exists
    if let Some(parent) = sentinel_path.parent() {
        crate::utils::create_shared_dir_all_sync(parent)?;
    }

    let info = SentinelInfo {
        api_url: api_url.to_string(),
        instance_id: uuid::Uuid::new_v4().to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    let content = serde_json::to_string_pretty(&info).map_err(std::io::Error::other)?;
    std::fs::write(&sentinel_path, content)?;

    info!(
        path = %sentinel_path.display(),
        "API sentinel file written"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_detect_no_sentinel() {
        let tmp = TempDir::new().unwrap();
        let mode = detect_transport_mode(tmp.path().to_str().unwrap());
        assert_eq!(mode, TransportMode::Api);
    }

    #[test]
    fn test_detect_with_sentinel() {
        let tmp = TempDir::new().unwrap();
        write_sentinel(tmp.path().to_str().unwrap(), "http://localhost:8080").unwrap();
        let mode = detect_transport_mode(tmp.path().to_str().unwrap());
        assert_eq!(mode, TransportMode::Volume);
    }

    #[test]
    fn test_detect_malformed_sentinel() {
        let tmp = TempDir::new().unwrap();
        let sentinel = tmp.path().join(SENTINEL_FILENAME);
        std::fs::write(sentinel, "not json").unwrap();
        let mode = detect_transport_mode(tmp.path().to_str().unwrap());
        assert_eq!(mode, TransportMode::Volume); // still volume if file exists
    }

    #[test]
    fn test_transport_mode_serde() {
        let json = serde_json::to_string(&TransportMode::Api).unwrap();
        assert_eq!(json, "\"api\"");
        let parsed: TransportMode = serde_json::from_str("\"volume\"").unwrap();
        assert_eq!(parsed, TransportMode::Volume);
        let parsed: TransportMode = serde_json::from_str("\"auto\"").unwrap();
        assert_eq!(parsed, TransportMode::Auto);
    }
}
