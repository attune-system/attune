//! Pack Test Executor Module
//!
//! Executes pack tests by running test runners and collecting results.

use crate::error::{Error, Result};
use crate::models::pack_test::{PackTestResult, TestCaseResult, TestStatus, TestSuiteResult};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::{debug, error, info, warn};

const MAX_TEST_OUTPUT_BYTES: usize = 1024 * 1024;

/// Test configuration from pack.yaml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConfig {
    pub enabled: bool,
    pub discovery: DiscoveryConfig,
    pub runners: HashMap<String, RunnerConfig>,
    pub result_format: Option<String>,
    pub result_path: Option<String>,
    pub min_pass_rate: Option<f64>,
    pub on_failure: Option<String>,
}

impl TestConfig {
    pub fn validate_policy(&self) -> Result<()> {
        if self
            .min_pass_rate
            .is_some_and(|rate| !(0.0..=1.0).contains(&rate))
        {
            return Err(Error::validation(
                "testing.min_pass_rate must be between 0.0 and 1.0",
            ));
        }
        if self
            .on_failure
            .as_deref()
            .is_some_and(|policy| !matches!(policy, "block" | "warn" | "ignore"))
        {
            return Err(Error::validation(
                "testing.on_failure must be block, warn, or ignore",
            ));
        }
        let mut total_timeout = 60_u64;
        for runner in self.runners.values() {
            let timeout = runner.timeout.unwrap_or(300);
            if !(1..=3600).contains(&timeout) {
                return Err(Error::validation(
                    "testing runner timeouts must be between 1 and 3600 seconds",
                ));
            }
            total_timeout = total_timeout.saturating_add(timeout);
        }
        if total_timeout > 21_660 {
            return Err(Error::validation(
                "combined testing runner timeout must not exceed 6 hours",
            ));
        }
        Ok(())
    }

    pub fn accepts_result(&self, result: &PackTestResult) -> bool {
        let has_execution_error = result.test_suites.iter().any(|suite| {
            suite
                .test_cases
                .iter()
                .any(|case| case.status == TestStatus::Error)
        });
        let meets_pass_rate = result.total_tests > 0
            && !has_execution_error
            && result.pass_rate >= self.min_pass_rate.unwrap_or(1.0);
        meets_pass_rate || matches!(self.on_failure.as_deref(), Some("warn" | "ignore"))
    }
}

/// Test discovery configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    pub method: String,
    pub path: Option<String>,
}

/// Test runner configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerConfig {
    pub r#type: String,
    pub entry_point: String,
    pub timeout: Option<u64>,
    pub result_format: Option<String>,
}

/// Test executor for running pack tests
pub struct TestExecutor {
    /// Base directory for pack files
    pack_base_dir: PathBuf,
}

impl TestExecutor {
    /// Create a new test executor
    pub fn new(pack_base_dir: PathBuf) -> Self {
        Self { pack_base_dir }
    }

    /// Execute all tests for a pack, looking up the pack directory from the base dir
    pub async fn execute_pack_tests(
        &self,
        pack_ref: &str,
        pack_version: &str,
        test_config: &TestConfig,
    ) -> Result<PackTestResult> {
        let pack_dir = self.pack_base_dir.join(pack_ref);
        self.execute_pack_tests_at(&pack_dir, pack_ref, pack_version, test_config)
            .await
    }

    /// Execute all tests for a pack at a specific directory path.
    ///
    /// Use this when the pack files are not yet at the standard
    /// `packs_base_dir/pack_ref` location (e.g., during installation
    /// from a temp directory).
    pub async fn execute_pack_tests_at(
        &self,
        pack_dir: &Path,
        pack_ref: &str,
        pack_version: &str,
        test_config: &TestConfig,
    ) -> Result<PackTestResult> {
        self.execute_pack_tests_at_with_python_interpreter(
            pack_dir,
            pack_ref,
            pack_version,
            test_config,
            None,
        )
        .await
    }

