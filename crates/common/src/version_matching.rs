//! Runtime version constraint matching
//!
//! Provides utilities for parsing and evaluating semver version constraints
//! against available runtime versions. Used by the worker to select the
//! appropriate runtime version when an action or sensor declares a
//! `runtime_version_constraint`.
//!
//! # Constraint Syntax
//!
//! Constraints follow standard semver range syntax:
//!
//! | Constraint      | Meaning                                |
//! |-----------------|----------------------------------------|
//! | `3.12`          | Exactly 3.12.x (any patch)             |
//! | `=3.12.1`       | Exactly 3.12.1                         |
//! | `>=3.12`        | 3.12.0 or newer                        |
//! | `>=3.12,<4.0`   | 3.12.0 or newer, but below 4.0.0       |
//! | `~3.12`         | Compatible with 3.12.x (>=3.12.0, <3.13.0) |
//! | `^3.12`         | Compatible with 3.x.x (>=3.12.0, <4.0.0)  |
//!
//! Multiple constraints can be separated by commas (AND logic).
//!
//! # Lenient Parsing
//!
//! Version strings are parsed leniently to handle real-world formats:
//! - `3.12` → `3.12.0`
//! - `3` → `3.0.0`
//! - `v3.12.1` → `3.12.1` (leading 'v' stripped)
//! - `3.12.1-beta.1` → parsed with pre-release info
//!
//! # Examples
//!
//! ```
//! use attune_common::version_matching::{parse_version, matches_constraint, select_best_version};
//! use attune_common::models::RuntimeVersion;
//!
//! // Simple constraint matching
//! assert!(matches_constraint("3.12.1", ">=3.12").unwrap());
//! assert!(!matches_constraint("3.11.0", ">=3.12").unwrap());
//!
//! // Range constraints
//! assert!(matches_constraint("3.12.5", ">=3.12,<4.0").unwrap());
//! assert!(!matches_constraint("4.0.0", ">=3.12,<4.0").unwrap());
//!
//! // Tilde (patch-level compatibility)
//! assert!(matches_constraint("3.12.5", "~3.12").unwrap());
//! assert!(!matches_constraint("3.13.0", "~3.12").unwrap());
//!
//! // Caret (minor-level compatibility)
//! assert!(matches_constraint("3.15.0", "^3.12").unwrap());
//! assert!(!matches_constraint("4.0.0", "^3.12").unwrap());
//! ```

use semver::{Version, VersionReq};
use tracing::{debug, warn};

use crate::models::RuntimeVersion;

/// Error type for version matching operations.
#[derive(Debug, thiserror::Error)]
pub enum VersionError {
    #[error("Invalid version string '{0}': {1}")]
    InvalidVersion(String, String),

    #[error("Invalid version constraint '{0}': {1}")]
    InvalidConstraint(String, String),
}

/// Result type for version matching operations.
pub type VersionResult<T> = std::result::Result<T, VersionError>;

/// Parse a version string leniently into a [`semver::Version`].
///
/// Handles common real-world formats:
/// - `"3.12"` → `Version { major: 3, minor: 12, patch: 0 }`
/// - `"3"` → `Version { major: 3, minor: 0, patch: 0 }`
/// - `"v3.12.1"` → `Version { major: 3, minor: 12, patch: 1 }`
/// - `"3.12.1"` → `Version { major: 3, minor: 12, patch: 1 }`
pub fn parse_version(version_str: &str) -> VersionResult<Version> {
    let trimmed = version_str.trim();

    // Strip leading 'v' or 'V'
    let stripped = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);

    // Try direct parse first (handles full semver like "3.12.1" and pre-release)
    if let Ok(v) = Version::parse(stripped) {
        return Ok(v);
    }

    // Try adding missing components
    let parts: Vec<&str> = stripped.split('.').collect();
    let padded = match parts.len() {
        1 => format!("{}.0.0", parts[0]),
        2 => format!("{}.{}.0", parts[0], parts[1]),
        _ => {
            // More than 3 parts or other issues — try joining first 3
            if parts.len() >= 3 {
                format!("{}.{}.{}", parts[0], parts[1], parts[2])
            } else {
                stripped.to_string()
            }
        }
    };

    Version::parse(&padded)
        .map_err(|e| VersionError::InvalidVersion(version_str.to_string(), e.to_string()))
}

