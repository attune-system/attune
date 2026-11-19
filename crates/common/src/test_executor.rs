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
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracing::{debug, error, info, warn};

const MAX_TEST_OUTPUT_BYTES: usize = 1024 * 1024;
/// Lossy UTF-8 can replace each retained raw byte with three text bytes. The
/// raw retention/drain policy remains exactly the existing one-MiB prefix.
pub const MAX_TEST_CAPTURE_TEXT_BYTES: usize = 3 * MAX_TEST_OUTPUT_BYTES;

/// Test configuration from pack.yaml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConfig {
    pub enabled: bool,
    #[serde(default)]
    pub candidate_api_effects: bool,
    pub discovery: DiscoveryConfig,
    pub runners: HashMap<String, RunnerConfig>,
    pub result_format: Option<String>,
    pub result_path: Option<String>,
    pub min_pass_rate: Option<f64>,
    pub on_failure: Option<String>,
}

impl TestConfig {
    pub fn required_recipe_digests(&self) -> Result<Vec<String>> {
        let mut digests: Vec<String> = self
            .runners
            .values()
            .filter_map(|runner| runner.lifecycle.as_ref())
            .map(|recipe| {
                recipe
                    .digest()
                    .map_err(|error| Error::validation(error.to_string()))
            })
            .collect::<Result<_>>()?;
        digests.sort();
        digests.dedup();
        Ok(digests)
    }

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
        for (name, runner) in &self.runners {
            if let Some(recipe) = &runner.lifecycle {
                if runner.r#type != "runtime" {
                    return Err(Error::validation(
                        "Lifecycle test runners must use type runtime",
                    ));
                }
                recipe
                    .validate()
                    .map_err(|error| Error::validation(error.to_string()))?;
            } else if runner.r#type == "runtime" {
                return Err(Error::validation(
                    "Runtime test runner requires a lifecycle recipe",
                ));
            }
            let timeout = runner.timeout.unwrap_or(300);
            if !(1..=3600).contains(&timeout) {
                return Err(crate::pack_registry::diagnostics::metadata_error(
                    "pack.yaml",
                    &format!(
                        "/testing/runners{}/timeout",
                        crate::pack_registry::diagnostics::pointer(name)
                    ),
                    "Runner timeout must be between 1 and 3600 seconds",
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<crate::runtime_lifecycle::RuntimeRecipe>,
    pub r#type: String,
    pub entry_point: String,
    pub timeout: Option<u64>,
    pub result_format: Option<String>,
}

/// Test executor for running pack tests
pub struct TestExecutor {
    lifecycle_environment: crate::child_process_environment::ChildProcessEnvironment,
    lifecycle_root: Option<PathBuf>,
    /// Base directory for pack files
    pack_base_dir: PathBuf,
    candidate_effects: Option<CandidateTestEnvironment>,
    processes: TestProcessOwner,
    cancellation: CancellationToken,
    workload_sandbox: Option<crate::workload_sandbox::WorkloadSandbox>,
    process_effect: Option<std::sync::Arc<dyn TestProcessEffect>>,
}

/// Owns subprocess supervisors independently of the future consuming their output.
/// After stopping admission/aborting consumers, explicitly await `shutdown` before
/// removing their working directories. Dropping this owner is not a cleanup barrier.
#[derive(Clone, Default)]
pub struct TestProcessOwner {
    tasks: TaskTracker,
}

impl TestProcessOwner {
    pub async fn shutdown(&self) {
        self.tasks.close();
        self.tasks.wait().await;
    }
}

/// Separate from ATTUNE_API_TOKEN and archive-download credentials.
/// No Debug implementation because this contains a runtime secret.
pub struct CandidateTestEnvironment {
    pub url: String,
    pub token: String,
}

#[derive(Debug, thiserror::Error)]
enum SuiteExecutionError {
    #[error(transparent)]
    Effect(Error),
    #[error(transparent)]
    Suite(Error),
}
impl From<Error> for SuiteExecutionError {
    fn from(error: Error) -> Self {
        Self::Suite(error)
    }
}

/// Typed owning-loop evidence; never guessed from a guest exit number.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TestProcessInterruption {
    PreparationUnverified,
    TimedOut,
    Cancelled,
    SourceRevoked,
    InputUncertain,
    ProviderFailure,
    OutOfMemory,
}

/// An owning native manager may replace only the process effect. Discovery,
/// entrypoint validation, result parsing and pass/failure policy stay here.
/// Errors mean incomplete effects/capture and must not become publishable results.
#[async_trait::async_trait]
pub trait TestProcessEffect: Send + Sync {
    async fn run(&self, request: TestProcessRequest) -> Result<CommandOutput>;
}
pub struct TestProcessRequest {
    pub suite: String,
    pub runner: RunnerConfig,
    pub entry_point: PathBuf,
    pub candidate_effects: Option<CandidateTestEnvironment>,
    pub cancellation: CancellationToken,
}

impl TestExecutor {
    /// Create a new test executor
    pub fn new(pack_base_dir: PathBuf) -> Self {
        Self {
            pack_base_dir,
            lifecycle_environment: Default::default(),
            lifecycle_root: None,
            candidate_effects: None,
            processes: TestProcessOwner::default(),
            cancellation: CancellationToken::new(),
            workload_sandbox: None,
            process_effect: None,
        }
    }

    pub fn with_workload_sandbox(
        mut self,
        sandbox: crate::workload_sandbox::WorkloadSandbox,
    ) -> Self {
        self.workload_sandbox = Some(sandbox);
        self.lifecycle_environment = self.lifecycle_environment.with_workload_sandbox(sandbox);
        self
    }

    pub fn with_process_effect(mut self, effect: std::sync::Arc<dyn TestProcessEffect>) -> Self {
        self.process_effect = Some(effect);
        self
    }

    pub fn process_owner(&self) -> TestProcessOwner {
        self.processes.clone()
    }

    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub fn with_runtime_lifecycle(
        mut self,
        environment: crate::child_process_environment::ChildProcessEnvironment,
        root: PathBuf,
    ) -> Self {
        self.lifecycle_environment = match self.workload_sandbox {
            Some(sandbox) => environment.with_workload_sandbox(sandbox),
            None => environment,
        };
        self.lifecycle_root = Some(root);
        self
    }

    pub fn with_candidate_effects(mut self, environment: CandidateTestEnvironment) -> Self {
        self.candidate_effects = Some(environment);
        self
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
        if test_config.candidate_api_effects != self.candidate_effects.is_some() {
            return Err(Error::validation("Candidate API effects require an isolated server-owned context; ordinary tests cannot receive one"));
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
            if self.cancellation.is_cancelled() {
                if self.process_effect.is_some() {
                    test_suites.push(Self::failed_suite(
                        runner_name,
                        runner_config,
                        "Pack tests cancelled before runner start",
                    ));
                    continue;
                }
                return Err(Error::Internal("Pack tests cancelled".into()));
            }
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
                Err(SuiteExecutionError::Effect(e)) => return Err(e),
                Err(SuiteExecutionError::Suite(e)) => {
                    error!("Test suite '{}' failed to execute: {}", runner_name, e);
                    // Create a failed suite result
                    test_suites.push(Self::failed_suite(
                        runner_name,
                        runner_config,
                        &e.to_string(),
                    ));
                }
            }
        }

        // Cancellation is an interrupted attempt, even when the pack's failure
        // policy would otherwise permit failed/error suites to be published.
        if self.cancellation.is_cancelled() && self.process_effect.is_none() {
            return Err(Error::Internal("Pack tests cancelled".into()));
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

    fn failed_suite(
        runner_name: &str,
        runner_config: &RunnerConfig,
        error: &str,
    ) -> TestSuiteResult {
        TestSuiteResult {
            name: runner_name.into(),
            runner_type: runner_config.r#type.clone(),
            total: 1,
            passed: 0,
            failed: 1,
            skipped: 0,
            duration_ms: 0,
            test_cases: vec![TestCaseResult {
                name: format!("{runner_name}_execution"),
                status: TestStatus::Error,
                duration_ms: 0,
                error_message: Some(error.into()),
                stdout: None,
                stderr: None,
            }],
        }
    }

    /// Execute a single test suite
    async fn execute_test_suite(
        &self,
        pack_dir: &Path,
        runner_name: &str,
        runner_config: &RunnerConfig,
        python_interpreter: Option<&Path>,
    ) -> std::result::Result<TestSuiteResult, SuiteExecutionError> {
        let start_time = Instant::now();
        let suite_deadline =
            tokio::time::Instant::now() + Duration::from_secs(runner_config.timeout.unwrap_or(300));

        let entry_point =
            confined_test_entrypoint(pack_dir, &runner_config.entry_point).map_err(|_| {
                crate::pack_registry::diagnostics::metadata_error(
                    "pack.yaml",
                    &format!(
                        "/testing/runners{}/entry_point",
                        crate::pack_registry::diagnostics::pointer(runner_name)
                    ),
                    "Test entrypoint must name an existing file within the pack",
                )
            })?;
        // Absolute canonical paths also prevent a leading '-' from becoming an option.
        let relative_entry_point = entry_point.to_string_lossy().into_owned();

        if !matches!(
            runner_config.r#type.as_str(),
            "script" | "unittest" | "pytest" | "runtime"
        ) {
            return Err(Error::validation(format!(
                "Unsupported runner type: {}",
                runner_config.r#type
            ))
            .into());
        }
        if let Some(effect) = &self.process_effect {
            let entry_point = entry_point
                .strip_prefix(pack_dir)
                .map_err(|_| Error::validation("Candidate entrypoint escaped verified root"))?
                .to_path_buf();
            let request = TestProcessRequest {
                suite: runner_name.into(),
                runner: runner_config.clone(),
                entry_point,
                candidate_effects: self.candidate_effects.as_ref().map(|e| {
                    CandidateTestEnvironment {
                        url: e.url.clone(),
                        token: e.token.clone(),
                    }
                }),
                cancellation: self.cancellation.clone(),
            };
            let effect = effect.clone();
            // This task outlives an aborted consumer. The ordinary handler must
            // await process_owner.shutdown before touching leases or roots.
            let output = self
                .processes
                .tasks
                .spawn(async move { effect.run(request).await })
                .await
                .map_err(|_| {
                    SuiteExecutionError::Effect(Error::validation(
                        "Native test effect owner failed",
                    ))
                })?
                .map_err(SuiteExecutionError::Effect)?;
            return self
                .parse_suite_output(&output, runner_name, runner_config, start_time)
                .map_err(Into::into);
        }

        let target = if let Some(recipe) = &runner_config.lifecycle {
            self.lifecycle_environment
                .admits_runtime_recipe(
                    &recipe
                        .digest()
                        .map_err(|error| Error::validation(error.to_string()))?,
                )
                .then_some(())
                .ok_or_else(|| {
                    Error::validation("Runtime recipe is not admitted by this process")
                })?;
            let root = self.lifecycle_root.as_deref().ok_or_else(|| {
                Error::validation(
                    "Runtime test preparation requires an operator-owned generation root",
                )
            })?;
            let preparation = tokio::time::timeout_at(
                suite_deadline,
                recipe.prepare(
                    pack_dir,
                    crate::runtime_lifecycle::RuntimeSource::File(&entry_point),
                    root,
                    &self.lifecycle_environment,
                ),
            );
            Some(tokio::select! {
                result = preparation => result.map_err(|_| Error::Timeout("Runtime test preparation exceeded runner timeout".into()))?
                    .map_err(|error| Error::validation(error.to_string()))?,
                _ = self.cancellation.cancelled() => return Err(Error::validation("Runtime test preparation cancelled").into()),
            })
        } else {
            None
        };

        let (command, args) = match runner_config.r#type.as_str() {
            "runtime" => {
                let target = target
                    .as_ref()
                    .ok_or_else(|| Error::validation("Runtime test target is missing"))?;
                (target.command[0].clone(), target.command[1..].to_vec())
            }
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
                ))
                .into());
            }
        };

        // Execute test command with pack_dir as working directory
        let timeout_seconds = runner_config.timeout.unwrap_or(300);
        let timeout_seconds = runner_config
            .lifecycle
            .as_ref()
            .map(|recipe| timeout_seconds.min(recipe.launch.timeout_seconds))
            .unwrap_or(timeout_seconds);
        let timeout_duration = Duration::from_secs(timeout_seconds)
            .min(suite_deadline.saturating_duration_since(tokio::time::Instant::now()));
        let outcome = if target.is_some() {
            self.run_command_prepared(
                &command,
                &args,
                pack_dir,
                python_interpreter.and_then(Path::parent),
                timeout_duration,
                target.as_ref(),
            )
            .await
        } else {
            self.run_command(
                &command,
                &args,
                pack_dir,
                python_interpreter.and_then(Path::parent),
                timeout_duration,
            )
            .await
        };
        if outcome.is_ok() {
            if let Some(target) = target {
                target
                    .cleanup()
                    .await
                    .map_err(|error| Error::validation(error.to_string()))?;
            }
        }
        let output = outcome?;

        self.parse_suite_output(&output, runner_name, runner_config, start_time)
            .map_err(Into::into)
    }

    fn parse_suite_output(
        &self,
        output: &CommandOutput,
        runner_name: &str,
        runner_config: &RunnerConfig,
        start_time: Instant,
    ) -> Result<TestSuiteResult> {
        let duration_ms = start_time.elapsed().as_millis() as i64;
        // Parse output based on result format
        let result_format = runner_config.result_format.as_deref().unwrap_or("simple");

        let mut suite_result = match result_format {
            "simple" => self.parse_simple_output(output, runner_name, &runner_config.r#type)?,
            "json" => self.parse_json_output(&output.stdout, runner_name)?,
            _ => {
                warn!(
                    "Unknown result format '{}', falling back to simple",
                    result_format
                );
                self.parse_simple_output(output, runner_name, &runner_config.r#type)?
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
        self.run_command_prepared(command, args, working_dir, runtime_bin_dir, timeout, None)
            .await
    }

    async fn run_command_prepared(
        &self,
        command: &str,
        args: &[String],
        working_dir: &Path,
        runtime_bin_dir: Option<&Path>,
        timeout: Duration,
        target: Option<&crate::runtime_lifecycle::PreparedTarget>,
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
        crate::child_process_environment::ChildProcessEnvironment::from_parent(
            std::iter::empty(),
            &[],
        )?
        .apply(cmd.as_std_mut());
        cmd.env("PATH", path)
            .env("HOME", working_dir)
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .args(args)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        cmd.kill_on_drop(true);
        if let Some(target) = target {
            self.lifecycle_environment.apply(cmd.as_std_mut());
            cmd.envs(&target.env).current_dir(&target.working_dir);
        }
        if let Some(candidate) = &self.candidate_effects {
            cmd.env("ATTUNE_CANDIDATE_EFFECTS_URL", &candidate.url)
                .env("ATTUNE_CANDIDATE_EFFECTS_TOKEN", &candidate.token);
        }
        let runtime = runtime_bin_dir.and_then(Path::parent);
        let mut reads = vec![working_dir];
        if let Some(runtime) = runtime {
            reads.push(runtime);
        }
        let sandbox = match self.workload_sandbox {
            Some(sandbox) => sandbox,
            None => crate::workload_sandbox::WorkloadSandbox::from_environment(true)
                .map_err(|error| Error::validation(error.to_string()))?,
        };
        let workspace = if matches!(
            sandbox,
            crate::workload_sandbox::WorkloadSandbox::Bubblewrap
        ) {
            if working_dir
                .canonicalize()
                .map_err(|error| Error::io(error.to_string()))?
                != working_dir
            {
                return Err(Error::validation(
                    "Candidate workspace must be a canonical path",
                ));
            }
            let workspace = tempfile::tempdir().map_err(|error| Error::io(error.to_string()))?;
            let prepared = copy_private_test_tree(working_dir, workspace.path()).and_then(|()| {
                crate::workload_sandbox::prepare_staging(workspace.path(), working_dir)
            });
            if let Err(error) = prepared {
                workspace
                    .close()
                    .map_err(|error| Error::io(error.to_string()))?;
                return Err(Error::io(error.to_string()));
            }
            Some(workspace)
        } else {
            None
        };
        let wrapped = if let Some(workspace) = &workspace {
            sandbox.wrap_workspace(cmd.into_std(), &reads, workspace.path(), working_dir)
        } else {
            sandbox.wrap(cmd.into_std(), &reads)
        };
        let mut cmd = match wrapped {
            Ok(command) => Command::from(command),
            Err(error) => {
                if let Some(workspace) = workspace {
                    workspace
                        .close()
                        .map_err(|error| Error::io(error.to_string()))?;
                }
                return Err(Error::validation(error.to_string()));
            }
        };
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);

        // No await between spawning and transferring ownership to the tracker.
        // Closing the sender signals cancellation even when the consuming task is aborted.
        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                if let Some(workspace) = workspace {
                    workspace
                        .close()
                        .map_err(|error| Error::io(error.to_string()))?;
                }
                return Err(Error::Internal(format!(
                    "Failed to spawn command '{command}': {error}"
                )));
            }
        };
        let (consumer, cancelled) = tokio::sync::oneshot::channel::<()>();
        let cancellation = self.cancellation.clone();
        let require_group_exit = target.is_some();
        let task = self.processes.tasks.spawn(async move {
            let result =
                Self::collect_command(child, timeout, cancelled, cancellation, require_group_exit)
                    .await;
            if let Some(workspace) = workspace {
                workspace.close().map_err(|error| {
                    Error::io(format!("Test workspace cleanup failed: {error}"))
                })?;
            }
            if let Err(error) = &result {
                // The consuming future may have been aborted, so also surface
                // failures here rather than relying on its JoinHandle result.
                warn!(%error, "Pack test subprocess supervisor completed with an error");
            }
            result
        });
        let result = task
            .await
            .map_err(|error| Error::Internal(format!("Test process supervisor failed: {error}")))?;
        drop(consumer);
        result
    }

    async fn collect_command(
        mut child: tokio::process::Child,
        timeout: Duration,
        mut cancelled: tokio::sync::oneshot::Receiver<()>,
        cancellation: CancellationToken,
        require_group_exit: bool,
    ) -> Result<CommandOutput> {
        let start = Instant::now();
        #[cfg(unix)]
        let group = child.id();
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        // Readers are inline futures, never detached tasks. Keep them alive when
        // cancelling the wait so cleanup can drain both after killing the group.
        let readers = async { tokio::join!(Self::read_stream(stdout), Self::read_stream(stderr)) };
        tokio::pin!(readers);
        let mut captured = None;
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        let result = loop {
            tokio::select! {
                status = child.wait() => break status.map_err(|error| Error::Internal(format!("Process wait failed: {error}"))),
                output = &mut readers, if captured.is_none() => captured = Some(output),
                _ = &mut deadline => break Err(Error::Timeout(format!("Test execution timed out after {timeout:?}"))),
                _ = &mut cancelled => break Err(Error::Internal("Pack test consumer cancelled".into())),
                _ = cancellation.cancelled() => break Err(Error::Internal("Pack tests cancelled".into())),
            }
        };
        // Also clean up descendants after a normal leader exit. On Windows we
        // own only the direct child; process-tree containment needs Job Objects.
        #[cfg(unix)]
        if let Some(pid) = group {
            let killed = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
            if killed != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    warn!(%error, "Failed to kill pack test process group");
                }
            }
        }
        if result.is_err() {
            if let Err(error) = child.start_kill() {
                warn!(%error, "Failed to kill pack test child; awaiting reap");
            }
        }
        // Always reap before reporting cancellation/timeout to the caller.
        child
            .wait()
            .await
            .map_err(|error| Error::Internal(format!("Process reap failed: {error}")))?;
        let (stdout, stderr) = match captured {
            Some(output) => output,
            None => {
                // Escaped groups / Windows descendants can retain the pipes.
                // Expiry drops the inline readers, closing both handles; no task survives.
                tokio::time::timeout(Duration::from_secs(2), &mut readers)
                    .await
                    .map_err(|_| {
                        Error::Internal(
                            "Pack test output pipes did not close after termination".into(),
                        )
                    })?
            }
        };
        if require_group_exit {
            #[cfg(unix)]
            crate::runtime_lifecycle::stop_owned_process_group(group)
                .await
                .map_err(|error| Error::validation(error.to_string()))?;
            #[cfg(not(unix))]
            return Err(Error::validation(
                "Recipe process-tree teardown is unavailable on this platform",
            ));
        }
        let status = result?;
        Ok(CommandOutput {
            interruption: None,
            exit_code: status.code().unwrap_or(-1),
            stdout: stdout?,
            stderr: stderr?,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Read from an async stream
    pub async fn read_stream(mut stream: impl tokio::io::AsyncRead + Unpin) -> Result<String> {
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
        if (output.exit_code != 0 || output.interruption.is_some()) && failed == 0 {
            failed = 1;
            total = total.max(failed + skipped);
            passed = passed.min(total.saturating_sub(failed + skipped));
        }

        // Create a single test case representing the entire suite
        let test_case = TestCaseResult {
            name: format!("{}_suite", runner_name),
            status: if output.interruption.is_some() {
                TestStatus::Error
            } else if output.exit_code == 0 {
                TestStatus::Passed
            } else {
                TestStatus::Failed
            },
            duration_ms: output.duration_ms as i64,
            error_message: if let Some(reason) = output.interruption {
                Some(format!(
                    "{reason:?}; observed exit code: {}",
                    output.exit_code
                ))
            } else if output.exit_code != 0 {
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
pub struct CommandOutput {
    pub interruption: Option<TestProcessInterruption>,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

/// Resolve only ordinary relative file paths inside the owned pack tree.
/// Pack sources must remain immutable while tests run.
pub(crate) fn confined_test_entrypoint(root: &Path, entry: &str) -> Result<PathBuf> {
    let path = Path::new(entry);
    if entry.is_empty()
        || path.components().any(|part| {
            !matches!(
                part,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err(Error::Validation("Unsafe test entrypoint".into()));
    }
    let root = root
        .canonicalize()
        .map_err(|error| Error::Io(error.to_string()))?;
    let resolved = root
        .join(path)
        .canonicalize()
        .map_err(|error| Error::Io(error.to_string()))?;
    if !resolved.starts_with(&root) || !resolved.is_file() {
        return Err(Error::Validation(
            "Test entrypoint escapes pack root".into(),
        ));
    }
    Ok(resolved)
}

/// Copy executable test inputs without following host links. The sandbox mounts
/// only this owned copy writable; publication inputs and runtime stay read-only.
fn copy_private_test_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source = entry.path();
        let destination = destination.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(&source)?;
        if metadata.file_type().is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(&source)?, destination)?;
            #[cfg(not(unix))]
            return Err(std::io::Error::other("Isolated test links require Unix"));
        } else if metadata.is_dir() {
            std::fs::create_dir(&destination)?;
            copy_private_test_tree(&source, &destination)?;
        } else if metadata.is_file() {
            std::fs::copy(&source, &destination)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = metadata.permissions().mode() | 0o200;
                std::fs::set_permissions(destination, std::fs::Permissions::from_mode(mode))?;
            }
        } else {
            return Err(std::io::Error::other(
                "Unsupported candidate test input type",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[ignore = "requires Linux bubblewrap and unprivileged namespaces"]
    async fn isolated_candidate_writes_private_copy_and_preserves_inputs() {
        let root = tempfile::tempdir().unwrap();
        let pack = root.path().join("candidate");
        std::fs::create_dir(&pack).unwrap();
        std::fs::write(pack.join("input"), "original").unwrap();
        let hidden = root.path().join("manager-token");
        std::fs::write(&hidden, "owned-dummy-host-token").unwrap();
        let mut dummy = std::process::Command::new("/bin/sleep")
            .arg("60")
            .env_clear()
            .env("OWNED_DUMMY_CREDENTIAL", "not-for-candidate")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = dummy.id();
        let executor = super::TestExecutor::new(root.path().to_path_buf())
            .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::Bubblewrap)
            .with_candidate_effects(CandidateTestEnvironment {
                url: "https://api/candidate".into(),
                token: "candidate-only".into(),
            });
        let script = format!("set -eu; test ! -e '{}'; test ! -e /proc/{pid}/environ; ! kill -TERM {pid} 2>/dev/null; test \"$ATTUNE_CANDIDATE_EFFECTS_TOKEN\" = candidate-only; test -z \"${{ATTUNE_API_TOKEN:-}}\"; echo modified > input; mkdir output; echo generated > output/result; echo test-ok", hidden.display());
        let result = executor
            .run_command(
                "/bin/sh",
                &["-c".into(), script],
                &pack,
                None,
                std::time::Duration::from_secs(10),
            )
            .await;
        executor.process_owner().shutdown().await;
        let survived = dummy.try_wait().unwrap().is_none();
        if survived {
            dummy.kill().unwrap();
        }
        dummy.wait().unwrap();
        let output = result.unwrap();
        assert_eq!(output.stdout.trim(), "test-ok");
        assert_eq!(output.exit_code, 0);
        assert!(
            survived,
            "candidate signaled the owned host credential process"
        );
        assert_eq!(
            std::fs::read_to_string(hidden).unwrap(),
            "owned-dummy-host-token"
        );
        assert_eq!(
            std::fs::read_to_string(pack.join("input")).unwrap(),
            "original"
        );
        assert!(!pack.join("output").exists());
    }

    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn runner_entrypoint_paths_and_candidate_tree_are_validated_before_execution() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let root = temp_root.join("pack");
        std::fs::create_dir(&root).unwrap();
        let marker = temp_root.join("executed");
        let outside = temp_root.join("outside.sh");
        std::fs::write(&outside, format!("touch '{}'\n", marker.display())).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape.sh")).unwrap();
        std::os::unix::fs::symlink(&temp_root, root.join("escape-dir")).unwrap();
        std::fs::write(
            root.join("safe.sh"),
            "printf 'Total Tests: 1\nPassed: 1\nFailed: 0\n'\n",
        )
        .unwrap();
        // Path/tree validation and output parsing use an explicit process
        // fixture. Real confinement is required in the isolated_* host lane.
        let executor = TestExecutor::new(root.clone())
            .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::Disabled);
        for entry in [
            "../outside.sh",
            outside.to_str().unwrap(),
            "escape.sh",
            "escape-dir/outside.sh",
        ] {
            let runner = RunnerConfig {
                lifecycle: None,
                r#type: "script".into(),
                entry_point: entry.into(),
                timeout: Some(5),
                result_format: None,
            };
            assert!(
                executor
                    .execute_test_suite(&root, "escape", &runner, None)
                    .await
                    .is_err(),
                "{entry}"
            );
            assert!(!marker.exists());
        }
        let runner = RunnerConfig {
            lifecycle: None,
            r#type: "script".into(),
            entry_point: "safe.sh".into(),
            timeout: Some(5),
            result_format: None,
        };
        let private_copy = temp_root.join("private-copy");
        std::fs::create_dir(&private_copy).unwrap();
        // Copying preserves links without following them. The owning caller's
        // complete preparation step rewrites internal links and rejects unsafe
        // links before a process can start.
        copy_private_test_tree(&root, &private_copy).unwrap();
        assert!(crate::workload_sandbox::prepare_staging(&private_copy, &root).is_err());
        assert!(!marker.exists());
        std::fs::remove_file(root.join("escape.sh")).unwrap();
        std::fs::remove_file(root.join("escape-dir")).unwrap();

        std::fs::write(root.join("copied-input"), "published").unwrap();
        std::os::unix::fs::symlink("copied-input", root.join("relative-link")).unwrap();
        std::os::unix::fs::symlink(root.join("copied-input"), root.join("absolute-link")).unwrap();
        let safe_copy = temp_root.join("safe-copy");
        std::fs::create_dir(&safe_copy).unwrap();
        copy_private_test_tree(&root, &safe_copy).unwrap();
        crate::workload_sandbox::prepare_staging(&safe_copy, &root).unwrap();
        assert!(safe_copy
            .join("absolute-link")
            .canonicalize()
            .unwrap()
            .starts_with(&safe_copy));
        std::fs::write(safe_copy.join("absolute-link"), "private").unwrap();
        assert_eq!(
            std::fs::read_to_string(safe_copy.join("relative-link")).unwrap(),
            "private"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("copied-input")).unwrap(),
            "published"
        );
        let safe = executor
            .execute_test_suite(&root, "safe", &runner, None)
            .await;
        executor.process_owner().shutdown().await;
        assert_eq!(safe.unwrap().passed, 1);
    }

    #[tokio::test]
    async fn candidate_credentials_are_explicit_and_not_general_execution_credentials() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("run.sh"),
            "set -eu\ntest \"$ATTUNE_CANDIDATE_EFFECTS_URL\" = https://api/candidate\ntest \"$ATTUNE_CANDIDATE_EFFECTS_TOKEN\" = candidate-only\ntest -z \"${ATTUNE_API_TOKEN:-}\"\ntest -z \"${ATTUNE_CANDIDATE_ACCESS_TOKEN:-}\"\nprintf 'Total Tests: 1\\nPassed: 1\\nFailed: 0\\n'\n").unwrap();
        let mut config = test_config(None, None);
        config.candidate_api_effects = true;
        config.runners.insert(
            "isolated".into(),
            RunnerConfig {
                lifecycle: None,
                r#type: "script".into(),
                entry_point: "run.sh".into(),
                timeout: Some(10),
                result_format: None,
            },
        );
        let plain = TestExecutor::new(root.path().to_path_buf())
            .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::Disabled);
        assert!(plain
            .execute_pack_tests_at(root.path(), "candidate", "1.0.0", &config)
            .await
            .is_err());
        let selected = TestExecutor::new(root.path().to_path_buf())
            .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::Disabled)
            .with_candidate_effects(CandidateTestEnvironment {
                url: "https://api/candidate".into(),
                token: "candidate-only".into(),
            });
        let result = selected
            .execute_pack_tests_at(root.path(), "candidate", "1.0.0", &config)
            .await;
        selected.process_owner().shutdown().await;
        assert_eq!(result.unwrap().passed, 1);
        config.candidate_api_effects = false;
        assert!(selected
            .execute_pack_tests_at(root.path(), "candidate", "1.0.0", &config)
            .await
            .is_err());
    }

    fn test_config(min_pass_rate: Option<f64>, on_failure: Option<&str>) -> TestConfig {
        TestConfig {
            candidate_api_effects: false,
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
                lifecycle: None,
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
            interruption: None,
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
            interruption: None,
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
            interruption: None,
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
        let executor = TestExecutor::new(temp.path().to_path_buf())
            .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::Disabled);
        let output = executor
            .run_command(
                "/bin/sh",
                &["-c".to_string(), "dd if=/dev/zero bs=1048576 count=2 status=none; dd if=/dev/zero bs=1048576 count=2 status=none >&2".to_string()],
                temp.path(),
                None,
                Duration::from_secs(10),
            )
            .await;
        executor.process_owner().shutdown().await;
        let output = output.unwrap();
        assert_eq!(output.stdout.len(), MAX_TEST_OUTPUT_BYTES);
        assert_eq!(output.stderr.len(), MAX_TEST_OUTPUT_BYTES);
    }

    #[tokio::test]
    async fn test_command_timeout_stops_process_group() {
        let temp = tempfile::tempdir().unwrap();
        let executor = TestExecutor::new(temp.path().to_path_buf())
            .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::Disabled);
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

        executor.process_owner().shutdown().await;
        assert!(matches!(result, Err(Error::Timeout(_))));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    #[ignore = "requires Linux bubblewrap and unprivileged namespaces"]
    async fn isolated_missing_executable_exits_nonzero_and_drains_owner() {
        let root = tempfile::tempdir().unwrap();
        let executor = TestExecutor::new(root.path().into())
            .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::Bubblewrap);
        let owner = executor.process_owner();
        let result = executor
            .run_command(
                &root.path().join("missing").to_string_lossy(),
                &[],
                root.path(),
                None,
                Duration::from_secs(5),
            )
            .await;
        owner.shutdown().await;
        assert_ne!(result.unwrap().exit_code, 0);
        assert!(owner.tasks.is_empty());
        root.close().unwrap();
    }

    #[tokio::test]
    async fn test_spawn_failure_leaves_no_owned_process() {
        let root = tempfile::tempdir().unwrap();
        let executor = TestExecutor::new(root.path().into())
            .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::Disabled);
        let owner = executor.process_owner();
        assert!(executor
            .run_command(
                &root.path().join("missing-executable").to_string_lossy(),
                &[],
                root.path(),
                None,
                Duration::from_secs(5),
            )
            .await
            .is_err());
        owner.shutdown().await;
        assert!(owner.tasks.is_empty());
        root.close().unwrap();
    }

    #[cfg(target_os = "linux")]
    async fn descendant_ready(root: &Path) -> i32 {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(pid) = tokio::fs::read_to_string(root.join("ready")).await {
                    if let Ok(pid) = pid.trim().parse() {
                        return pid;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("descendant readiness signal")
    }

    #[cfg(target_os = "linux")]
    async fn assert_descendant_stopped(pid: i32) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match tokio::fs::read_to_string(format!("/proc/{pid}/stat")).await {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                    Ok(stat)
                        if stat
                            .rsplit_once(')')
                            .unwrap()
                            .1
                            .trim_start()
                            .starts_with('Z') =>
                    {
                        break
                    }
                    _ => tokio::task::yield_now().await,
                }
            }
        })
        .await
        .expect("descendant must terminate (an orphan zombie cannot retain pipes)");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn test_aborted_consumer_awaits_process_and_readers_before_root_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_path_buf();
        let executor = TestExecutor::new(path.clone())
            .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::Disabled);
        let processes = executor.process_owner();
        let consumer = tokio::spawn(async move {
            executor
                .run_command(
                    "/bin/sh",
                    &["-c".into(), "sleep 60 & echo $! > ready; wait".into()],
                    &path,
                    None,
                    Duration::from_secs(30),
                )
                .await
        });
        let pid = descendant_ready(root.path()).await;
        consumer.abort();
        assert!(consumer.await.unwrap_err().is_cancelled());
        // This is the cleanup barrier, not the abort/join of the consumer.
        tokio::time::timeout(Duration::from_secs(10), processes.shutdown())
            .await
            .unwrap();
        assert_descendant_stopped(pid).await;
        assert!(root.path().exists());
        root.close().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn test_cancelled_pack_runner_retains_root_until_owned_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_path_buf();
        std::fs::write(path.join("test.sh"), "sleep 60 & echo $! > ready; wait").unwrap();
        let cancellation = CancellationToken::new();
        let consumer_guard = cancellation.clone().drop_guard();
        let tasks = TaskTracker::new();
        let (finished, completion) = tokio::sync::oneshot::channel();
        let handler_path = path.clone();
        let handler = tasks.spawn(async move {
            let path = handler_path;
            let executor = TestExecutor::new(path.clone())
                .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::Disabled)
                .with_cancellation(cancellation);
            let config = TestConfig {
                enabled: true,
                candidate_api_effects: false,
                discovery: DiscoveryConfig {
                    method: "explicit".into(),
                    path: None,
                },
                runners: HashMap::from([(
                    "script".into(),
                    RunnerConfig {
                        lifecycle: None,
                        r#type: "script".into(),
                        entry_point: "test.sh".into(),
                        timeout: Some(30),
                        result_format: None,
                    },
                )]),
                result_format: None,
                result_path: None,
                min_pass_rate: None,
                on_failure: Some("warn".into()),
            };
            let result = executor
                .execute_pack_tests_at(&path, "candidate", "1.0.0", &config)
                .await;
            assert!(path.exists(), "candidate root must survive runner cleanup");
            root.close().unwrap();
            finished.send(result).unwrap();
        });
        let consumer = tokio::spawn(async move {
            let _guard = consumer_guard;
            handler.await.unwrap();
        });
        let pid = descendant_ready(&path).await;
        consumer.abort();
        assert!(consumer.await.unwrap_err().is_cancelled());
        tasks.close();
        tokio::time::timeout(Duration::from_secs(10), tasks.wait())
            .await
            .unwrap();
        let result = completion.await.unwrap();
        assert!(
            result.is_err(),
            "cancellation must not become an acceptable suite result"
        );
        assert_descendant_stopped(pid).await;
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_normal_leader_exit_terminates_descendants_holding_pipes() {
        let root = tempfile::tempdir().unwrap();
        let executor = TestExecutor::new(root.path().into())
            .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::Disabled);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            executor.run_command(
                "/bin/sh",
                &[
                    "-c".into(),
                    "sleep 60 & printf done; printf error >&2".into(),
                ],
                root.path(),
                None,
                Duration::from_secs(30),
            ),
        )
        .await;
        executor.process_owner().shutdown().await;
        let result = result.unwrap().unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "done");
        assert_eq!(result.stderr, "error");
        root.close().unwrap();
    }

    #[tokio::test]
    async fn test_commands_do_not_inherit_service_secrets() {
        const CHILD: &str = "ATTUNE_TESTEXEC_ENV_FIXTURE_CHILD";
        const SECRET: &str = "ATTUNE_TEST_SECRET_SHOULD_NOT_LEAK";
        if std::env::var(CHILD).as_deref() != Ok("owned-fixture") {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "test_executor::tests::test_commands_do_not_inherit_service_secrets",
                    "--nocapture",
                    "--test-threads=4",
                ])
                .env_clear()
                .env(CHILD, "owned-fixture")
                .env(SECRET, "owned-dummy-service-secret")
                .output()
                .await
                .unwrap();
            assert!(output.status.success(), "owned environment fixture failed");
            assert!(!String::from_utf8_lossy(&output.stdout).contains("owned-dummy-service-secret"));
            return;
        }
        assert_eq!(
            std::env::var(SECRET).as_deref(),
            Ok("owned-dummy-service-secret")
        );
        let temp = tempfile::tempdir().unwrap();
        let executor = TestExecutor::new(temp.path().to_path_buf())
            .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::Disabled);
        let result = executor
            .run_command(
                "/bin/sh",
                &["-c".into(), format!("test -z \"${{{SECRET}:-}}\"")],
                temp.path(),
                None,
                Duration::from_secs(5),
            )
            .await;
        executor.process_owner().shutdown().await;
        assert_eq!(
            result.unwrap().exit_code,
            0,
            "pack test inherited a service secret"
        );
    }
}