    /// Execute tests using a prepared Python interpreter when one is available.
    pub async fn execute_pack_tests_at_with_python_interpreter(
        &self,
        pack_dir: &Path,
        pack_ref: &str,
        pack_version: &str,
        test_config: &TestConfig,
        python_interpreter: Option<&Path>,
    ) -> Result<PackTestResult> {
        info!("Executing tests for pack: {} v{}", pack_ref, pack_version);

        if !test_config.enabled {
            return Err(Error::Validation(
                "Testing is not enabled for this pack".to_string(),
            ));
        }
        test_config.validate_policy()?;

        if !pack_dir.exists() {
            return Err(Error::not_found(
                "pack_directory",
                "path",
                pack_dir.display().to_string(),
            ));
        }

        let start_time = Instant::now();
        let execution_time = Utc::now();
        let mut test_suites = Vec::new();

        // Execute tests for each runner
        for (runner_name, runner_config) in &test_config.runners {
            info!(
                "Running test suite: {} ({})",
                runner_name, runner_config.r#type
            );

            match self
                .execute_test_suite(pack_dir, runner_name, runner_config, python_interpreter)
                .await
            {
                Ok(suite_result) => {
                    info!(
                        "Test suite '{}' completed: {}/{} passed",
                        runner_name, suite_result.passed, suite_result.total
                    );
                    test_suites.push(suite_result);
                }
                Err(e) => {
                    error!("Test suite '{}' failed to execute: {}", runner_name, e);
                    // Create a failed suite result
                    test_suites.push(TestSuiteResult {
                        name: runner_name.clone(),
                        runner_type: runner_config.r#type.clone(),
                        total: 1,
                        passed: 0,
                        failed: 1,
                        skipped: 0,
                        duration_ms: 0,
                        test_cases: vec![TestCaseResult {
                            name: format!("{}_execution", runner_name),
                            status: TestStatus::Error,
                            duration_ms: 0,
                            error_message: Some(e.to_string()),
                            stdout: None,
                            stderr: None,
                        }],
                    });
                }
            }
        }

        let total_duration_ms = start_time.elapsed().as_millis() as i64;

        // Aggregate results
        let total_tests: i32 = test_suites.iter().map(|s| s.total).sum();
        let passed: i32 = test_suites.iter().map(|s| s.passed).sum();
        let failed: i32 = test_suites.iter().map(|s| s.failed).sum();
        let skipped: i32 = test_suites.iter().map(|s| s.skipped).sum();
        let pass_rate = if total_tests > 0 {
            passed as f64 / total_tests as f64
        } else {
            0.0
        };

        info!(
            "Pack tests completed: {}/{} passed ({:.1}%)",
            passed,
            total_tests,
            pass_rate * 100.0
        );

        // Determine overall test status
        let status = if failed > 0 {
            "failed".to_string()
        } else if passed == total_tests {
            "passed".to_string()
        } else if skipped == total_tests {
            "skipped".to_string()
        } else {
            "partial".to_string()
        };

        Ok(PackTestResult {
            pack_ref: pack_ref.to_string(),
            pack_version: pack_version.to_string(),
            execution_time,
            status,
            total_tests,
            passed,
            failed,
            skipped,
            pass_rate,
            duration_ms: total_duration_ms,
            test_suites,
        })
    }

