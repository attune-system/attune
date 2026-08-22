//! Pack registry module for managing pack indices and installation sources
//!
//! This module provides data structures and functionality for:
//! - Pack registry index files (JSON format)
//! - Pack installation sources (git, archive, local)
//! - Registry client for fetching and parsing indices
//! - Pack search and discovery

pub mod archive;
pub mod client;
pub mod dependency;
pub mod installer;
pub mod loader;
pub mod outbound;
pub mod storage;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

// Re-export client, installer, loader, storage, and dependency utilities
pub use archive::{
    extract_archive, extract_tar, extract_tar_archive, extract_zip, SafeExtractionLimits,
};
pub use client::{validate_registry_headers, RegistryClient};
pub use dependency::{
    DependencyValidation, DependencyValidator, PackDepValidation, RuntimeDepValidation,
};
pub use installer::{
    ChecksumSubject, InstalledPack, PackInstaller, PackSource, RegistryPackIdentity,
};
pub use loader::{PackComponentLoader, PackLoadResult};
pub use outbound::{validate_remote_pack_url, OutboundUrlPolicy, ValidatedUrl};
pub use storage::{
    calculate_directory_checksum, calculate_file_checksum, verify_checksum, PackReplacement,
    PackStorage,
};

/// Pack registry index file
///
/// This is the top-level structure of a pack registry index file (typically index.json).
/// It contains metadata about the registry and a list of available packs.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PackIndex {
    /// Human-readable registry name
    pub registry_name: String,

    /// Registry homepage URL
    pub registry_url: String,

    /// Index format version (semantic versioning)
    pub version: String,

    /// ISO 8601 timestamp of last update
    pub last_updated: String,

    /// List of available packs
    pub packs: Vec<PackIndexEntry>,
}

/// Pack entry in a registry index
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PackIndexEntry {
    /// Unique pack identifier (matches pack.yaml ref)
    #[serde(rename = "ref")]
    pub pack_ref: String,

    /// Human-readable pack name
    pub label: String,

    /// Brief pack description
    pub description: String,

    /// Brief use-case summary for browsing/install decisions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_case: Option<String>,

    /// Semantic version (latest available)
    pub version: String,

    /// Pack author/maintainer name
    pub author: String,

    /// Contact email
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    /// Pack homepage URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,

    /// Source repository URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,

    /// SPDX license identifier
    pub license: String,

    /// Searchable keywords/tags
    pub keywords: Vec<String>,

    /// Required runtimes (python3, nodejs, shell)
    pub runtime_deps: Vec<String>,

    /// Available installation sources
    pub install_sources: Vec<InstallSource>,

    /// Pack components summary
    pub contents: PackContents,

    /// Pack dependencies
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<PackDependencies>,

    /// Additional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<PackMeta>,
}

/// Installation source for a pack
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum InstallSource {
    /// Git repository source
    Git {
        /// Git repository URL
        url: String,

        /// Git ref (tag, branch, commit)
        #[serde(
            rename = "ref",
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_required_git_ref"
        )]
        #[schema(required = true, nullable = false)]
        git_ref: Option<String>,

        /// Checksum in format "algorithm:hash"
        checksum: String,
    },

    /// Archive (zip, tar.gz) source
    Archive {
        /// Archive URL
        url: String,

        /// Checksum in format "algorithm:hash"
        checksum: String,
    },
}

impl InstallSource {
    /// Get the URL for this install source
    pub fn url(&self) -> &str {
        match self {
            InstallSource::Git { url, .. } => url,
            InstallSource::Archive { url, .. } => url,
        }
    }

    /// Get the checksum for this install source
    pub fn checksum(&self) -> &str {
        match self {
            InstallSource::Git { checksum, .. } => checksum,
            InstallSource::Archive { checksum, .. } => checksum,
        }
    }

    /// Get the source type as a string
    pub fn source_type(&self) -> &'static str {
        match self {
            InstallSource::Git { .. } => "git",
            InstallSource::Archive { .. } => "archive",
        }
    }
}

/// Pack contents summary
#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PackContents {
    /// List of actions
    pub actions: Vec<ComponentSummary>,

    /// List of sensors
    pub sensors: Vec<ComponentSummary>,

    /// List of triggers
    pub triggers: Vec<ComponentSummary>,

    /// List of bundled rules
    pub rules: Vec<ComponentSummary>,

    /// List of bundled workflows
    pub workflows: Vec<ComponentSummary>,
}

/// Component summary (action, sensor, trigger, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentSummary {
    /// Component name
    pub name: String,

    /// Brief description
    pub description: String,
}

/// Pack dependencies
#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PackDependencies {
    /// Attune version requirement (semver)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attune_version: Option<String>,

    /// Python version requirement
    #[serde(skip_serializing_if = "Option::is_none")]
    pub python_version: Option<String>,

    /// Node.js version requirement
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodejs_version: Option<String>,

    /// Pack dependencies (format: "ref@version")
    #[serde(default)]
    pub packs: Vec<String>,
}

/// Additional pack metadata
#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
pub struct PackMeta {
    /// Download count
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloads: Option<u64>,

