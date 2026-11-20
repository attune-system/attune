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
use std::sync::LazyLock;
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
    verify_single_version(version, environment).await
}

async fn verify_single_version(
    version: &RuntimeVersion,
    child_environment: &ChildProcessEnvironment,
) -> bool {
    if version.execution_config.get("lifecycle").is_some() {
        return attune_common::runtime_lifecycle::verify_registered_version(
            version,
            child_environment,
        )
        .await;
    }
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

        return run_basic_binary_check(binary, child_environment).await;
    }

    // Run commands in priority order (lowest priority number = highest priority)
    for cmd in &commands {
        match run_verification_command(cmd, child_environment, &version.runtime_ref).await {
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
async fn run_verification_command(
    cmd: &VerificationCommand,
    child_environment: &ChildProcessEnvironment,
    runtime_ref: &str,
) -> std::result::Result<bool, String> {
    let mut command = Command::new(&cmd.binary);
    child_environment.apply(command.as_std_mut());
    command.args(&cmd.args);
    let mut command = Command::from(
        child_environment
            .sandbox_workload(command.into_std(), &[])
            .map_err(|e| format!("Failed to isolate verification for '{}': {e}", cmd.binary))?,
    );
    let output = command
        .stdin(std::process::Stdio::null())
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
            log_verification_pattern_mismatch(cmd, &result.stdout, &result.stderr);
            return Ok(false);
        }

        let version = (cmd.args == ["--version"])
            .then(|| observed_builtin_version(runtime_ref, &result.stdout, &result.stderr))
            .flatten();
        if let Some(version) = version {
            info!(
                runtime = runtime_ref,
                version = version,
                "Verified interpreter version"
            );
        }
    }

    Ok(true)
}

/// Extract a bounded version field independently of the runtime's acceptance regex.
fn observed_builtin_version<'a>(
    runtime_ref: &str,
    stdout: &'a [u8],
    stderr: &'a [u8],
) -> Option<&'a str> {
    static VERSION: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\A[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\z")
            .expect("valid interpreter version regex")
    });
    let prefix = match runtime_ref {
        "core.python" => "Python ",
        "core.nodejs" => "v",
        _ => return None,
    };
    let output = match (stdout, stderr) {
        ([], output) | (output, []) => std::str::from_utf8(output).ok()?,
        _ => return None,
    };
    let line = output
        .strip_suffix("\r\n")
        .or_else(|| output.strip_suffix('\n'))
        .unwrap_or(output);
    let version = line.strip_prefix(prefix)?;
    VERSION.is_match(version).then_some(version)
}

fn log_verification_pattern_mismatch(cmd: &VerificationCommand, stdout: &[u8], stderr: &[u8]) {
    // The selected child baseline can contain purpose-bounded operator
    // passthrough values. A probe may echo them, so retain only output sizes.
    debug!(
        binary = %cmd.binary,
        pattern = cmd.pattern.as_deref().unwrap_or_default(),
        stdout_bytes = stdout.len(),
        stderr_bytes = stderr.len(),
        "Verification output did not match expected pattern"
    );
}