#[cfg(test)]
mod native_effect_tests {
    use super::*;
    use std::sync::Arc;
    struct OutputEffect {
        fail: bool,
    }
    #[async_trait::async_trait]
    impl TestProcessEffect for OutputEffect {
        async fn run(&self, request: TestProcessRequest) -> Result<CommandOutput> {
            assert_eq!(request.entry_point, Path::new("run.sh"));
            assert!(request.candidate_effects.is_none());
            if self.fail {
                return Err(Error::validation("unknown provider settlement"));
            }
            Ok(CommandOutput {
                interruption: None,
                exit_code: 1,
                stdout: "Total Tests: 4\nPassed: 3\nFailed: 1\n".into(),
                stderr: "separate error stream".into(),
                duration_ms: 17,
            })
        }
    }
    fn config() -> TestConfig {
        TestConfig {
            enabled: true,
            candidate_api_effects: false,
            discovery: DiscoveryConfig {
                method: "directory".into(),
                path: Some(".".into()),
            },
            runners: HashMap::from([(
                "native".into(),
                RunnerConfig {
                    lifecycle: None,
                    r#type: "script".into(),
                    entry_point: "run.sh".into(),
                    timeout: Some(5),
                    result_format: None,
                },
            )]),
            result_format: None,
            result_path: None,
            min_pass_rate: Some(0.75),
            on_failure: Some("ignore".into()),
        }
    }
    #[tokio::test]
    async fn native_effect_reuses_parser_and_policy_without_local_launch() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("run.sh"), "never run locally").unwrap();
        let executor = TestExecutor::new(root.path().into())
            .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::NativeEffectsRequired)
            .with_process_effect(Arc::new(OutputEffect { fail: false }));
        let cfg = config();
        let result = executor
            .execute_pack_tests_at(root.path(), "fixture", "1", &cfg)
            .await
            .unwrap();
        executor.process_owner().shutdown().await;
        assert_eq!(
            (result.total_tests, result.passed, result.failed),
            (4, 3, 1)
        );
        assert!(cfg.accepts_result(&result));
        assert_eq!(
            result.test_suites[0].test_cases[0].stderr.as_deref(),
            Some("separate error stream")
        );
        root.close().unwrap();
    }
    #[tokio::test]
    async fn native_effect_uncertainty_cannot_be_ignored_by_failure_policy() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("run.sh"), "never run locally").unwrap();
        let executor = TestExecutor::new(root.path().into())
            .with_process_effect(Arc::new(OutputEffect { fail: true }));
        let result = executor
            .execute_pack_tests_at(root.path(), "fixture", "1", &config())
            .await;
        executor.process_owner().shutdown().await;
        assert!(result.is_err());
        root.close().unwrap();
    }
    #[tokio::test]
    async fn native_effect_settled_parser_errors_keep_existing_suite_policy() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("run.sh"), "never run locally").unwrap();
        let executor = TestExecutor::new(root.path().into())
            .with_process_effect(Arc::new(OutputEffect { fail: false }));
        let mut cfg = config();
        cfg.runners.get_mut("native").unwrap().result_format = Some("json".into());
        let result = executor
            .execute_pack_tests_at(root.path(), "fixture", "1", &cfg)
            .await
            .unwrap();
        executor.process_owner().shutdown().await;
        assert_eq!(result.failed, 1);
        assert!(cfg.accepts_result(&result));
        root.close().unwrap();
    }
    struct RecordedTimeout;
    #[async_trait::async_trait]
    impl TestProcessEffect for RecordedTimeout {
        async fn run(&self, _: TestProcessRequest) -> Result<CommandOutput> {
            Ok(CommandOutput {
                interruption: Some(TestProcessInterruption::TimedOut),
                exit_code: 0,
                stdout: "Total Tests: 1\nPassed: 1\nFailed: 0\n".into(),
                stderr: "exact stopped stream".into(),
                duration_ms: 1,
            })
        }
    }
    #[tokio::test]
    async fn native_effect_observed_timeout_keeps_exit_status_and_failure_policy() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("run.sh"), "never run locally").unwrap();
        let executor =
            TestExecutor::new(root.path().into()).with_process_effect(Arc::new(RecordedTimeout));
        let cfg = config();
        let result = executor
            .execute_pack_tests_at(root.path(), "fixture", "1", &cfg)
            .await
            .unwrap();
        executor.process_owner().shutdown().await;
        assert_eq!(result.failed, 1);
        assert_eq!(result.passed, 0);
        assert!(cfg.accepts_result(&result));
        assert!(result.test_suites[0].test_cases[0]
            .error_message
            .as_deref()
            .unwrap()
            .contains("observed exit code: 0"));
        root.close().unwrap();
    }
    struct PendingEffect {
        started: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: tokio::sync::Notify,
        done: std::sync::atomic::AtomicBool,
    }
    #[async_trait::async_trait]
    impl TestProcessEffect for PendingEffect {
        async fn run(&self, _: TestProcessRequest) -> Result<CommandOutput> {
            self.started.lock().await.take().unwrap().send(()).unwrap();
            self.release.notified().await;
            self.done.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(CommandOutput {
                interruption: None,
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: 0,
            })
        }
    }
    #[tokio::test]
    async fn native_effect_owner_joins_after_consumer_abort_before_root_cleanup() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("run.sh"), "never run locally").unwrap();
        let (signal, started) = tokio::sync::oneshot::channel();
        let effect = Arc::new(PendingEffect {
            started: tokio::sync::Mutex::new(Some(signal)),
            release: tokio::sync::Notify::new(),
            done: Default::default(),
        });
        let executor = TestExecutor::new(root.path().into()).with_process_effect(effect.clone());
        let owner = executor.process_owner();
        let path = root.path().to_path_buf();
        let consumer = tokio::spawn(async move {
            executor
                .execute_pack_tests_at(&path, "fixture", "1", &config())
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), started)
            .await
            .unwrap()
            .unwrap();
        consumer.abort();
        assert!(consumer.await.is_err());
        assert!(!effect.done.load(std::sync::atomic::Ordering::SeqCst));
        effect.release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), owner.shutdown())
            .await
            .unwrap();
        assert!(effect.done.load(std::sync::atomic::Ordering::SeqCst));
        root.close().unwrap();
    }
    #[tokio::test]
    async fn native_effect_unsupported_metadata_keeps_existing_failure_policy() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("run.sh"), "never run locally").unwrap();
        let executor = TestExecutor::new(root.path().into())
            .with_process_effect(Arc::new(OutputEffect { fail: true }));
        let mut cfg = config();
        cfg.runners.get_mut("native").unwrap().r#type = "unsupported".into();
        let result = executor
            .execute_pack_tests_at(root.path(), "fixture", "1", &cfg)
            .await
            .unwrap();
        executor.process_owner().shutdown().await;
        assert_eq!(result.failed, 1);
        assert!(cfg.accepts_result(&result));
        assert!(result.test_suites[0].test_cases[0]
            .error_message
            .as_deref()
            .unwrap()
            .contains("Unsupported runner type"));
        root.close().unwrap();
    }
}