    /// Execute a single test suite
    async fn execute_test_suite(
        &self,
        pack_dir: &Path,
        runner_name: &str,
        runner_config: &RunnerConfig,
        python_interpreter: Option<&Path>,
    ) -> Result<TestSuiteResult> {
        let start_time = Instant::now();

        // Resolve entry point path
        let entry_point = pack_dir.join(&runner_config.entry_point);
        if !entry_point.exists() {
            return Err(Error::not_found(
                "test_entry_point",
                "path",
                entry_point.display().to_string(),
            ));
        }

        // Determine command based on runner type
        // Use relative path from pack directory for the entry point
        let relative_entry_point = entry_point
            .strip_prefix(pack_dir)
            .unwrap_or(&entry_point)
            .to_string_lossy()
            .to_string();

        let (command, args) = match runner_config.r#type.as_str() {
            "script" => {
                // Execute as shell script
                let shell = if entry_point.extension().and_then(|s| s.to_str()) == Some("sh") {
                    "/bin/sh"
                } else {
                    "/bin/bash"
                };
                (shell.to_string(), vec![relative_entry_point])
            }
            "unittest" => {
                // Execute as Python unittest
                (
                    python_interpreter
                        .unwrap_or_else(|| Path::new("python3"))
                        .to_string_lossy()
                        .to_string(),
                    vec![
                        "-m".to_string(),
                        "unittest".to_string(),
                        relative_entry_point,
                    ],
                )
            }
            "pytest" => {
                // Run pytest as a module so the prepared environment supplies it.
                (
                    python_interpreter
                        .unwrap_or_else(|| Path::new("python3"))
                        .to_string_lossy()
                        .to_string(),
                    vec![
                        "-m".to_string(),
                        "pytest".to_string(),
                        relative_entry_point,
                        "-v".to_string(),
                    ],
                )
            }
            _ => {
                return Err(Error::Validation(format!(
                    "Unsupported runner type: {}",
                    runner_config.r#type
                )));
            }
        };

        // Execute test command with pack_dir as working directory
        let timeout_duration = Duration::from_secs(runner_config.timeout.unwrap_or(300));
        let output = self
            .run_command(
                &command,
                &args,
                pack_dir,
                python_interpreter.and_then(Path::parent),
                timeout_duration,
            )
            .await?;

        let duration_ms = start_time.elapsed().as_millis() as i64;

        // Parse output based on result format
        let result_format = runner_config.result_format.as_deref().unwrap_or("simple");

        let mut suite_result = match result_format {
            "simple" => self.parse_simple_output(&output, runner_name, &runner_config.r#type)?,
            "json" => self.parse_json_output(&output.stdout, runner_name)?,
            _ => {
                warn!(
                    "Unknown result format '{}', falling back to simple",
                    result_format
                );
                self.parse_simple_output(&output, runner_name, &runner_config.r#type)?
            }
        };

        suite_result.duration_ms = duration_ms;

        Ok(suite_result)
    }

    /// Run a command with timeout
    async fn run_command(
        &self,
        command: &str,
        args: &[String],
        working_dir: &Path,
        runtime_bin_dir: Option<&Path>,
        timeout: Duration,
    ) -> Result<CommandOutput> {
        debug!(
            "Executing command: {} {} (timeout: {:?})",
            command,
            args.join(" "),
            timeout
        );

        let path = runtime_bin_dir
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| format!("{}:/usr/local/bin:/usr/bin:/bin", path.display()))
            .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".to_string());
        let mut cmd = Command::new(command);
        cmd.env_clear()
            .env("PATH", path)
            .env("HOME", working_dir)
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .args(args)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        cmd.kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);

