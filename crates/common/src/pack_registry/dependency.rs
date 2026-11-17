//! Pack Dependency Validation
//!
//! This module provides functionality for validating pack dependencies including:
//! - Runtime dependencies (Python, Node.js, shell versions)
//! - Pack dependencies with version constraints
//! - Semver version parsing and comparison

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Dependency validation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyValidation {
    /// Whether all dependencies are satisfied
    pub valid: bool,

    /// Runtime dependencies validation
    pub runtime_deps: Vec<RuntimeDepValidation>,

    /// Pack dependencies validation
    pub pack_deps: Vec<PackDepValidation>,

    /// Warnings (non-blocking issues)
    pub warnings: Vec<String>,

    /// Errors (blocking issues)
    pub errors: Vec<String>,
}

/// Runtime dependency validation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeDepValidation {
    /// Runtime name (e.g., "python3", "nodejs")
    pub runtime: String,

    /// Required version constraint (e.g., ">=3.8", "^14.0.0")
    pub required_version: Option<String>,

    /// Detected version on system
    pub detected_version: Option<String>,

    /// Whether requirement is satisfied
    pub satisfied: bool,

    /// Error message if not satisfied
    pub error: Option<String>,
}

/// Pack dependency validation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackDepValidation {
    /// Pack reference
    pub pack_ref: String,

    /// Required version constraint (e.g., "1.0.0", ">=1.2.0", "^2.0.0")
    pub required_version: String,

    /// Installed version (if pack is installed)
    pub installed_version: Option<String>,

    /// Whether requirement is satisfied
    pub satisfied: bool,

    /// Error message if not satisfied
    pub error: Option<String>,
}

/// Dependency validator
pub struct DependencyValidator;

impl DependencyValidator {
    /// Create a new dependency validator
    pub fn new() -> Self {
        Self
    }

    /// Validate all dependencies for a pack
    pub async fn validate(
        &mut self,
        runtime_deps: &[String],
        pack_deps: &[(String, String)],
        installed_packs: &HashMap<String, String>,
    ) -> Result<DependencyValidation> {
        let mut validation = DependencyValidation {
            valid: true,
            runtime_deps: Vec::new(),
            pack_deps: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
        };

        // Validate runtime dependencies
        for runtime_dep in runtime_deps {
            let result = self.validate_runtime_dep(runtime_dep).await?;
            if !result.satisfied {
                validation.valid = false;
                if let Some(error) = &result.error {
                    validation.errors.push(error.clone());
                }
            }
            validation.runtime_deps.push(result);
        }

        // Validate pack dependencies
        for (pack_ref, version_constraint) in pack_deps {
            let result = self.validate_pack_dep(pack_ref, version_constraint, installed_packs)?;
            if !result.satisfied {
                validation.valid = false;
                if let Some(error) = &result.error {
                    validation.errors.push(error.clone());
                }
            }
            validation.pack_deps.push(result);
        }

        Ok(validation)
    }

    /// Validate a single runtime dependency
    async fn validate_runtime_dep(&mut self, runtime_dep: &str) -> Result<RuntimeDepValidation> {
        // Parse runtime dependency (e.g., "python3>=3.8", "nodejs^14.0.0")
        let (runtime, version_constraint) = parse_runtime_dep(runtime_dep)?;

        Ok(RuntimeDepValidation {
            runtime,
            required_version: version_constraint,
            detected_version: None,
            satisfied: false,
            error: Some("Legacy local runtime dependency probing is unsupported; use registered worker runtime capabilities".to_string()),
        })
    }

    /// Validate a single pack dependency
    fn validate_pack_dep(
        &self,
        pack_ref: &str,
        version_constraint: &str,
        installed_packs: &HashMap<String, String>,
    ) -> Result<PackDepValidation> {
        let installed_version = installed_packs.get(pack_ref).cloned();

        let satisfied = if let Some(ref installed) = installed_version {
            match_version_constraint(installed, version_constraint)?
        } else {
            false
        };

        let error = if !satisfied {
            if let Some(ref installed) = installed_version {
                Some(format!(
                    "Pack '{}' version {} does not satisfy constraint '{}'",
                    pack_ref, installed, version_constraint
                ))
            } else {
                Some(format!("Required pack '{}' is not installed", pack_ref))
            }
        } else {
            None
        };

        Ok(PackDepValidation {
            pack_ref: pack_ref.to_string(),
            required_version: version_constraint.to_string(),
            installed_version,
            satisfied,
            error,
        })
    }
}

