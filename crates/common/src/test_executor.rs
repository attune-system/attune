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
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, error, info, warn};

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
                        total: 0,
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
            .run_command(&command, &args, pack_dir, timeout_duration)
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
        timeout: Duration,
    ) -> Result<CommandOutput> {
        debug!(
            "Executing command: {} {} (timeout: {:?})",
            command,
            args.join(" "),
            timeout
        );

        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        let start = Instant::now();
        let mut child = cmd.spawn().map_err(|e| {
            Error::Internal(format!("Failed to spawn command '{}': {}", command, e))
        })?;

        // Wait for process with timeout
        let status = tokio::time::timeout(timeout, child.wait())
            .await
            .map_err(|_| Error::Timeout(format!("Test execution timed out after {:?}", timeout)))?
            .map_err(|e| Error::Internal(format!("Process wait failed: {}", e)))?;

        // Read output
        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        let stdout = if let Some(stdout) = stdout_handle {
            self.read_stream(stdout).await?
        } else {
            String::new()
        };

        let stderr = if let Some(stderr) = stderr_handle {
            self.read_stream(stderr).await?
        } else {
            String::new()
        };

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
    async fn read_stream(&self, stream: impl tokio::io::AsyncRead + Unpin) -> Result<String> {
        let mut reader = BufReader::new(stream);
        let mut output = String::new();
        let mut line = String::new();

        while reader
            .read_line(&mut line)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read stream: {}", e)))?
            > 0
        {
            output.push_str(&line);
            line.clear();
        }

        Ok(output)
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
}
