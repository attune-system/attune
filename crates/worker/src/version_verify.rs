//! Runtime Version Verification
//!
//! At worker startup, this module verifies which runtime versions are actually
//! available on the system by running each version's verification commands
//! (from the `distributions` JSONB column). Versions that pass verification
//! are marked `available = true`; those that fail are marked `available = false`.
//!
//! This ensures the worker has an accurate picture of what it can execute,
//! and `select_best_version()` only considers versions whose interpreters
//! are genuinely present on this particular host/container.

use attune_common::child_process_environment::ChildProcessEnvironment;
use attune_common::repositories::{FindById, List, RuntimeRepository};
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use std::collections::HashMap;
use tracing::{debug, info, warn};

use attune_common::models::RuntimeVersion;
use attune_common::repositories::runtime_version::RuntimeVersionRepository;
use attune_common::runtime_detection::{normalize_runtime_name, runtime_aliases_match_filter};
use attune_common::version_matching::parse_version;

/// Result of verifying runtime versions on this worker.
#[derive(Debug)]
pub struct VersionVerificationResult {
    /// Total number of versions checked.
    pub total_checked: usize,
    /// Number of versions marked as available.
    pub available: usize,
    /// Number of versions marked as unavailable.
    pub unavailable: usize,
    /// Errors encountered during verification (non-fatal).
    pub errors: Vec<String>,
}

pub const RUNTIME_VERSIONS_CAPABILITY_KEY: &str = "runtime_versions";

/// Verify all registered runtime versions and update their `available` flag.
///
/// For each `RuntimeVersion` row in the database:
/// 1. Extract verification commands from `distributions.verification.commands`
/// 2. Run each command (in priority order) until one succeeds
/// 3. Update `available` and `verified_at` in the database
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `runtime_filter` - Optional runtime name filter (from `ATTUNE_WORKER_RUNTIMES`)
pub async fn verify_all_runtime_versions(
    pool: &PgPool,
    runtime_filter: Option<&[String]>,
    child_environment: &ChildProcessEnvironment,
) -> VersionVerificationResult {
    info!("Starting runtime version verification");

    let mut result = VersionVerificationResult {
        total_checked: 0,
        available: 0,
        unavailable: 0,
        errors: Vec::new(),
    };

    // Load all runtime versions
    let versions: Vec<RuntimeVersion> = match RuntimeVersionRepository::list(pool).await {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("Failed to load runtime versions from database: {}", e);
            warn!("{}", msg);
            result.errors.push(msg);
            return result;
        }
    };

    if versions.is_empty() {
        debug!("No runtime versions registered, skipping verification");
        return result;
    }

    info!("Found {} runtime version(s) to verify", versions.len());

    for version in &versions {
        // Apply runtime filter: extract the runtime base name from the ref
        // e.g., "core.python" → "python"
        let rt_base_name = version
            .runtime_ref
            .split('.')
            .next_back()
            .unwrap_or(&version.runtime_ref)
            .to_lowercase();

        if let Some(filter) = runtime_filter {
            if !runtime_aliases_match_filter(std::slice::from_ref(&rt_base_name), filter) {
                debug!(
                    "Skipping version '{}' of runtime '{}' (not in worker runtime filter)",
                    version.version, version.runtime_ref,
                );
                continue;
            }
        }

        result.total_checked += 1;

        let is_available = verify_version_with_parent(pool, version, child_environment).await;

        // Update the database
        match RuntimeVersionRepository::set_availability(pool, version.id, is_available).await {
            Ok(_) => {
                if is_available {
                    info!(
                        "Runtime version '{}' {} is available",
                        version.runtime_ref, version.version,
                    );
                    result.available += 1;
                } else {
                    info!(
                        "Runtime version '{}' {} is NOT available on this system",
                        version.runtime_ref, version.version,
                    );
                    result.unavailable += 1;
                }
            }
            Err(e) => {
                let msg = format!(
                    "Failed to update availability for version '{}' {}: {}",
                    version.runtime_ref, version.version, e,
                );
                warn!("{}", msg);
                result.errors.push(msg);
            }
        }
    }

    info!(
        "Runtime version verification complete: {} checked, {} available, {} unavailable, {} error(s)",
        result.total_checked,
        result.available,
        result.unavailable,
        result.errors.len(),
    );

    result
}