impl Default for DependencyValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse runtime dependency string (e.g., "python3>=3.8" -> ("python3", Some(">=3.8")))
fn parse_runtime_dep(runtime_dep: &str) -> Result<(String, Option<String>)> {
    // Find operator position
    let operators = [">=", "<=", "^", "~", ">", "<", "="];

    for op in &operators {
        if let Some(pos) = runtime_dep.find(op) {
            let runtime = runtime_dep[..pos].trim().to_string();
            let version = runtime_dep[pos..].trim().to_string();
            return Ok((runtime, Some(version)));
        }
    }

    // No version constraint
    Ok((runtime_dep.trim().to_string(), None))
}

/// Match version against constraint
fn match_version_constraint(version: &str, constraint: &str) -> Result<bool> {
    // Handle wildcard constraint
    if constraint == "*" {
        return Ok(true);
    }

    // Parse constraint
    if let Some(stripped) = constraint.strip_prefix(">=") {
        let required = stripped.trim();
        Ok(compare_versions(version, required)? >= 0)
    } else if let Some(stripped) = constraint.strip_prefix("<=") {
        let required = stripped.trim();
        Ok(compare_versions(version, required)? <= 0)
    } else if let Some(stripped) = constraint.strip_prefix('>') {
        let required = stripped.trim();
        Ok(compare_versions(version, required)? > 0)
    } else if let Some(stripped) = constraint.strip_prefix('<') {
        let required = stripped.trim();
        Ok(compare_versions(version, required)? < 0)
    } else if let Some(stripped) = constraint.strip_prefix('=') {
        let required = stripped.trim();
        Ok(compare_versions(version, required)? == 0)
    } else if let Some(stripped) = constraint.strip_prefix('^') {
        // Caret: Compatible with version (major.minor.patch)
        // ^1.2.3 := >=1.2.3 <2.0.0
        let required = stripped.trim();
        match_caret_constraint(version, required)
    } else if let Some(stripped) = constraint.strip_prefix('~') {
        // Tilde: Approximately equivalent to version
        // ~1.2.3 := >=1.2.3 <1.3.0
        let required = stripped.trim();
        match_tilde_constraint(version, required)
    } else {
        // Exact match
        Ok(compare_versions(version, constraint)? == 0)
    }
}

/// Compare two semver versions (-1: v1 < v2, 0: v1 == v2, 1: v1 > v2)
fn compare_versions(v1: &str, v2: &str) -> Result<i32> {
    let parts1 = parse_version(v1)?;
    let parts2 = parse_version(v2)?;

    for i in 0..3 {
        if parts1[i] < parts2[i] {
            return Ok(-1);
        } else if parts1[i] > parts2[i] {
            return Ok(1);
        }
    }

    Ok(0)
}

/// Parse version string to [major, minor, patch]
fn parse_version(version: &str) -> Result<[u32; 3]> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.is_empty() {
        return Err(Error::validation(format!("Invalid version: {}", version)));
    }

    let mut result = [0u32; 3];
    for (i, part) in parts.iter().enumerate().take(3) {
        result[i] = part
            .parse()
            .map_err(|_| Error::validation(format!("Invalid version number: {}", part)))?;
    }

    Ok(result)
}

/// Match caret constraint (^1.2.3 := >=1.2.3 <2.0.0)
fn match_caret_constraint(version: &str, required: &str) -> Result<bool> {
    let v_parts = parse_version(version)?;
    let r_parts = parse_version(required)?;

    // Must be >= required version
    if compare_versions(version, required)? < 0 {
        return Ok(false);
    }

    // Must have same major version (if major > 0)
    if r_parts[0] > 0 {
        Ok(v_parts[0] == r_parts[0])
    } else if r_parts[1] > 0 {
        // If major is 0, must have same minor version
        Ok(v_parts[0] == 0 && v_parts[1] == r_parts[1])
    } else {
        // If major and minor are 0, must have same patch version
        Ok(v_parts[0] == 0 && v_parts[1] == 0 && v_parts[2] == r_parts[2])
    }
}

