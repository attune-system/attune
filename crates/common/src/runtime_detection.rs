//! Runtime Detection Module
//!
//! Provides unified runtime capability detection for both sensor and worker services.
//! Supports three-tier configuration:
//! 1. Environment variable override (highest priority)
//! 2. Config file specification (medium priority)
//! 3. Database-driven detection with verification (lowest priority)
//!
//! Also provides alias-based matching functions ([`runtime_aliases_match_filter`]
//! and [`runtime_aliases_contain`]) for comparing runtime alias lists against
//! worker filters and capability strings. Aliases are declared per-runtime in
//! pack manifests, so no hardcoded alias table is needed here.

use crate::child_process_environment::ChildProcessEnvironment;
use crate::config::Config;
use crate::error::Result;
use crate::models::Runtime;
use crate::repositories::runtime::SELECT_COLUMNS;
use serde_json::json;
use sqlx::PgPool;
use std::collections::HashMap;
use std::process::Command;
use tracing::{debug, info, warn};

/// Check if a runtime's aliases overlap with a filter list.
///
/// The filter list comes from `ATTUNE_WORKER_RUNTIMES` (e.g., `["python", "shell"]`).
/// A runtime matches if any of its declared aliases appear in the filter list.
/// Comparison is case-insensitive.
pub fn normalize_runtime_name(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "node" | "nodejs" | "node.js" => "node".to_string(),
        "python" | "python3" => "python".to_string(),
        "shell" | "bash" | "sh" => "shell".to_string(),
        "native" | "builtin" | "standalone" => "native".to_string(),
        "ruby" | "rb" => "ruby".to_string(),
        "go" | "golang" => "go".to_string(),
        "java" | "jdk" | "openjdk" => "java".to_string(),
        "perl" | "perl5" => "perl".to_string(),
        "r" | "rscript" => "r".to_string(),
        other => other.to_string(),
    }
}

pub fn runtime_aliases_match_filter(aliases: &[String], filter: &[String]) -> bool {
    aliases.iter().any(|alias| {
        let normalized_alias = normalize_runtime_name(alias);
        filter
            .iter()
            .any(|f| normalize_runtime_name(f) == normalized_alias)
    })
}

/// Check if a runtime's aliases contain a specific name.
///
/// Used by the scheduler to check if a worker's capability string
/// (e.g., "python") matches a runtime's aliases (e.g., ["python", "python3"]).
/// Comparison is case-insensitive.
pub fn runtime_aliases_contain(aliases: &[String], name: &str) -> bool {
    let normalized_name = normalize_runtime_name(name);
    aliases
        .iter()
        .any(|a| normalize_runtime_name(a) == normalized_name)
}

/// Runtime detection service
pub struct RuntimeDetector {
    pool: PgPool,
    child_environment: ChildProcessEnvironment,
}