/// Build the worker capability payload describing which verified runtime
/// versions are currently available on this worker.
///
/// The JSON shape is:
/// `{ "<normalized-runtime-name>": ["<version>", ...] }`
///
/// Example:
/// `{ "python": ["3.12", "3.11"], "node": ["20", "18"] }`
pub fn build_runtime_versions_capability(
    versions: &[RuntimeVersion],
    runtime_filter: Option<&[String]>,
) -> JsonValue {
    let mut grouped: HashMap<String, Vec<String>> = HashMap::new();

    for version in versions
        .iter()
        .filter(|version| version.available && version.execution_config.get("lifecycle").is_none())
    {
        let runtime_name = normalize_runtime_name(
            version
                .runtime_ref
                .split('.')
                .next_back()
                .unwrap_or(&version.runtime_ref),
        );

        if let Some(filter) = runtime_filter {
            if !runtime_aliases_match_filter(std::slice::from_ref(&runtime_name), filter) {
                continue;
            }
        }

        grouped
            .entry(runtime_name)
            .or_default()
            .push(version.version.clone());
    }

    for versions in grouped.values_mut() {
        versions.sort_by(|a, b| match (parse_version(a), parse_version(b)) {
            (Ok(a_ver), Ok(b_ver)) => b_ver.cmp(&a_ver),
            _ => b.cmp(a),
        });
        versions.dedup();
    }

    let mut entries: Vec<(String, Vec<String>)> = grouped.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    JsonValue::Object(
        entries
            .into_iter()
            .map(|(runtime_name, versions)| (runtime_name, json!(versions)))
            .collect(),
    )
}

/// Query runtime versions from the database and build the worker capability
/// payload from versions that verify locally on this worker.
///
/// This intentionally does **not** trust the shared `runtime_version.available`
/// flag, because that flag is global across workers and may reflect a different
/// host/container in heterogeneous deployments.
pub async fn collect_runtime_versions_capability(
    pool: &PgPool,
    runtime_filter: Option<&[String]>,
    child_environment: &ChildProcessEnvironment,
) -> JsonValue {
    match RuntimeVersionRepository::list(pool).await {
        Ok(versions) => {
            let mut local_versions = Vec::new();

            for version in versions {
                let runtime_name = normalize_runtime_name(
                    version
                        .runtime_ref
                        .split('.')
                        .next_back()
                        .unwrap_or(&version.runtime_ref),
                );

                if let Some(filter) = runtime_filter {
                    if !runtime_aliases_match_filter(std::slice::from_ref(&runtime_name), filter) {
                        continue;
                    }
                }

                if verify_version_with_parent(pool, &version, child_environment).await {
                    let mut verified_version = version.clone();
                    verified_version.available = true;
                    local_versions.push(verified_version);
                }
            }

            build_runtime_versions_capability(&local_versions, runtime_filter)
        }
        Err(e) => {
            warn!(
                "Failed to load runtime versions for worker capability refresh: {}",
                e
            );
            JsonValue::Object(Default::default())
        }
    }
}

/// Verify a single runtime version by running its verification commands.
///
/// Returns `true` if at least one verification command succeeds.
async fn verify_version_with_parent(
    pool: &PgPool,
    version: &RuntimeVersion,
    environment: &ChildProcessEnvironment,
) -> bool {
    let Ok(Some(parent)) = RuntimeRepository::find_by_id(pool, version.runtime).await else {
        return false;
    };
    if attune_common::runtime_lifecycle::validate_version_mode(
        &parent.execution_config,
        &version.execution_config,
    )
    .is_err()
    {
        return false;
    }
    attune_common::runtime_detection::verify_registered_runtime_version(version, environment).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_version(runtime_ref: &str, version: &str, available: bool) -> RuntimeVersion {
        RuntimeVersion {
            id: 1,
            runtime: 1,
            runtime_ref: runtime_ref.to_string(),
            version: version.to_string(),
            version_major: None,
            version_minor: None,
            version_patch: None,
            execution_config: json!({}),
            distributions: json!({}),
            is_default: false,
            available,
            verified_at: None,
            meta: json!({}),
            retired_at: None,
            created: chrono::Utc::now(),
            updated: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_build_runtime_versions_capability_groups_and_sorts_versions() {
        let versions = vec![
            make_version("core.python", "3.11", true),
            make_version("core.python", "3.13", false),
            make_version("core.python", "3.12", true),
            make_version("core.nodejs", "18", true),
            make_version("core.nodejs", "20", true),
        ];

        let capability = build_runtime_versions_capability(&versions, None);

        assert_eq!(
            capability,
            json!({
                "node": ["20", "18"],
                "python": ["3.12", "3.11"]
            })
        );
    }

    #[test]
    fn test_build_runtime_versions_capability_applies_runtime_filter() {
        let versions = vec![
            make_version("core.python", "3.12", true),
            make_version("core.nodejs", "20", true),
        ];

        let capability =
            build_runtime_versions_capability(&versions, Some(&["python3".to_string()]));

        assert_eq!(
            capability,
            json!({
                "python": ["3.12"]
            })
        );
    }
}