/// Match tilde constraint (~1.2.3 := >=1.2.3 <1.3.0)
fn match_tilde_constraint(version: &str, required: &str) -> Result<bool> {
    let v_parts = parse_version(version)?;
    let r_parts = parse_version(required)?;

    // Must be >= required version
    if compare_versions(version, required)? < 0 {
        return Ok(false);
    }

    // Must have same major and minor version
    Ok(v_parts[0] == r_parts[0] && v_parts[1] == r_parts[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_runtime_dep() {
        let (runtime, version) = parse_runtime_dep("python3>=3.8").unwrap();
        assert_eq!(runtime, "python3");
        assert_eq!(version, Some(">=3.8".to_string()));

        let (runtime, version) = parse_runtime_dep("nodejs").unwrap();
        assert_eq!(runtime, "nodejs");
        assert_eq!(version, None);

        let (runtime, version) = parse_runtime_dep("python3 >= 3.8").unwrap();
        assert_eq!(runtime, "python3");
        assert_eq!(version, Some(">= 3.8".to_string()));
    }

    #[test]
    fn test_parse_version() {
        assert_eq!(parse_version("1.2.3").unwrap(), [1, 2, 3]);
        assert_eq!(parse_version("1.0.0").unwrap(), [1, 0, 0]);
        assert_eq!(parse_version("0.1").unwrap(), [0, 1, 0]);
        assert_eq!(parse_version("2").unwrap(), [2, 0, 0]);
    }

    #[test]
    fn test_compare_versions() {
        assert_eq!(compare_versions("1.2.3", "1.2.3").unwrap(), 0);
        assert_eq!(compare_versions("1.2.3", "1.2.4").unwrap(), -1);
        assert_eq!(compare_versions("1.3.0", "1.2.9").unwrap(), 1);
        assert_eq!(compare_versions("2.0.0", "1.9.9").unwrap(), 1);
    }

    #[test]
    fn test_match_version_constraint() {
        assert!(match_version_constraint("1.2.3", ">=1.2.0").unwrap());
        assert!(match_version_constraint("1.2.3", "<=1.3.0").unwrap());
        assert!(match_version_constraint("1.2.3", ">1.2.2").unwrap());
        assert!(match_version_constraint("1.2.3", "<1.2.4").unwrap());
        assert!(match_version_constraint("1.2.3", "=1.2.3").unwrap());
        assert!(match_version_constraint("1.2.3", "1.2.3").unwrap());

        assert!(!match_version_constraint("1.2.3", ">=1.2.4").unwrap());
        assert!(!match_version_constraint("1.2.3", "<1.2.3").unwrap());
    }

    #[test]
    fn test_match_caret_constraint() {
        // ^1.2.3 := >=1.2.3 <2.0.0
        assert!(match_caret_constraint("1.2.3", "1.2.3").unwrap());
        assert!(match_caret_constraint("1.2.4", "1.2.3").unwrap());
        assert!(match_caret_constraint("1.9.9", "1.2.3").unwrap());
        assert!(!match_caret_constraint("2.0.0", "1.2.3").unwrap());
        assert!(!match_caret_constraint("1.2.2", "1.2.3").unwrap());

        // ^0.2.3 := >=0.2.3 <0.3.0
        assert!(match_caret_constraint("0.2.3", "0.2.3").unwrap());
        assert!(match_caret_constraint("0.2.9", "0.2.3").unwrap());
        assert!(!match_caret_constraint("0.3.0", "0.2.3").unwrap());

        // ^0.0.3 := =0.0.3
        assert!(match_caret_constraint("0.0.3", "0.0.3").unwrap());
        assert!(!match_caret_constraint("0.0.4", "0.0.3").unwrap());
    }

    #[test]
    fn test_match_tilde_constraint() {
        // ~1.2.3 := >=1.2.3 <1.3.0
        assert!(match_tilde_constraint("1.2.3", "1.2.3").unwrap());
        assert!(match_tilde_constraint("1.2.9", "1.2.3").unwrap());
        assert!(!match_tilde_constraint("1.3.0", "1.2.3").unwrap());
        assert!(!match_tilde_constraint("1.2.2", "1.2.3").unwrap());
    }

    #[tokio::test]
    async fn local_runtime_dependencies_fail_without_executing_host_programs() {
        let result = DependencyValidator::new()
            .validate_runtime_dep("shell")
            .await
            .unwrap();
        assert!(!result.satisfied);
        assert!(result
            .error
            .unwrap()
            .contains("local runtime dependency probing is unsupported"));
    }
}