    /// Star/rating count
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stars: Option<u64>,

    /// Tested Attune versions
    #[serde(default)]
    pub tested_attune_versions: Vec<String>,

    /// Additional custom fields
    #[serde(flatten)]
    #[schema(value_type = Object)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

fn deserialize_required_git_ref<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

/// Checksum with algorithm
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checksum {
    /// Hash algorithm (sha256, sha512, etc.)
    pub algorithm: String,

    /// Hash value (hex string)
    pub hash: String,
}

impl Checksum {
    /// Parse a checksum string in format "algorithm:hash"
    pub fn parse(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(format!(
                "Invalid checksum format: {}. Expected 'algorithm:hash'",
                s
            ));
        }

        let algorithm = parts[0].to_lowercase();
        let hash = parts[1].to_lowercase();

        // Validate algorithm
        match algorithm.as_str() {
            "sha256" | "sha512" | "sha1" | "md5" => {}
            _ => return Err(format!("Unsupported hash algorithm: {}", algorithm)),
        }

        // Basic validation of hash format (hex string)
        if !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "Invalid hash format: {}. Must be hexadecimal",
                hash
            ));
        }

        Ok(Self { algorithm, hash })
    }

    /// Parse the checksum format required by remote registry indices.
    pub(crate) fn parse_registry_sha256(s: &str) -> Result<Self, String> {
        let hash = s.strip_prefix("sha256:").ok_or_else(|| {
            "Registry checksums must use sha256:<64 lowercase hex characters>".to_string()
        })?;
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(
                "Registry checksums must use sha256:<64 lowercase hex characters>".to_string(),
            );
        }

        Ok(Self {
            algorithm: "sha256".to_string(),
            hash: hash.to_string(),
        })
    }
}

impl std::fmt::Display for Checksum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.algorithm, self.hash)
    }
}

impl std::str::FromStr for Checksum {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checksum_parse() {
        let checksum = Checksum::parse("sha256:abc123def456").unwrap();
        assert_eq!(checksum.algorithm, "sha256");
        assert_eq!(checksum.hash, "abc123def456");

        let checksum = Checksum::parse("SHA256:ABC123DEF456").unwrap();
        assert_eq!(checksum.algorithm, "sha256");
        assert_eq!(checksum.hash, "abc123def456");
    }

    #[test]
    fn test_checksum_parse_invalid() {
        assert!(Checksum::parse("invalid").is_err());
        assert!(Checksum::parse("sha256").is_err());
        assert!(Checksum::parse("sha256:xyz").is_err()); // non-hex
        assert!(Checksum::parse("unknown:abc123").is_err()); // unknown algorithm
    }

    #[test]
    fn test_checksum_to_string() {
        let checksum = Checksum {
            algorithm: "sha256".to_string(),
            hash: "abc123".to_string(),
        };
        assert_eq!(checksum.to_string(), "sha256:abc123");
    }

    #[test]
    fn test_install_source_getters() {
        let git_source = InstallSource::Git {
            url: "https://github.com/example/pack".to_string(),
            git_ref: Some("v1.0.0".to_string()),
            checksum: "sha256:abc123".to_string(),
        };

        assert_eq!(git_source.url(), "https://github.com/example/pack");
        assert_eq!(git_source.checksum(), "sha256:abc123");
        assert_eq!(git_source.source_type(), "git");

        let archive_source = InstallSource::Archive {
            url: "https://example.com/pack.zip".to_string(),
            checksum: "sha256:def456".to_string(),
        };

        assert_eq!(archive_source.url(), "https://example.com/pack.zip");
        assert_eq!(archive_source.checksum(), "sha256:def456");
        assert_eq!(archive_source.source_type(), "archive");
    }

    #[test]
    fn test_pack_index_deserialization() {
        let json = r#"{
            "registry_name": "Test Registry",
            "registry_url": "https://registry.example.com",
            "version": "1.0",
            "last_updated": "2024-01-20T12:00:00Z",
            "packs": [
                {
                    "ref": "test-pack",
                    "label": "Test Pack",
                    "description": "A test pack",
                    "version": "1.0.0",
                    "author": "Test Author",
                    "license": "Apache-2.0",
                    "keywords": ["test"],
                    "runtime_deps": ["python3"],
                    "install_sources": [
                        {
                            "type": "git",
                            "url": "https://github.com/example/pack",
                            "ref": "v1.0.0",
                            "checksum": "sha256:abc123"
                        }
                    ],
                    "contents": {
                        "actions": [
                            {
                                "name": "test_action",
                                "description": "Test action"
                            }
                        ],
                        "sensors": [],
                        "triggers": [],
                        "rules": [],
                        "workflows": []
                    }
                }
            ]
        }"#;

        let index: PackIndex = serde_json::from_str(json).unwrap();
        assert_eq!(index.registry_name, "Test Registry");
        assert_eq!(index.packs.len(), 1);
        assert_eq!(index.packs[0].pack_ref, "test-pack");
        assert_eq!(index.packs[0].install_sources.len(), 1);
    }
}