/// Parse a version constraint string into a [`semver::VersionReq`].
///
/// Handles comma-separated constraints (AND logic) and the standard
/// semver operators: `=`, `>=`, `<=`, `>`, `<`, `~`, `^`.
///
/// If a bare version is given (no operator), it is treated as a
/// compatibility constraint: `"3.12"` becomes `">=3.12.0, <3.13.0"` (tilde behavior).
///
/// Note: The `semver` crate's `VersionReq` natively handles comma-separated
/// constraints and all standard operators.
pub fn parse_constraint(constraint_str: &str) -> VersionResult<VersionReq> {
    let trimmed = constraint_str.trim();

    if trimmed.is_empty() {
        // Empty constraint matches everything
        return Ok(VersionReq::STAR);
    }

    // Preprocess each comma-separated part to handle lenient input.
    // For each part, if it looks like a bare version (no operator prefix),
    // we treat it as a tilde constraint so "3.12" means "~3.12".
    let parts: Vec<String> = trimmed
        .split(',')
        .map(|part| {
            let p = part.trim();
            if p.is_empty() {
                return String::new();
            }

            // Check if the first character is an operator
            let first_char = p.chars().next().unwrap_or(' ');
            if first_char.is_ascii_digit() || first_char == 'v' || first_char == 'V' {
                // Bare version — treat as tilde range (compatible within minor)
                let stripped = p
                    .strip_prefix('v')
                    .or_else(|| p.strip_prefix('V'))
                    .unwrap_or(p);

                // Pad to at least major.minor for tilde semantics
                let dot_count = stripped.chars().filter(|c| *c == '.').count();
                let padded = match dot_count {
                    0 => format!("{}.0", stripped),
                    _ => stripped.to_string(),
                };

                format!("~{}", padded)
            } else {
                // Has operator prefix — normalize version part if needed
                // Find where the version number starts
                let version_start = p.find(|c: char| c.is_ascii_digit()).unwrap_or(p.len());

                let (op, ver) = p.split_at(version_start);
                let ver = ver
                    .strip_prefix('v')
                    .or_else(|| ver.strip_prefix('V'))
                    .unwrap_or(ver);

                // Pad version if needed
                let dot_count = ver.chars().filter(|c| *c == '.').count();
                let padded = match dot_count {
                    0 if !ver.is_empty() => format!("{}.0.0", ver),
                    1 => format!("{}.0", ver),
                    _ => ver.to_string(),
                };

                format!("{}{}", op.trim(), padded)
            }
        })
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        return Ok(VersionReq::STAR);
    }

    let normalized = parts.join(", ");

    VersionReq::parse(&normalized)
        .map_err(|e| VersionError::InvalidConstraint(constraint_str.to_string(), e.to_string()))
}

/// Check whether a version string satisfies a constraint string.
///
/// Returns `true` if the version matches the constraint.
/// Returns an error if either the version or constraint cannot be parsed.
///
/// # Examples
///
/// ```
/// use attune_common::version_matching::matches_constraint;
///
/// assert!(matches_constraint("3.12.1", ">=3.12").unwrap());
/// assert!(!matches_constraint("3.11.0", ">=3.12").unwrap());
/// assert!(matches_constraint("3.12.5", ">=3.12,<4.0").unwrap());
/// ```
pub fn matches_constraint(version_str: &str, constraint_str: &str) -> VersionResult<bool> {
    let version = parse_version(version_str)?;
    let constraint = parse_constraint(constraint_str)?;
    Ok(constraint.matches(&version))
}

