//! Local verification of an exact registered runtime version.
use crate::{child_process_environment::ChildProcessEnvironment, models::RuntimeVersion};
use std::{sync::LazyLock, time::Duration};
use tokio::process::Command;
use tracing::{debug, info};

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

pub async fn verify_registered_runtime_version(
    version: &RuntimeVersion,
    child_environment: &ChildProcessEnvironment,
) -> bool {
    if version.execution_config.get("lifecycle").is_some() {
        return crate::runtime_lifecycle::verify_registered_version(version, child_environment)
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
}