        let start = Instant::now();
        let mut child = cmd.spawn().map_err(|e| {
            Error::Internal(format!("Failed to spawn command '{}': {}", command, e))
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Internal("Test process stdout was not captured".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Internal("Test process stderr was not captured".to_string()))?;
        let stdout_task = tokio::spawn(Self::read_stream(stdout));
        let stderr_task = tokio::spawn(Self::read_stream(stderr));

        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(status) => {
                status.map_err(|e| Error::Internal(format!("Process wait failed: {e}")))?
            }
            Err(_) => {
                #[cfg(unix)]
                if let Some(pid) = child.id() {
                    // The child starts its own process group, so this also stops descendants.
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGKILL);
                    }
                }
                let _ = child.kill().await;
                let _ = child.wait().await;
                let _ = stdout_task.await;
                let _ = stderr_task.await;
                return Err(Error::Timeout(format!(
                    "Test execution timed out after {:?}",
                    timeout
                )));
            }
        };
        let stdout = stdout_task
            .await
            .map_err(|error| Error::Internal(format!("Stdout reader task failed: {error}")))??;
        let stderr = stderr_task
            .await
            .map_err(|error| Error::Internal(format!("Stderr reader task failed: {error}")))??;

        let duration_ms = start.elapsed().as_millis() as u64;
        let exit_code = status.code().unwrap_or(-1);

        Ok(CommandOutput {
            exit_code,
            stdout,
            stderr,
            duration_ms,
        })
    }

    /// Read from an async stream
    async fn read_stream(mut stream: impl tokio::io::AsyncRead + Unpin) -> Result<String> {
        let mut output = Vec::with_capacity(MAX_TEST_OUTPUT_BYTES);
        let mut chunk = [0_u8; 8192];
        loop {
            let read = stream
                .read(&mut chunk)
                .await
                .map_err(|e| Error::Internal(format!("Failed to read stream: {e}")))?;
            if read == 0 {
                break;
            }
            let remaining = MAX_TEST_OUTPUT_BYTES.saturating_sub(output.len());
            output.extend_from_slice(&chunk[..read.min(remaining)]);
        }
        Ok(String::from_utf8_lossy(&output).into_owned())
    }

    /// Parse simple test output format
    fn parse_simple_output(
        &self,
        output: &CommandOutput,
        runner_name: &str,
        runner_type: &str,
    ) -> Result<TestSuiteResult> {
        let text = format!("{}\n{}", output.stdout, output.stderr);

        // Parse test counts from output
        let total = self.extract_number(&text, "Total Tests:");
        let passed = self.extract_number(&text, "Passed:");
        let failed = self.extract_number(&text, "Failed:");
        let skipped = self.extract_number(&text, "Skipped:").or(Some(0));

        // If we couldn't parse counts, use exit code
        let (total, passed, failed, skipped) = if total.is_none() || passed.is_none() {
            if output.exit_code == 0 {
                (1, 1, 0, 0)
            } else {
                (1, 0, 1, 0)
            }
        } else {
            (
                total.unwrap_or(0),
                passed.unwrap_or(0),
                failed.unwrap_or(0),
                skipped.unwrap_or(0),
            )
        };
        let mut total = total.max(1);
        let mut passed = passed.max(0);
        let mut failed = failed.max(0);
        let skipped = skipped.max(0);
        if output.exit_code != 0 && failed == 0 {
            failed = 1;
            total = total.max(failed + skipped);
            passed = passed.min(total.saturating_sub(failed + skipped));
        }

        // Create a single test case representing the entire suite
        let test_case = TestCaseResult {
            name: format!("{}_suite", runner_name),
            status: if output.exit_code == 0 {
                TestStatus::Passed
            } else {
                TestStatus::Failed
            },
            duration_ms: output.duration_ms as i64,
            error_message: if output.exit_code != 0 {
                Some(format!("Exit code: {}", output.exit_code))
            } else {
                None
            },
            stdout: if !output.stdout.is_empty() {
                Some(output.stdout.clone())
            } else {
                None
            },
            stderr: if !output.stderr.is_empty() {
                Some(output.stderr.clone())
            } else {
                None
            },
        };

        Ok(TestSuiteResult {
            name: runner_name.to_string(),
            runner_type: runner_type.to_string(),
            total,
            passed,
            failed,
            skipped,
            duration_ms: output.duration_ms as i64,
            test_cases: vec![test_case],
        })
    }

    /// Parse JSON test output format
    fn parse_json_output(&self, _json_str: &str, _runner_name: &str) -> Result<TestSuiteResult> {
        // TODO: Implement JSON parsing for structured test results
        // For now, return a basic result
        Err(Error::Validation(
            "JSON result format not yet implemented".to_string(),
        ))
    }

    /// Extract a number from text after a label
    fn extract_number(&self, text: &str, label: &str) -> Option<i32> {
        text.lines()
            .find(|line| line.contains(label))
            .and_then(|line| {
                line.split(label)
                    .nth(1)?
                    .split_whitespace()
                    .next()?
                    .parse::<i32>()
                    .ok()
            })
    }
}