/// Select the best matching runtime version from a list of candidates.
///
/// "Best" is defined as the highest version that satisfies the constraint
/// and is marked as available. If no constraint is given, the default version
/// is preferred; if no default exists, the highest available version is returned.
///
/// # Arguments
///
/// * `versions` - All registered versions for a runtime (any order)
/// * `constraint` - Optional version constraint string (e.g., `">=3.12"`)
///
/// # Returns
///
/// The best matching `RuntimeVersion`, or `None` if no version matches.
pub fn select_best_version<'a>(
    versions: &'a [RuntimeVersion],
    constraint: Option<&str>,
) -> Option<&'a RuntimeVersion> {
    if versions.is_empty() {
        return None;
    }

    // Only consider available versions
    let available: Vec<&RuntimeVersion> = versions.iter().filter(|v| v.available).collect();

    if available.is_empty() {
        debug!("No available versions found");
        return None;
    }

    match constraint {
        Some(constraint_str) if !constraint_str.trim().is_empty() => {
            let req = match parse_constraint(constraint_str) {
                Ok(r) => r,
                Err(e) => {
                    warn!("Invalid version constraint '{}': {}", constraint_str, e);
                    return None;
                }
            };

            // Filter to versions that match the constraint, then pick the highest
            let mut matching: Vec<(&RuntimeVersion, Version)> = available
                .iter()
                .filter_map(|rv| match parse_version(&rv.version) {
                    Ok(v) if req.matches(&v) => Some((*rv, v)),
                    Ok(_) => {
                        debug!(
                            "Version {} does not match constraint '{}'",
                            rv.version, constraint_str
                        );
                        None
                    }
                    Err(e) => {
                        warn!("Cannot parse version '{}' for matching: {}", rv.version, e);
                        None
                    }
                })
                .collect();

            if matching.is_empty() {
                debug!(
                    "No available versions match constraint '{}'",
                    constraint_str
                );
                return None;
            }

            // Sort by semver descending — highest version first
            matching.sort_by(|a, b| b.1.cmp(&a.1));

            Some(matching[0].0)
        }

        _ => {
            // No constraint — prefer the default version, else the highest available
            if let Some(default) = available.iter().find(|v| v.is_default) {
                return Some(default);
            }

            // Pick highest available version
            let mut with_parsed: Vec<(&RuntimeVersion, Version)> = available
                .iter()
                .filter_map(|rv| parse_version(&rv.version).ok().map(|v| (*rv, v)))
                .collect();

            with_parsed.sort_by(|a, b| b.1.cmp(&a.1));
            with_parsed.first().map(|(rv, _)| *rv)
        }
    }
}

pub fn runtime_version_matches_worker(
    runtime_version: &RuntimeVersion,
    advertised_version: &str,
) -> bool {
    if advertised_version == runtime_version.version {
        return true;
    }
    let Ok(advertised) = parse_version(advertised_version) else {
        return false;
    };
    let Ok(registered) = parse_version(&runtime_version.version) else {
        return false;
    };
    if runtime_version.version_minor.is_none() {
        advertised.major == registered.major
    } else if runtime_version.version_patch.is_none() {
        advertised.major == registered.major && advertised.minor == registered.minor
    } else {
        advertised == registered
    }
}