impl RuntimeDetector {
    /// Create a new runtime detector
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            child_environment: Default::default(),
        }
    }

    pub fn with_child_environment(mut self, environment: ChildProcessEnvironment) -> Self {
        self.child_environment = environment;
        self
    }

    /// Detect available runtimes using three-tier priority:
    /// 1. Environment variable (ATTUNE_WORKER_RUNTIMES or ATTUNE_SENSOR_RUNTIMES)
    /// 2. Config file capabilities
    /// 3. Database-driven detection with verification
    ///
    /// Returns a HashMap of capabilities including the "runtimes" key with detected runtime names
    pub async fn detect_capabilities(
        &self,
        _config: &Config,
        env_var_name: &str,
        config_capabilities: Option<&HashMap<String, serde_json::Value>>,
    ) -> Result<HashMap<String, serde_json::Value>> {
        let mut capabilities = HashMap::new();

        // Check environment variable override first (highest priority)
        if let Ok(runtimes_env) = std::env::var(env_var_name) {
            info!(
                "Using runtimes from {} (override): {}",
                env_var_name, runtimes_env
            );
            let runtime_list: Vec<String> = runtimes_env
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
            capabilities.insert("runtimes".to_string(), json!(runtime_list));

            // Copy any other capabilities from config
            if let Some(config_caps) = config_capabilities {
                for (key, value) in config_caps.iter() {
                    if key != "runtimes" {
                        capabilities.insert(key.clone(), value.clone());
                    }
                }
            }

            return Ok(capabilities);
        }

        // Check config file (medium priority)
        if let Some(config_caps) = config_capabilities {
            if let Some(config_runtimes) = config_caps.get("runtimes") {
                if let Some(runtime_array) = config_runtimes.as_array() {
                    if !runtime_array.is_empty() {
                        info!("Using runtimes from config file");
                        let runtime_list: Vec<String> = runtime_array
                            .iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
                            .collect();
                        capabilities.insert("runtimes".to_string(), json!(runtime_list));

                        // Copy other capabilities from config
                        for (key, value) in config_caps.iter() {
                            if key != "runtimes" {
                                capabilities.insert(key.clone(), value.clone());
                            }
                        }

                        return Ok(capabilities);
                    }
                }
            }

            // Copy non-runtime capabilities from config
            for (key, value) in config_caps.iter() {
                if key != "runtimes" {
                    capabilities.insert(key.clone(), value.clone());
                }
            }
        }

        // Database-driven detection (lowest priority)
        info!("No runtime override found, detecting from database...");
        let detected_runtimes = self.detect_from_database().await?;
        capabilities.insert("runtimes".to_string(), json!(detected_runtimes));

        Ok(capabilities)
    }

    /// Detect available runtimes by querying database and verifying each runtime
    pub async fn detect_from_database(&self) -> Result<Vec<String>> {
        info!("Querying database for runtime definitions...");

        // Query all runtimes from database
        let query = format!("SELECT {} FROM runtime ORDER BY ref", SELECT_COLUMNS);
        let runtimes = sqlx::query_as::<_, Runtime>(&query)
            .fetch_all(&self.pool)
            .await?;

        info!("Found {} runtime(s) in database", runtimes.len());

        let mut available_runtimes = Vec::new();

        // Verify each runtime
        for runtime in runtimes {
            if self.verify_runtime_available(&runtime).await {
                info!("✓ Runtime available: {} ({})", runtime.name, runtime.r#ref);
                available_runtimes.push(runtime.name.to_lowercase());
            } else {
                debug!(
                    "✗ Runtime not available: {} ({})",
                    runtime.name, runtime.r#ref
                );
            }
        }

        info!("Detected available runtimes: {:?}", available_runtimes);

        Ok(available_runtimes)
    }

    /// Verify if a runtime is available on this system
    pub async fn verify_runtime_available(&self, runtime: &Runtime) -> bool {
        // Check if runtime is always available (e.g., shell, native)
        if let Some(verification) = runtime.distributions.get("verification") {
            if let Some(always_available) = verification.get("always_available") {
                if always_available.as_bool() == Some(true) {
                    debug!("Runtime {} is marked as always available", runtime.name);
                    return true;
                }
            }

            if let Some(check_required) = verification.get("check_required") {
                if check_required.as_bool() == Some(false) {
                    debug!(
                        "Runtime {} does not require verification check",
                        runtime.name
                    );
                    return true;
                }
            }

            // Get verification commands
            if let Some(commands) = verification.get("commands") {
                if let Some(commands_array) = commands.as_array() {
                    // Try each command in priority order
                    let mut sorted_commands = commands_array.clone();
                    sorted_commands.sort_by_key(|cmd| {
                        cmd.get("priority").and_then(|p| p.as_i64()).unwrap_or(999)
                    });

                    for cmd in sorted_commands {
                        if Self::try_verification_command(
                            &cmd,
                            &runtime.name,
                            &self.child_environment,
                        )
                        .await
                        {
                            return true;
                        }
                    }
                }
            }
        }

        // No verification metadata or all checks failed
        false
    }

    /// Try executing a verification command to check if runtime is available
    async fn try_verification_command(
        cmd: &serde_json::Value,
        runtime_name: &str,
        child_environment: &ChildProcessEnvironment,
    ) -> bool {
        let binary = match cmd.get("binary").and_then(|b| b.as_str()) {
            Some(b) => b,
            None => {
                warn!(
                    "Verification command missing 'binary' field for {}",
                    runtime_name
                );
                return false;
            }
        };

        let args = cmd
            .get("args")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();

        let expected_exit_code = cmd.get("exit_code").and_then(|e| e.as_i64()).unwrap_or(0);

        let pattern = cmd.get("pattern").and_then(|p| p.as_str());

        let optional = cmd
            .get("optional")
            .and_then(|o| o.as_bool())
            .unwrap_or(false);

        debug!(
            "Trying verification: {} {:?} (expecting exit code {})",
            binary, args, expected_exit_code
        );

        // Execute command
        let mut command = Command::new(binary);
        child_environment.apply(&mut command);
        let output = match command.args(&args).output() {
            Ok(output) => output,
            Err(e) => {
                if !optional {
                    debug!("Failed to execute {}: {}", binary, e);
                }
                return false;
            }
        };

        // Check exit code
        let exit_code = output.status.code().unwrap_or(-1);
        if exit_code != expected_exit_code as i32 {
            if !optional {
                debug!(
                    "Command {} exited with {} (expected {})",
                    binary, exit_code, expected_exit_code
                );
            }
            return false;
        }

        // Check pattern if specified
        if let Some(pattern_str) = pattern {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined_output = format!("{}{}", stdout, stderr);

            match regex::Regex::new(pattern_str) {
                Ok(re) => {
                    if re.is_match(&combined_output) {
                        debug!(
                            "✓ Runtime verified: {} (matched pattern: {})",
                            runtime_name, pattern_str
                        );
                        return true;
                    } else {
                        if !optional {
                            debug!(
                                "Command {} output did not match pattern: {}",
                                binary, pattern_str
                            );
                        }
                        return false;
                    }
                }
                Err(e) => {
                    warn!("Invalid regex pattern '{}': {}", pattern_str, e);
                    return false;
                }
            }
        }

        // No pattern specified, just check exit code (already verified above)
        debug!("✓ Runtime verified: {} (exit code match)", runtime_name);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_verification_uses_selected_environment() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::TempDir::new().unwrap();
        let executable = root.path().join("runtime");
        std::fs::write(&executable, "#!/bin/sh\ntest \"$LD_LIBRARY_PATH\" = /configured/library || exit 19\ntest -z \"${ATTUNE__SECURITY__JWT_SECRET+x}\" || exit 20\nprintf 'v3.14.15\\n'\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let parent = vec![
            ("LD_LIBRARY_PATH".into(), "/configured/library".into()),
            ("ATTUNE__SECURITY__JWT_SECRET".into(), "dummy-jwt".into()),
        ];
        let minimal = ChildProcessEnvironment::from_parent(parent.clone(), &[]).unwrap();
        let selected =
            ChildProcessEnvironment::from_parent(parent, &["LD_LIBRARY_PATH".into()]).unwrap();
        let command = serde_json::json!({"binary": executable, "args": [], "exit_code": 0, "pattern": "3\\.14\\.15"});
        assert!(!RuntimeDetector::try_verification_command(&command, "fixture", &minimal).await);
        assert!(RuntimeDetector::try_verification_command(&command, "fixture", &selected).await);
    }
    use serde_json::json;

    #[test]
    fn test_runtime_aliases_match_filter() {
        let aliases = vec!["python".to_string(), "python3".to_string()];
        let filter = vec!["python".to_string(), "shell".to_string()];
        assert!(runtime_aliases_match_filter(&aliases, &filter));

        let filter_no_match = vec!["node".to_string(), "ruby".to_string()];
        assert!(!runtime_aliases_match_filter(&aliases, &filter_no_match));
    }

    #[test]
    fn test_runtime_aliases_match_filter_normalizes_common_aliases() {
        let aliases = vec!["node.js".to_string()];
        let filter = vec!["node".to_string()];
        assert!(runtime_aliases_match_filter(&aliases, &filter));

        let aliases = vec!["python3".to_string()];
        let filter = vec!["python".to_string()];
        assert!(runtime_aliases_match_filter(&aliases, &filter));
    }

    #[test]
    fn test_runtime_aliases_contain_normalizes_common_aliases() {
        let aliases = vec!["nodejs".to_string()];
        assert!(runtime_aliases_contain(&aliases, "node"));
        assert!(runtime_aliases_contain(&aliases, "node.js"));
    }

    #[test]
    fn test_runtime_aliases_match_filter_case_insensitive() {
        let aliases = vec!["Python".to_string(), "python3".to_string()];
        let filter = vec!["python".to_string()];
        assert!(runtime_aliases_match_filter(&aliases, &filter));
    }

    #[test]
    fn test_runtime_aliases_match_filter_empty() {
        let aliases: Vec<String> = vec![];
        let filter = vec!["python".to_string()];
        assert!(!runtime_aliases_match_filter(&aliases, &filter));

        let aliases = vec!["python".to_string()];
        let filter: Vec<String> = vec![];
        assert!(!runtime_aliases_match_filter(&aliases, &filter));
    }

    #[test]
    fn test_runtime_aliases_contain() {
        let aliases = vec!["ruby".to_string(), "rb".to_string()];
        assert!(runtime_aliases_contain(&aliases, "ruby"));
        assert!(runtime_aliases_contain(&aliases, "rb"));
        assert!(!runtime_aliases_contain(&aliases, "python"));
    }

    #[test]
    fn test_runtime_aliases_contain_case_insensitive() {
        let aliases = vec!["ruby".to_string(), "rb".to_string()];
        assert!(runtime_aliases_contain(&aliases, "Ruby"));
        assert!(runtime_aliases_contain(&aliases, "RB"));
    }

    #[test]
    fn test_verification_command_structure() {
        let cmd = json!({
            "binary": "python3",
            "args": ["--version"],
            "exit_code": 0,
            "pattern": "Python 3\\.",
            "priority": 1
        });

        assert_eq!(cmd.get("binary").unwrap().as_str().unwrap(), "python3");
        assert!(cmd.get("args").unwrap().is_array());
        assert_eq!(cmd.get("exit_code").unwrap().as_i64().unwrap(), 0);
    }

    #[test]
    fn test_always_available_flag() {
        let verification = json!({
            "always_available": true
        });

        assert!(verification
            .get("always_available")
            .unwrap()
            .as_bool()
            .unwrap());
    }

    #[tokio::test]
    async fn test_verify_command_with_pattern() {
        // Test shell verification (should always work)
        let cmd = json!({
            "binary": "sh",
            "args": ["--version"],
            "exit_code": 0,
            "optional": true,
            "priority": 1
        });

        // This might fail on some systems, but should not panic
        let _ = RuntimeDetector::try_verification_command(&cmd, "Shell", &Default::default()).await;
    }
}