/// Command execution output
#[derive(Debug)]
struct CommandOutput {
    exit_code: i32,
    stdout: String,
    stderr: String,
    duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(min_pass_rate: Option<f64>, on_failure: Option<&str>) -> TestConfig {
        TestConfig {
            enabled: true,
            discovery: DiscoveryConfig {
                method: "directory".to_string(),
                path: Some("tests".to_string()),
            },
            runners: HashMap::new(),
            result_format: None,
            result_path: None,
            min_pass_rate,
            on_failure: on_failure.map(str::to_string),
        }
    }

    fn test_result(total_tests: i32, pass_rate: f64) -> PackTestResult {
        PackTestResult {
            pack_ref: "test".to_string(),
            pack_version: "1.0.0".to_string(),
            execution_time: Utc::now(),
            status: "failed".to_string(),
            total_tests,
            passed: (total_tests as f64 * pass_rate) as i32,
            failed: total_tests - (total_tests as f64 * pass_rate) as i32,
            skipped: 0,
            pass_rate,
            duration_ms: 1,
            test_suites: Vec::new(),
        }
    }

    #[test]
    fn test_policy_applies_threshold_and_failure_mode() {
        let partial = test_result(10, 0.8);
        assert!(test_config(Some(0.8), Some("block")).accepts_result(&partial));
        assert!(!test_config(Some(0.9), Some("block")).accepts_result(&partial));
        assert!(test_config(Some(1.0), Some("warn")).accepts_result(&partial));
        assert!(test_config(Some(1.0), Some("ignore")).accepts_result(&partial));
        assert!(!test_config(Some(0.0), Some("block")).accepts_result(&test_result(0, 0.0)));

        let mut runner_error = test_result(11, 10.0 / 11.0);
        runner_error.test_suites.push(TestSuiteResult {
            name: "broken".to_string(),
            runner_type: "script".to_string(),
            total: 1,
            passed: 0,
            failed: 1,
            skipped: 0,
            duration_ms: 0,
            test_cases: vec![TestCaseResult {
                name: "broken_execution".to_string(),
                status: TestStatus::Error,
                duration_ms: 0,
                error_message: Some("could not start".to_string()),
                stdout: None,
                stderr: None,
            }],
        });
        assert!(!test_config(Some(0.5), Some("block")).accepts_result(&runner_error));
        assert!(test_config(Some(1.0), Some("warn")).accepts_result(&runner_error));
    }

    #[test]
    fn test_policy_rejects_invalid_values() {
        assert!(test_config(Some(1.1), Some("block"))
            .validate_policy()
            .is_err());
        assert!(test_config(Some(1.0), Some("continue"))
            .validate_policy()
            .is_err());
        let mut config = test_config(Some(1.0), Some("block"));
        config.runners.insert(
            "slow".to_string(),
            RunnerConfig {
                r#type: "script".to_string(),
                entry_point: "tests/run.sh".to_string(),
                timeout: Some(3601),
                result_format: None,
            },
        );
        assert!(config.validate_policy().is_err());
    }

    #[test]
    fn test_extract_number() {
        let executor = TestExecutor::new(PathBuf::from("/tmp"));

        let text = "Total Tests: 36\nPassed: 35\nFailed: 1";

        assert_eq!(executor.extract_number(text, "Total Tests:"), Some(36));
        assert_eq!(executor.extract_number(text, "Passed:"), Some(35));
        assert_eq!(executor.extract_number(text, "Failed:"), Some(1));
        assert_eq!(executor.extract_number(text, "Skipped:"), None);
    }