/// Extract semver components from a version string.
///
/// Returns `(major, minor, patch)` as `Option<i32>` values.
/// Useful for populating the `version_major`, `version_minor`, `version_patch`
/// columns in the `runtime_version` table.
pub fn extract_version_components(version_str: &str) -> (Option<i32>, Option<i32>, Option<i32>) {
    match parse_version(version_str) {
        Ok(v) => {
            let stripped = version_str
                .trim()
                .strip_prefix(['v', 'V'])
                .unwrap_or(version_str.trim());
            let component_count = stripped
                .split(['-', '+'])
                .next()
                .map(|core| core.split('.').count())
                .unwrap_or(0);
            (
                i32::try_from(v.major).ok(),
                (component_count >= 2)
                    .then(|| i32::try_from(v.minor).ok())
                    .flatten(),
                (component_count >= 3)
                    .then(|| i32::try_from(v.patch).ok())
                    .flatten(),
            )
        }
        Err(_) => (None, None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ========================================================================
    // parse_version tests
    // ========================================================================

    #[test]
    fn test_parse_version_full() {
        let v = parse_version("3.12.1").unwrap();
        assert_eq!(v, Version::new(3, 12, 1));
    }

    #[test]
    fn test_parse_version_two_parts() {
        let v = parse_version("3.12").unwrap();
        assert_eq!(v, Version::new(3, 12, 0));
    }

    #[test]
    fn test_parse_version_one_part() {
        let v = parse_version("3").unwrap();
        assert_eq!(v, Version::new(3, 0, 0));
    }

    #[test]
    fn test_parse_version_leading_v() {
        let v = parse_version("v3.12.1").unwrap();
        assert_eq!(v, Version::new(3, 12, 1));
    }

    #[test]
    fn test_parse_version_leading_v_uppercase() {
        let v = parse_version("V20.11.0").unwrap();
        assert_eq!(v, Version::new(20, 11, 0));
    }

    #[test]
    fn test_parse_version_with_whitespace() {
        let v = parse_version("  3.12.1  ").unwrap();
        assert_eq!(v, Version::new(3, 12, 1));
    }

    #[test]
    fn test_parse_version_invalid() {
        assert!(parse_version("not-a-version").is_err());
    }

    // ========================================================================
    // parse_constraint tests
    // ========================================================================

    #[test]
    fn test_parse_constraint_gte() {
        let req = parse_constraint(">=3.12").unwrap();
        assert!(req.matches(&Version::new(3, 12, 0)));
        assert!(req.matches(&Version::new(3, 13, 0)));
        assert!(req.matches(&Version::new(4, 0, 0)));
        assert!(!req.matches(&Version::new(3, 11, 9)));
    }

    #[test]
    fn test_parse_constraint_exact_with_eq() {
        let req = parse_constraint("=3.12.1").unwrap();
        assert!(req.matches(&Version::new(3, 12, 1)));
        assert!(!req.matches(&Version::new(3, 12, 2)));
    }

    #[test]
    fn test_parse_constraint_bare_version() {
        // Bare "3.12" is treated as ~3.12 → >=3.12.0, <3.13.0
        let req = parse_constraint("3.12").unwrap();
        assert!(req.matches(&Version::new(3, 12, 0)));
        assert!(req.matches(&Version::new(3, 12, 9)));
        assert!(!req.matches(&Version::new(3, 13, 0)));
        assert!(!req.matches(&Version::new(3, 11, 0)));
    }

    #[test]
    fn test_parse_constraint_tilde() {
        let req = parse_constraint("~3.12").unwrap();
        assert!(req.matches(&Version::new(3, 12, 0)));
        assert!(req.matches(&Version::new(3, 12, 99)));
        assert!(!req.matches(&Version::new(3, 13, 0)));
    }

    #[test]
    fn test_parse_constraint_caret() {
        let req = parse_constraint("^3.12").unwrap();
        assert!(req.matches(&Version::new(3, 12, 0)));
        assert!(req.matches(&Version::new(3, 99, 0)));
        assert!(!req.matches(&Version::new(4, 0, 0)));
    }

    #[test]
    fn test_parse_constraint_range() {
        let req = parse_constraint(">=3.12,<4.0").unwrap();
        assert!(req.matches(&Version::new(3, 12, 0)));
        assert!(req.matches(&Version::new(3, 99, 0)));
        assert!(!req.matches(&Version::new(4, 0, 0)));
        assert!(!req.matches(&Version::new(3, 11, 0)));
    }

    #[test]
    fn test_parse_constraint_empty() {
        let req = parse_constraint("").unwrap();
        assert!(req.matches(&Version::new(0, 0, 1)));
        assert!(req.matches(&Version::new(999, 0, 0)));
    }

    #[test]
    fn test_parse_constraint_lt() {
        let req = parse_constraint("<4.0").unwrap();
        assert!(req.matches(&Version::new(3, 99, 99)));
        assert!(!req.matches(&Version::new(4, 0, 0)));
    }

    #[test]
    fn test_parse_constraint_lte() {
        let req = parse_constraint("<=3.12").unwrap();
        assert!(req.matches(&Version::new(3, 12, 0)));
        // Note: semver <=3.12.0 means exactly ≤3.12.0
        assert!(!req.matches(&Version::new(3, 12, 1)));
        assert!(!req.matches(&Version::new(3, 13, 0)));
    }

    #[test]
    fn test_parse_constraint_gt() {
        let req = parse_constraint(">3.12").unwrap();
        assert!(!req.matches(&Version::new(3, 12, 0)));
        assert!(req.matches(&Version::new(3, 12, 1)));
        assert!(req.matches(&Version::new(3, 13, 0)));
    }

    // ========================================================================
    // matches_constraint tests
    // ========================================================================

    #[test]
    fn test_matches_constraint_basic() {
        assert!(matches_constraint("3.12.1", ">=3.12").unwrap());
        assert!(!matches_constraint("3.11.0", ">=3.12").unwrap());
    }

    #[test]
    fn test_matches_constraint_range() {
        assert!(matches_constraint("3.12.5", ">=3.12,<4.0").unwrap());
        assert!(!matches_constraint("4.0.0", ">=3.12,<4.0").unwrap());
    }

    #[test]
    fn test_matches_constraint_tilde() {
        assert!(matches_constraint("3.12.5", "~3.12").unwrap());
        assert!(!matches_constraint("3.13.0", "~3.12").unwrap());
    }

    #[test]
    fn test_matches_constraint_caret() {
        assert!(matches_constraint("3.15.0", "^3.12").unwrap());
        assert!(!matches_constraint("4.0.0", "^3.12").unwrap());
    }

    #[test]
    fn test_matches_constraint_node_versions() {
        assert!(matches_constraint("20.11.0", ">=18").unwrap());
        assert!(matches_constraint("18.0.0", ">=18").unwrap());
        assert!(!matches_constraint("16.20.0", ">=18").unwrap());
    }

    // ========================================================================
    // extract_version_components tests
    // ========================================================================

    #[test]
    fn test_extract_components_full() {
        let (maj, min, pat) = extract_version_components("3.12.1");
        assert_eq!(maj, Some(3));
        assert_eq!(min, Some(12));
        assert_eq!(pat, Some(1));
    }

    #[test]
    fn test_extract_components_partial() {
        let (maj, min, pat) = extract_version_components("20.11");
        assert_eq!(maj, Some(20));
        assert_eq!(min, Some(11));
        assert_eq!(pat, None);

        let (maj, min, pat) = extract_version_components("20");
        assert_eq!(maj, Some(20));
        assert_eq!(min, None);
        assert_eq!(pat, None);
    }

    #[test]
    fn test_extract_components_invalid() {
        let (maj, min, pat) = extract_version_components("not-a-version");
        assert_eq!(maj, None);
        assert_eq!(min, None);
        assert_eq!(pat, None);
    }

    // ========================================================================
    // select_best_version tests
    // ========================================================================

    fn make_version(
        id: i64,
        runtime: i64,
        version: &str,
        is_default: bool,
        available: bool,
    ) -> RuntimeVersion {
        let (major, minor, patch) = extract_version_components(version);
        RuntimeVersion {
            id,
            runtime,
            runtime_ref: "core.python".to_string(),
            version: version.to_string(),
            version_major: major,
            version_minor: minor,
            version_patch: patch,
            execution_config: json!({}),
            distributions: json!({}),
            is_default,
            available,
            verified_at: None,
            meta: json!({}),
            created: chrono::Utc::now(),
            updated: chrono::Utc::now(),
        }
    }

    #[test]
    fn worker_version_matching_does_not_substitute_a_different_patch() {
        let registered = make_version(1, 1, "3.12.9", false, true);
        assert!(!runtime_version_matches_worker(&registered, "3.12.1"));
        assert!(runtime_version_matches_worker(&registered, "3.12.9"));
    }

    #[test]
    fn worker_version_matching_allows_patch_for_generic_minor_registration() {
        let registered = make_version(1, 1, "3.12", false, true);
        assert!(runtime_version_matches_worker(&registered, "3.12.7"));
    }

    #[test]
    fn worker_version_matching_allows_minor_for_generic_major_registration() {
        let registered = make_version(1, 1, "20", false, true);
        assert!(runtime_version_matches_worker(&registered, "20.11.1"));
        assert!(!runtime_version_matches_worker(&registered, "21.0.0"));
    }

    #[test]
    fn test_select_best_no_constraint_prefers_default() {
        let versions = vec![
            make_version(1, 1, "3.11.0", false, true),
            make_version(2, 1, "3.12.0", true, true), // default
            make_version(3, 1, "3.14.0", false, true),
        ];

        let best = select_best_version(&versions, None).unwrap();
        assert_eq!(best.id, 2); // default version
    }

    #[test]
    fn test_select_best_no_constraint_no_default_picks_highest() {
        let versions = vec![
            make_version(1, 1, "3.11.0", false, true),
            make_version(2, 1, "3.12.0", false, true),
            make_version(3, 1, "3.14.0", false, true),
        ];

        let best = select_best_version(&versions, None).unwrap();
        assert_eq!(best.id, 3); // highest version
    }

    #[test]
    fn test_select_best_with_constraint() {
        let versions = vec![
            make_version(1, 1, "3.11.0", false, true),
            make_version(2, 1, "3.12.0", false, true),
            make_version(3, 1, "3.14.0", false, true),
        ];

        // >=3.12,<3.14 should pick 3.12.0 (3.14.0 is excluded)
        let best = select_best_version(&versions, Some(">=3.12,<3.14")).unwrap();
        assert_eq!(best.id, 2);
    }

    #[test]
    fn test_select_best_with_constraint_picks_highest_match() {
        let versions = vec![
            make_version(1, 1, "3.11.0", false, true),
            make_version(2, 1, "3.12.0", false, true),
            make_version(3, 1, "3.12.5", false, true),
            make_version(4, 1, "3.13.0", false, true),
        ];

        // ~3.12 → >=3.12.0, <3.13.0 → should pick 3.12.5
        let best = select_best_version(&versions, Some("~3.12")).unwrap();
        assert_eq!(best.id, 3);
    }

    #[test]
    fn test_select_best_skips_unavailable() {
        let versions = vec![
            make_version(1, 1, "3.12.0", false, true),
            make_version(2, 1, "3.14.0", false, false), // not available
        ];

        let best = select_best_version(&versions, Some(">=3.12")).unwrap();
        assert_eq!(best.id, 1); // 3.14 is unavailable
    }

    #[test]
    fn test_select_best_no_match() {
        let versions = vec![
            make_version(1, 1, "3.11.0", false, true),
            make_version(2, 1, "3.12.0", false, true),
        ];

        let best = select_best_version(&versions, Some(">=4.0"));
        assert!(best.is_none());
    }

    #[test]
    fn test_select_best_empty_versions() {
        let versions: Vec<RuntimeVersion> = vec![];
        assert!(select_best_version(&versions, None).is_none());
    }

    #[test]
    fn test_select_best_all_unavailable() {
        let versions = vec![
            make_version(1, 1, "3.12.0", false, false),
            make_version(2, 1, "3.14.0", false, false),
        ];

        assert!(select_best_version(&versions, None).is_none());
    }
}
