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

use attune_common::repositories::List;
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use std::collections::HashMap;
use std::time::Duration;
use tokio::process::Command;
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

/// A single verification command extracted from the `distributions` JSONB.
#[derive(Debug)]
struct VerificationCommand {
    binary: String,
    args: Vec<String>,
    expected_exit_code: i32,
    pattern: Option<String>,
    #[allow(dead_code)]
    priority: i32,
}

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

        let is_available = verify_single_version(version).await;

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

    for version in versions.iter().filter(|version| version.available) {
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

                if verify_single_version(&version).await {
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
async fn verify_single_version(version: &RuntimeVersion) -> bool {
    let commands = extract_verification_commands(&version.distributions);

    if commands.is_empty() {
        // No verification commands — try using the version's execution_config
        // interpreter binary with --version as a basic check.
        let exec_config = version.parsed_execution_config();
        let binary = &exec_config.interpreter.binary;
        if binary.is_empty() {
            debug!(
                "No verification commands and no interpreter for '{}' {}. \
                 Assuming available (will fail at execution time if not).",
                version.runtime_ref, version.version,
            );
            return true;
        }

        debug!(
            "No verification commands for '{}' {}. \
             Falling back to '{} --version' check.",
            version.runtime_ref, version.version, binary,
        );

        return run_basic_binary_check(binary).await;
    }

    // Run commands in priority order (lowest priority number = highest priority)
    for cmd in &commands {
        match run_verification_command(cmd).await {
            Ok(true) => {
                debug!(
                    "Verification passed for '{}' {} using binary '{}'",
                    version.runtime_ref, version.version, cmd.binary,
                );
                return true;
            }
            Ok(false) => {
                debug!(
                    "Verification failed for '{}' {} using binary '{}' \
                     (pattern mismatch or non-zero exit)",
                    version.runtime_ref, version.version, cmd.binary,
                );
            }
            Err(e) => {
                debug!(
                    "Verification command '{}' for '{}' {} failed: {}",
                    cmd.binary, version.runtime_ref, version.version, e,
                );
            }
        }
    }

    false
}

/// Extract verification commands from the `distributions` JSONB.
///
/// Expected structure:
/// ```json
/// {
///   "verification": {
///     "commands": [
///       {
///         "binary": "python3.12",
///         "args": ["--version"],
///         "exit_code": 0,
///         "pattern": "Python 3\\.12\\.",
///         "priority": 1
///       }
///     ]
///   }
/// }
/// ```
fn extract_verification_commands(distributions: &serde_json::Value) -> Vec<VerificationCommand> {
    let mut commands = Vec::new();

    let cmds = match distributions
        .get("verification")
        .and_then(|v| v.get("commands"))
        .and_then(|v| v.as_array())
    {
        Some(arr) => arr,
        None => return commands,
    };

    for cmd_val in cmds {
        let binary = match cmd_val.get("binary").and_then(|v| v.as_str()) {
            Some(b) => b.to_string(),
            None => continue,
        };

        let args: Vec<String> = cmd_val
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let expected_exit_code = cmd_val
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;

        let pattern = cmd_val
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(String::from);

        let priority = cmd_val
            .get("priority")
            .and_then(|v| v.as_i64())
            .unwrap_or(100) as i32;

        commands.push(VerificationCommand {
            binary,
            args,
            expected_exit_code,
            pattern,
            priority,
        });
    }

    // Sort by priority (lowest number = highest priority)
    commands.sort_by_key(|c| c.priority);
    commands
}

/// Run a single verification command and check exit code + output pattern.
async fn run_verification_command(cmd: &VerificationCommand) -> std::result::Result<bool, String> {
    let output = Command::new(&cmd.binary)
        .args(&cmd.args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Failed to spawn '{}': {}", cmd.binary, e))?;

    let result = tokio::time::timeout(Duration::from_secs(10), output.wait_with_output())
        .await
        .map_err(|_| format!("Verification command '{}' timed out after 10s", cmd.binary))?
        .map_err(|e| format!("Failed to wait for '{}': {}", cmd.binary, e))?;

    // Check exit code
    let actual_exit = result.status.code().unwrap_or(-1);
    if actual_exit != cmd.expected_exit_code {
        return Ok(false);
    }

    // Check output pattern if specified
    if let Some(ref pattern) = cmd.pattern {
        let stdout = String::from_utf8_lossy(&result.stdout);
        let stderr = String::from_utf8_lossy(&result.stderr);
        let combined = format!("{}{}", stdout, stderr);

        let re = regex::Regex::new(pattern)
            .map_err(|e| format!("Invalid verification pattern '{}': {}", pattern, e))?;

        if !re.is_match(&combined) {
            debug!(
                "Pattern '{}' did not match output of '{}': stdout='{}', stderr='{}'",
                pattern,
                cmd.binary,
                stdout.trim(),
                stderr.trim(),
            );
            return Ok(false);
        }
    }

    Ok(true)
}

/// Basic binary availability check: run `binary --version` and check for exit 0.
async fn run_basic_binary_check(binary: &str) -> bool {
    match Command::new(binary)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => {
            match tokio::time::timeout(Duration::from_secs(10), child.wait_with_output()).await {
                Ok(Ok(output)) => output.status.success(),
                Ok(Err(e)) => {
                    debug!("Binary check for '{}' failed: {}", binary, e);
                    false
                }
                Err(_) => {
                    debug!("Binary check for '{}' timed out", binary);
                    false
                }
            }
        }
        Err(e) => {
            debug!("Failed to spawn '{}': {}", binary, e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_verification_commands_full() {
        let distributions = json!({
            "verification": {
                "commands": [
                    {
                        "binary": "python3.12",
                        "args": ["--version"],
                        "exit_code": 0,
                        "pattern": "Python 3\\.12\\.",
                        "priority": 1
                    },
                    {
                        "binary": "python3",
                        "args": ["--version"],
                        "exit_code": 0,
                        "pattern": "Python 3\\.12\\.",
                        "priority": 2
                    }
                ]
            }
        });

        let cmds = extract_verification_commands(&distributions);
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].binary, "python3.12");
        assert_eq!(cmds[0].priority, 1);
        assert_eq!(cmds[1].binary, "python3");
        assert_eq!(cmds[1].priority, 2);
        assert_eq!(cmds[0].args, vec!["--version"]);
        assert_eq!(cmds[0].expected_exit_code, 0);
        assert_eq!(cmds[0].pattern.as_deref(), Some("Python 3\\.12\\."));
    }

    #[test]
    fn test_extract_verification_commands_empty() {
        let distributions = json!({});
        let cmds = extract_verification_commands(&distributions);
        assert!(cmds.is_empty());
    }

    #[test]
    fn test_extract_verification_commands_no_commands_array() {
        let distributions = json!({
            "verification": {}
        });
        let cmds = extract_verification_commands(&distributions);
        assert!(cmds.is_empty());
    }

    #[test]
    fn test_extract_verification_commands_missing_binary() {
        let distributions = json!({
            "verification": {
                "commands": [
                    {
                        "args": ["--version"],
                        "exit_code": 0
                    }
                ]
            }
        });
        let cmds = extract_verification_commands(&distributions);
        assert!(cmds.is_empty(), "Commands without binary should be skipped");
    }

    #[test]
    fn test_extract_verification_commands_defaults() {
        let distributions = json!({
            "verification": {
                "commands": [
                    {
                        "binary": "node"
                    }
                ]
            }
        });
        let cmds = extract_verification_commands(&distributions);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].binary, "node");
        assert!(cmds[0].args.is_empty());
        assert_eq!(cmds[0].expected_exit_code, 0);
        assert!(cmds[0].pattern.is_none());
        assert_eq!(cmds[0].priority, 100);
    }

    #[test]
    fn test_extract_verification_commands_sorted_by_priority() {
        let distributions = json!({
            "verification": {
                "commands": [
                    { "binary": "low", "priority": 10 },
                    { "binary": "high", "priority": 1 },
                    { "binary": "mid", "priority": 5 }
                ]
            }
        });
        let cmds = extract_verification_commands(&distributions);
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0].binary, "high");
        assert_eq!(cmds[1].binary, "mid");
        assert_eq!(cmds[2].binary, "low");
    }

    #[tokio::test]
    async fn test_run_basic_binary_check_nonexistent() {
        // A binary that definitely doesn't exist
        let result = run_basic_binary_check("__nonexistent_binary_12345__").await;
        assert!(!result);
    }

    #[tokio::test]
    async fn test_run_verification_command_nonexistent() {
        let cmd = VerificationCommand {
            binary: "__nonexistent_binary_12345__".to_string(),
            args: vec!["--version".to_string()],
            expected_exit_code: 0,
            pattern: None,
            priority: 1,
        };
        let result = run_verification_command(&cmd).await;
        assert!(result.is_err());
    }

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