    #[test]
    fn test_parse_simple_output() {
        let executor = TestExecutor::new(PathBuf::from("/tmp"));

        let output = CommandOutput {
            exit_code: 0,
            stdout: "Total Tests: 36\nPassed: 36\nFailed: 0\n".to_string(),
            stderr: String::new(),
            duration_ms: 1234,
        };

        let result = executor
            .parse_simple_output(&output, "shell", "script")
            .unwrap();

        assert_eq!(result.total, 36);
        assert_eq!(result.passed, 36);
        assert_eq!(result.failed, 0);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.duration_ms, 1234);
    }

    #[test]
    fn test_parse_simple_output_with_failures() {
        let executor = TestExecutor::new(PathBuf::from("/tmp"));

        let output = CommandOutput {
            exit_code: 1,
            stdout: "Total Tests: 10\nPassed: 8\nFailed: 2\n".to_string(),
            stderr: "Some tests failed\n".to_string(),
            duration_ms: 5000,
        };

        let result = executor
            .parse_simple_output(&output, "python", "unittest")
            .unwrap();

        assert_eq!(result.total, 10);
        assert_eq!(result.passed, 8);
        assert_eq!(result.failed, 2);
        assert_eq!(result.test_cases.len(), 1);
        assert_eq!(result.test_cases[0].status, TestStatus::Failed);
    }

    #[test]
    fn test_nonzero_exit_cannot_report_all_tests_passed() {
        let executor = TestExecutor::new(PathBuf::from("/tmp"));
        let output = CommandOutput {
            exit_code: 1,
            stdout: "Total Tests: 1\nPassed: 1\nFailed: 0\n".to_string(),
            stderr: String::new(),
            duration_ms: 1,
        };

        let suite = executor
            .parse_simple_output(&output, "script", "script")
            .unwrap();
        assert_eq!(suite.total, 1);
        assert_eq!(suite.passed, 0);
        assert_eq!(suite.failed, 1);
    }

    #[tokio::test]
    async fn test_commands_drain_and_bound_stdout_and_stderr() {
        let temp = tempfile::tempdir().unwrap();
        let executor = TestExecutor::new(temp.path().to_path_buf());
        let output = executor
            .run_command(
                "/bin/sh",
                &["-c".to_string(), "dd if=/dev/zero bs=1048576 count=2 status=none; dd if=/dev/zero bs=1048576 count=2 status=none >&2".to_string()],
                temp.path(),
                None,
                Duration::from_secs(10),
            )
            .await
            .unwrap();

        assert_eq!(output.stdout.len(), MAX_TEST_OUTPUT_BYTES);
        assert_eq!(output.stderr.len(), MAX_TEST_OUTPUT_BYTES);
    }

    #[tokio::test]
    async fn test_command_timeout_stops_process_group() {
        let temp = tempfile::tempdir().unwrap();
        let executor = TestExecutor::new(temp.path().to_path_buf());
        let started = Instant::now();
        let result = executor
            .run_command(
                "/bin/sh",
                &["-c".to_string(), "sleep 30 & wait".to_string()],
                temp.path(),
                None,
                Duration::from_millis(100),
            )
            .await;

        assert!(matches!(result, Err(Error::Timeout(_))));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn test_commands_do_not_inherit_service_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let secret_name = "ATTUNE_TEST_SECRET_SHOULD_NOT_LEAK";
        std::env::set_var(secret_name, "secret-value");

        let executor = TestExecutor::new(temp.path().to_path_buf());
        let result = executor
            .run_command(
                "/bin/sh",
                &[
                    "-c".to_string(),
                    format!("test -z \"${{{secret_name}:-}}\""),
                ],
                temp.path(),
                None,
                Duration::from_secs(5),
            )
            .await;

        std::env::remove_var(secret_name);
        assert!(result.is_ok(), "pack test inherited a service secret");
    }
}