/// Basic binary availability check: run `binary --version` and check for exit 0.
async fn run_basic_binary_check(binary: &str, child_environment: &ChildProcessEnvironment) -> bool {
    let mut command = Command::new(binary);
    child_environment.apply(command.as_std_mut());
    command.arg("--version");
    let command = child_environment.sandbox_workload(command.into_std(), &[]);
    let mut command = match command {
        Ok(command) => Command::from(command),
        Err(error) => {
            debug!("Failed to isolate binary check for '{}': {}", binary, error);
            return false;
        }
    };
    match command
        .stdin(std::process::Stdio::null())
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
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct TestLogWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for TestLogWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer lock")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TestLogWriter {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

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
        let result =
            run_basic_binary_check("__nonexistent_binary_12345__", &Default::default()).await;
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
        let result = run_verification_command(&cmd, &Default::default(), "core.python").await;
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn verification_mismatch_log_excludes_child_output() {
        let variable = "OWNED_VERIFY_PASSTHROUGH".to_string();
        let secret = "owned-dummy-passthrough-secret";
        let environment = ChildProcessEnvironment::from_parent(
            [(
                std::ffi::OsString::from(&variable),
                std::ffi::OsString::from(secret),
            )],
            std::slice::from_ref(&variable),
        )
        .unwrap();
        let cmd = VerificationCommand {
            binary: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf %s \"$OWNED_VERIFY_PASSTHROUGH\"; printf %s \"$OWNED_VERIFY_PASSTHROUGH\" >&2"
                    .to_string(),
            ],
            expected_exit_code: 0,
            pattern: Some("expected-version".to_string()),
            priority: 1,
        };
        let writer = TestLogWriter::default();
        let buffer = writer.0.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(writer)
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let guard = tracing::dispatcher::set_default(&dispatch);

        assert!(!run_verification_command(&cmd, &environment, "core.python")
            .await
            .unwrap());

        drop(guard);
        let bytes = buffer.lock().expect("log buffer lock").clone();
        let log = String::from_utf8(bytes).expect("debug log should be UTF-8");

        assert!(log.contains("binary=/bin/sh"));
        assert!(log.contains("stdout_bytes=30"));
        assert!(log.contains("stderr_bytes=30"));
        assert!(!log.contains(secret));
    }

    #[test]
    fn observed_version_requires_one_builtin_version_line() {
        assert_eq!(
            observed_builtin_version("core.python", b"Python 3.12.8\n", b""),
            Some("3.12.8")
        );
        assert_eq!(
            observed_builtin_version("core.nodejs", b"", b"v22.5.1\r\n"),
            Some("22.5.1")
        );
        assert_eq!(
            observed_builtin_version("custom.python", b"Python 3.12.8\n", b""),
            None
        );
        for (stdout, stderr) in [
            (&b"Python 3.12.8\nextra"[..], &b""[..]),
            (b"Python 3.12.8 extra\n", b""),
            (b"Python 3.12.8\n", b"extra"),
            (b"Python 3.12.8675309\n", b""),
            (b"Python 3.12.8\n\n", b""),
        ] {
            assert_eq!(
                observed_builtin_version("core.python", stdout, stderr),
                None
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn verification_logs_only_a_matched_builtin_version() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("version-probe");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf 'Python 3.12.8\\n%s' \"$PROBE_EXTRA\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut cmd = VerificationCommand {
            binary: executable.to_string_lossy().into_owned(),
            args: vec!["--version".to_string()],
            expected_exit_code: 0,
            pattern: Some(r"Python 3\.12\.".to_string()),
            priority: 1,
        };
        let variable = "PROBE_EXTRA".to_string();
        let extra = ChildProcessEnvironment::from_parent(
            [(variable.clone().into(), "owned-dummy-extra".into())],
            std::slice::from_ref(&variable),
        )
        .unwrap();

        let writer = TestLogWriter::default();
        let buffer = writer.0.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(writer)
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);
        assert!(
            run_verification_command(&cmd, &Default::default(), "core.python")
                .await
                .unwrap()
        );
        assert!(run_verification_command(&cmd, &extra, "core.python")
            .await
            .unwrap());
        assert!(
            run_verification_command(&cmd, &Default::default(), "custom.python")
                .await
                .unwrap()
        );
        cmd.pattern = Some(r"Python 3\.13\.".to_string());
        assert!(
            !run_verification_command(&cmd, &Default::default(), "core.python")
                .await
                .unwrap()
        );

        let bytes = buffer.lock().unwrap();
        let log = std::str::from_utf8(&bytes).unwrap();
        assert_eq!(log.matches("Verified interpreter version").count(), 1);
        assert!(log.contains("version=\"3.12.8\""));
        assert!(!log.contains("owned-dummy-extra"));
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
