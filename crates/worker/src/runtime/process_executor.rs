//! Shared Process Executor
//!
//! Provides common subprocess execution infrastructure used by all runtime
//! implementations. Handles streaming stdout/stderr capture, bounded log
//! collection, timeout management, stdin parameter delivery, and
//! output format parsing.
//!
//! ## Cancellation Support
//!
//! When a `CancellationToken` is provided, the executor monitors it alongside
//! the running process. On cancellation:
//! 1. SIGTERM is sent to the process immediately
//! 2. After a 10-second grace period, SIGKILL is sent as a last resort

use super::{
    action_output_mirror::{
        action_output_requires_delayed_mirror, safe_action_output_mirror,
        ActionOutputMirrorDecision, ActionOutputMirrorInput,
    },
    parameter_passing, BoundedLogFileWriter, BoundedLogWriter, ExecutionResult, OutputFormat,
    RuntimeError, RuntimeResult,
};
use attune_common::runtime_log_mirror::{RuntimeLogMirror, RuntimeLogSource, RuntimeLogStream};
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::Path;
use std::time::Instant;
use tokio::io::{AsyncBufRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::time::{timeout, Duration, Instant as TokioInstant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

const ETXTBSY_RETRY_ATTEMPTS: usize = 5;
const ETXTBSY_RETRY_DELAY_MS: u64 = 25;

struct CapturedOutput {
    writer: BoundedLogWriter,
    logs_incomplete: bool,
    seal_error: Option<attune_common::Error>,
    mirror_source: Option<RuntimeLogSource>,
}

#[derive(Clone, Copy)]
enum ExecutionInterrupt {
    Cancelled,
    TimedOut,
}

async fn await_execution<F>(
    future: F,
    cancel_token: Option<&CancellationToken>,
    deadline: Option<TokioInstant>,
) -> Result<F::Output, ExecutionInterrupt>
where
    F: Future,
{
    tokio::pin!(future);
    match (cancel_token, deadline) {
        (Some(token), Some(deadline)) => tokio::select! {
            biased;
            result = &mut future => Ok(result),
            _ = token.cancelled() => Err(ExecutionInterrupt::Cancelled),
            _ = tokio::time::sleep_until(deadline) => Err(ExecutionInterrupt::TimedOut),
        },
        (Some(token), None) => tokio::select! {
            biased;
            result = &mut future => Ok(result),
            _ = token.cancelled() => Err(ExecutionInterrupt::Cancelled),
        },
        (None, Some(deadline)) => tokio::select! {
            biased;
            result = &mut future => Ok(result),
            _ = tokio::time::sleep_until(deadline) => Err(ExecutionInterrupt::TimedOut),
        },
        (None, None) => Ok(future.await),
    }
}

async fn write_parameters_to_stdin(
    mut stdin: ChildStdin,
    parameters_stdin: Option<&str>,
) -> Option<String> {
    let mut error = None;
    if let Some(params_data) = parameters_stdin {
        if let Err(e) = stdin.write_all(params_data.as_bytes()).await {
            error = Some(format!("Failed to write parameters to stdin: {e}"));
        } else if let Err(e) = stdin.write_all(b"\n").await {
            error = Some(format!("Failed to write newline to stdin: {e}"));
        }
    }
    drop(stdin);
    error
}

async fn capture_output<R>(
    mut reader: R,
    mut writer: BoundedLogWriter,
    mut file: Option<BoundedLogFileWriter>,
    cancel: CancellationToken,
    stream: RuntimeLogStream,
    live_mirror: bool,
) -> CapturedOutput
where
    R: AsyncBufRead + Unpin,
{
    let mut buffer = vec![0_u8; 64 * 1024];
    let mirror_source = file.as_ref().and_then(|log| log.mirror_source().cloned());
    let mut mirror = live_mirror
        .then(|| ())
        .and_then(|()| file.as_ref())
        .and_then(|log| {
            log.mirror_source()
                .cloned()
                .map(|source| RuntimeLogMirror::new(source, stream, Some(log.max_bytes() as u64)))
        });
    let mut logs_incomplete = false;
    loop {
        let read = tokio::select! {
            result = reader.read(&mut buffer) => result,
            _ = cancel.cancelled() => {
                logs_incomplete = true;
                file = None;
                break;
            }
        };
        match read {
            Ok(0) => break,
            Ok(read) => {
                let bytes = &buffer[..read];
                if writer.write_all(bytes).await.is_err() {
                    logs_incomplete = true;
                    break;
                }
                if let Some(active_mirror) = mirror.as_mut() {
                    if let Err(error) = active_mirror.push(bytes) {
                        warn!(%error, stream = stream.as_str(), "Failed to mirror runtime log output");
                        mirror = None;
                    }
                }
                if let Some(log) = file.as_mut() {
                    let write = tokio::select! {
                        result = log.write_all(bytes) => Some(result),
                        _ = cancel.cancelled() => None,
                    };
                    match write {
                        Some(Ok(())) => {}
                        Some(Err(_)) => {
                            logs_incomplete = true;
                            file = None;
                            break;
                        }
                        None => {
                            logs_incomplete = true;
                            file = None;
                            break;
                        }
                    }
                }
            }
            Err(_) => {
                logs_incomplete = true;
                break;
            }
        }
    }

    if let Some(mirror) = mirror.as_mut() {
        if let Err(error) = mirror.finish() {
            warn!(%error, stream = stream.as_str(), "Failed to finish mirrored runtime log output");
        }
    }

    let seal_error = if let Some(log) = file {
        tokio::select! {
            result = log.seal() => result.err(),
            _ = cancel.cancelled() => {
                logs_incomplete = true;
                None
            }
        }
    } else {
        None
    };
    CapturedOutput {
        writer,
        logs_incomplete,
        seal_error,
        mirror_source,
    }
}

#[cfg(windows)]
pub(crate) struct WindowsProcessTree {
    job: usize,
}

#[cfg(windows)]
impl WindowsProcessTree {
    pub(crate) fn assign(child: &tokio::process::Child) -> io::Result<Self> {
        use std::mem::size_of;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };

        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            unsafe { CloseHandle(job) };
            return Err(error);
        }

        let process = match child.id() {
            Some(pid) => unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) },
            None => std::ptr::null_mut(),
        };
        if process.is_null() {
            let error = io::Error::last_os_error();
            unsafe { CloseHandle(job) };
            return Err(error);
        }

        let assigned = unsafe { AssignProcessToJobObject(job, process) } != 0;
        unsafe { CloseHandle(process) };
        if !assigned {
            let error = io::Error::last_os_error();
            unsafe { CloseHandle(job) };
            return Err(error);
        }

        Ok(Self { job: job as usize })
    }

    pub(crate) fn terminate(&self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        if unsafe { TerminateJobObject(self.job as _, 1) } == 0 {
            warn!(
                "Failed to terminate Windows process tree: {}",
                io::Error::last_os_error()
            );
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsProcessTree {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;

        unsafe { CloseHandle(self.job as _) };
    }
}

/// Execute a subprocess command with streaming output capture.
///
/// This is the core execution function used by all runtime implementations.
/// It handles:
/// - Spawning the process with piped I/O
/// - Writing parameters (with secrets merged in) to stdin
/// - Streaming stdout/stderr with bounded log collection
/// - Timeout management
/// - Output format parsing (JSON, YAML, JSONL, text)
///
/// # Arguments
/// * `cmd` - Pre-configured `Command` (interpreter, args, env vars, working dir already set)
/// * `secrets` - Deprecated/unused — secrets are now merged into parameters by the caller
/// * `parameters_stdin` - Optional parameter data (including secrets) to write to stdin
/// * `timeout_secs` - Optional execution timeout in seconds
/// * `max_stdout_bytes` - Maximum stdout size before truncation
/// * `max_stderr_bytes` - Maximum stderr size before truncation
/// * `output_format` - How to parse stdout (Text, Json, Yaml, Jsonl)
pub async fn execute_streaming(
    cmd: Command,
    _secrets: &HashMap<String, serde_json::Value>,
    parameters_stdin: Option<&str>,
    timeout_secs: Option<u64>,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
    output_format: OutputFormat,
) -> RuntimeResult<ExecutionResult> {
    execute_streaming_cancellable(
        cmd,
        _secrets,
        parameters_stdin,
        timeout_secs,
        max_stdout_bytes,
        max_stderr_bytes,
        output_format,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
}

/// Execute a subprocess command with streaming output capture and optional cancellation.
///
/// This is the core execution function used by all runtime implementations.
/// It handles:
/// - Spawning the process with piped I/O
/// - Writing parameters (with secrets merged in) to stdin
/// - Streaming stdout/stderr with bounded log collection
/// - Timeout management
/// - Prompt cancellation via SIGTERM → SIGKILL escalation
/// - Output format parsing (JSON, YAML, JSONL, text)
///
/// # Arguments
/// * `cmd` - Pre-configured `Command` (interpreter, args, env vars, working dir already set)
/// * `secrets` - Deprecated/unused — secrets are now merged into parameters by the caller
/// * `parameters_stdin` - Optional parameter data (including secrets) to write to stdin
/// * `timeout_secs` - Optional execution timeout in seconds
/// * `max_stdout_bytes` - Maximum stdout size before truncation
/// * `max_stderr_bytes` - Maximum stderr size before truncation
/// * `output_format` - How to parse stdout (Text, Json, Yaml, Jsonl)
/// * `cancel_token` - Optional cancellation token for graceful process termination
#[allow(clippy::too_many_arguments)]
pub async fn execute_streaming_cancellable(
    mut cmd: Command,
    _secrets: &HashMap<String, serde_json::Value>,
    parameters_stdin: Option<&str>,
    timeout_secs: Option<u64>,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
    output_format: OutputFormat,
    out_schema: Option<&serde_json::Value>,
    cancel_token: Option<CancellationToken>,
    _stdout_log_path: Option<&Path>,
    _stderr_log_path: Option<&Path>,
    stdout_log_writer: Option<BoundedLogFileWriter>,
    stderr_log_writer: Option<BoundedLogFileWriter>,
) -> RuntimeResult<ExecutionResult> {
    let start = Instant::now();
    let delay_action_mirror = action_output_requires_delayed_mirror(out_schema);
    let execution_deadline = timeout_secs
        .and_then(|seconds| TokioInstant::now().checked_add(Duration::from_secs(seconds)));
    let log_finalization_timeout_ms = stdout_log_writer
        .as_ref()
        .map(BoundedLogFileWriter::finalization_timeout_ms)
        .into_iter()
        .chain(
            stderr_log_writer
                .as_ref()
                .map(BoundedLogFileWriter::finalization_timeout_ms),
        )
        .max()
        .unwrap_or(1_000);

    configure_child_process(&mut cmd)?;

    // Spawn process with piped I/O. Native executables can briefly return
    // ETXTBSY after installation, so keep the existing bounded retry here.
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut last_spawn_error = None;
    let mut child = None;
    for attempt in 0..ETXTBSY_RETRY_ATTEMPTS {
        match cmd.spawn() {
            Ok(process) => {
                child = Some(process);
                break;
            }
            Err(error)
                if error.raw_os_error() == Some(26) && attempt + 1 < ETXTBSY_RETRY_ATTEMPTS =>
            {
                last_spawn_error = Some(error);
                tokio::time::sleep(Duration::from_millis(ETXTBSY_RETRY_DELAY_MS)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    let mut child = child.ok_or_else(|| {
        last_spawn_error.unwrap_or_else(|| io::Error::other("process spawn retry exhausted"))
    })?;

    // A Job Object makes Windows cancellation/timeout apply to the action's
    // entire process tree, not just its interpreter wrapper.
    #[cfg(windows)]
    let process_tree = WindowsProcessTree::assign(&child)?;

    // Create bounded writers
    let stdout_writer = BoundedLogWriter::new_stdout(max_stdout_bytes);
    let stderr_writer = BoundedLogWriter::new_stderr(max_stderr_bytes);

    // Take stdout and stderr streams
    let stdout = child.stdout.take().expect("stdout not captured");
    let stderr = child.stderr.take().expect("stderr not captured");

    // Create buffered readers
    let stdout_reader = BufReader::new(stdout);
    let stderr_reader = BufReader::new(stderr);
    let output_cancel = CancellationToken::new();
    let stdout_cancel = output_cancel.clone();
    let stderr_cancel = output_cancel.clone();

    let stdout_task = tokio::spawn(capture_output(
        stdout_reader,
        stdout_writer,
        stdout_log_writer,
        stdout_cancel,
        RuntimeLogStream::Stdout,
        !delay_action_mirror,
    ));
    let stderr_task = tokio::spawn(capture_output(
        stderr_reader,
        stderr_writer,
        stderr_log_writer,
        stderr_cancel,
        RuntimeLogStream::Stderr,
        !delay_action_mirror,
    ));

    // Stdin delivery is part of the action's execution budget. Output capture
    // starts first so a child that writes before reading stdin cannot deadlock.
    let stdin_result = if let Some(stdin) = child.stdin.take() {
        await_execution(
            write_parameters_to_stdin(stdin, parameters_stdin),
            cancel_token.as_ref(),
            execution_deadline,
        )
        .await
    } else {
        Ok(None)
    };

    // Build the wait future that handles timeout, cancellation, and normal completion.
    //
    // The result is a tuple: (exit_status, was_cancelled, was_timed_out)
    let wait = match stdin_result {
        Ok(stdin_write_error) => {
            match await_execution(child.wait(), cancel_token.as_ref(), execution_deadline).await {
                Ok(result) => (result, false, false, stdin_write_error),
                Err(ExecutionInterrupt::Cancelled) => {
                    output_cancel.cancel();
                    #[cfg(windows)]
                    process_tree.terminate();
                    terminate_process(&mut child, "cancel");
                    (
                        wait_for_terminated_child(&mut child).await,
                        true,
                        false,
                        stdin_write_error,
                    )
                }
                Err(ExecutionInterrupt::TimedOut) => {
                    output_cancel.cancel();
                    warn!(
                        "Process timed out after {} seconds, terminating",
                        timeout_secs.unwrap()
                    );
                    #[cfg(windows)]
                    process_tree.terminate();
                    terminate_process(&mut child, "timeout");
                    (
                        wait_for_terminated_child(&mut child).await,
                        false,
                        true,
                        stdin_write_error,
                    )
                }
            }
        }
        Err(interrupt) => {
            output_cancel.cancel();
            let (was_cancelled, was_timed_out, reason) = match interrupt {
                ExecutionInterrupt::Cancelled => (true, false, "cancel"),
                ExecutionInterrupt::TimedOut => {
                    warn!("Process timed out during stdin delivery");
                    (false, true, "timeout")
                }
            };
            #[cfg(windows)]
            process_tree.terminate();
            terminate_process(&mut child, reason);
            (
                wait_for_terminated_child(&mut child).await,
                was_cancelled,
                was_timed_out,
                None,
            )
        }
    };
    let (wait_result, mut was_cancelled, mut was_timed_out, stdin_write_error) = wait;

    let output_results = async {
        let stdout = stdout_task.await.map_err(|error| {
            RuntimeError::ExecutionFailed(format!("stdout task failed: {error}"))
        })?;
        let stderr = stderr_task.await.map_err(|error| {
            RuntimeError::ExecutionFailed(format!("stderr task failed: {error}"))
        })?;
        Ok::<_, RuntimeError>((stdout, stderr))
    };
    tokio::pin!(output_results);
    let mut logs_incomplete = false;
    let outputs = if was_cancelled || was_timed_out {
        output_cancel.cancel();
        output_results.await?
    } else {
        let finalization_deadline = TokioInstant::now()
            .checked_add(Duration::from_millis(log_finalization_timeout_ms))
            .unwrap_or_else(TokioInstant::now);
        let effective_deadline = execution_deadline
            .map(|deadline| deadline.min(finalization_deadline))
            .unwrap_or(finalization_deadline);
        tokio::select! {
            result = &mut output_results => result?,
            _ = async {
                if let Some(token) = cancel_token.as_ref() {
                    token.cancelled().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                was_cancelled = true;
                logs_incomplete = true;
                output_cancel.cancel();
                output_results.await?
            }
            _ = tokio::time::sleep_until(effective_deadline) => {
                if execution_deadline.is_some_and(|deadline| deadline <= effective_deadline) {
                    was_timed_out = true;
                }
                logs_incomplete = true;
                output_cancel.cancel();
                output_results.await?
            }
        }
    };
    let (stdout_output, stderr_output) = outputs;
    logs_incomplete |= stdout_output.logs_incomplete || stderr_output.logs_incomplete;
    if !logs_incomplete {
        if let Some(error) = stdout_output.seal_error {
            return Err(RuntimeError::ExecutionFailed(format!(
                "Failed to seal stdout log: {error}"
            )));
        }
        if let Some(error) = stderr_output.seal_error {
            return Err(RuntimeError::ExecutionFailed(format!(
                "Failed to seal stderr log: {error}"
            )));
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    // Get results from bounded writers
    let stdout_result = stdout_output.writer.into_result();
    let stderr_result = stderr_output.writer.into_result();

    if delay_action_mirror {
        match safe_action_output_mirror(ActionOutputMirrorInput {
            stdout: &stdout_result.content,
            stderr: &stderr_result.content,
            output_format,
            out_schema,
            stdout_truncated: stdout_result.truncated,
            stderr_truncated: stderr_result.truncated,
            logs_incomplete,
            mirror_stdout: stdout_output.mirror_source.is_some(),
            mirror_stderr: stderr_output.mirror_source.is_some(),
        }) {
            ActionOutputMirrorDecision::Delayed(content) => {
                if let (Some(source), Some(stdout)) =
                    (stdout_output.mirror_source, content.stdout)
                {
                    mirror_delayed_action_output(source, RuntimeLogStream::Stdout, &stdout);
                }
                if let (Some(source), Some(stderr)) =
                    (stderr_output.mirror_source, content.stderr)
                {
                    mirror_delayed_action_output(source, RuntimeLogStream::Stderr, &stderr);
                }
            }
            ActionOutputMirrorDecision::Suppressed(reason) => warn!(
                reason = reason.as_str(),
                "Suppressed action runtime log mirroring because secret output could not be masked safely"
            ),
            ActionOutputMirrorDecision::Live => {}
        }
    }

    // Handle process wait result
    let (exit_code, process_error) = match wait_result {
        Ok(status) => (status.code().unwrap_or(-1), None),
        Err(e) => {
            warn!("Process wait failed but captured output: {}", e);
            (-1, Some(format!("Process wait failed: {}", e)))
        }
    };

    if was_timed_out {
        return Ok(ExecutionResult {
            exit_code: -1,
            stdout: stdout_result.content.clone(),
            stderr: stderr_result.content.clone(),
            result: None,
            duration_ms,
            error: Some(format!(
                "Execution timed out after {} seconds",
                timeout_secs.unwrap()
            )),
            stdout_truncated: stdout_result.truncated,
            stderr_truncated: stderr_result.truncated,
            stdout_bytes_truncated: stdout_result.bytes_truncated,
            stderr_bytes_truncated: stderr_result.bytes_truncated,
            timed_out: true,
            logs_incomplete,
        });
    }

    // If the process was cancelled, return a specific result
    if was_cancelled {
        return Ok(ExecutionResult {
            exit_code,
            stdout: stdout_result.content.clone(),
            stderr: stderr_result.content.clone(),
            result: None,
            duration_ms,
            error: Some("Execution cancelled by user".to_string()),
            stdout_truncated: stdout_result.truncated,
            stderr_truncated: stderr_result.truncated,
            stdout_bytes_truncated: stdout_result.bytes_truncated,
            stderr_bytes_truncated: stderr_result.bytes_truncated,
            timed_out: false,
            logs_incomplete,
        });
    }

    debug!(
        "Process execution completed: exit_code={}, duration={}ms, stdout_truncated={}, stderr_truncated={}",
        exit_code, duration_ms, stdout_result.truncated, stderr_result.truncated
    );

    // Parse result from stdout based on output_format
    let result = if exit_code == 0 && !stdout_result.content.trim().is_empty() {
        parse_output(&stdout_result.content, output_format)
    } else {
        None
    };

    // Determine error message
    let error = if let Some(proc_err) = process_error {
        Some(proc_err)
    } else if let Some(stdin_err) = stdin_write_error {
        // Ignore broken pipe errors for fast-exiting successful actions.
        // These occur when the process exits before we finish writing secrets to stdin.
        let is_broken_pipe = stdin_err.contains("Broken pipe") || stdin_err.contains("os error 32");
        let is_fast_exit = duration_ms < 500;
        let is_success = exit_code == 0;

        if is_broken_pipe && is_fast_exit && is_success {
            debug!(
                "Ignoring broken pipe error for fast-exiting successful action ({}ms)",
                duration_ms
            );
            None
        } else {
            Some(stdin_err)
        }
    } else if exit_code != 0 {
        Some(if stderr_result.content.is_empty() {
            format!("Command exited with code {}", exit_code)
        } else {
            // Use last line of stderr as error, or full stderr if short
            if stderr_result.content.lines().count() > 5 {
                stderr_result
                    .content
                    .lines()
                    .last()
                    .unwrap_or("")
                    .to_string()
            } else {
                stderr_result.content.clone()
            }
        })
    } else {
        None
    };

    Ok(ExecutionResult {
        exit_code,
        // Only populate stdout if result wasn't parsed (avoid duplication)
        stdout: if result.is_some() {
            String::new()
        } else {
            stdout_result.content.clone()
        },
        stderr: stderr_result.content.clone(),
        result,
        duration_ms,
        error,
        stdout_truncated: stdout_result.truncated,
        stderr_truncated: stderr_result.truncated,
        stdout_bytes_truncated: stdout_result.bytes_truncated,
        stderr_bytes_truncated: stderr_result.bytes_truncated,
        timed_out: false,
        logs_incomplete,
    })
}

fn mirror_delayed_action_output(source: RuntimeLogSource, stream: RuntimeLogStream, content: &str) {
    let mut mirror = RuntimeLogMirror::new(source, stream, None);
    if let Err(error) = mirror
        .push(content.as_bytes())
        .and_then(|()| mirror.finish())
    {
        warn!(%error, stream = stream.as_str(), "Failed to mirror masked action runtime log output");
    }
}

/// Parse stdout content according to the specified output format.
pub(crate) fn configure_child_process(cmd: &mut Command) -> io::Result<()> {
    #[cfg(unix)]
    {
        // Run each action in its own process group so cancellation and timeout
        // can terminate shell wrappers and any children they spawned.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }

    #[cfg(not(unix))]
    let _ = cmd;

    Ok(())
}

#[cfg(unix)]
struct KillProcessGroupOnDrop(Option<u32>);

#[cfg(unix)]
impl Drop for KillProcessGroupOnDrop {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            kill_process_group_or_process(pid, KILL_SIGNAL);
        }
    }
}

/// Run setup/install commands so dropping the future kills their whole process
/// group, not only the direct shell or package-manager child.
pub(crate) async fn run_command_output_owned(
    mut command: Command,
) -> io::Result<std::process::Output> {
    configure_child_process(&mut command)?;
    command
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = command.spawn()?;
    #[cfg(unix)]
    let mut process_group_guard = KillProcessGroupOnDrop(child.id());

    let output = child.wait_with_output().await;
    #[cfg(unix)]
    if output.is_ok() {
        process_group_guard.0 = None;
    }
    output
}

pub(crate) async fn wait_for_terminated_child(
    child: &mut tokio::process::Child,
) -> io::Result<std::process::ExitStatus> {
    // Capture the process-group leader before wait() reaps it and clears id().
    // A shell wrapper may exit on SIGTERM while a descendant ignores it.
    #[cfg(unix)]
    let process_group_id = child.id();

    let wait_result = timeout(std::time::Duration::from_secs(10), child.wait()).await;
    #[cfg(unix)]
    if let Some(pid) = process_group_id {
        // Always escalate after the graceful wait. Signalling a vanished group
        // is harmless; signalling a surviving group prevents orphaned children
        // even when the direct child exited promptly.
        kill_process_group_or_process(pid, KILL_SIGNAL);
    }

    match wait_result {
        Ok(status) => status,
        Err(_) => {
            warn!("Process did not exit after SIGTERM + 10s, sending SIGKILL");
            #[cfg(windows)]
            if let Err(error) = child.start_kill() {
                warn!("Failed to kill timed-out process: {}", error);
            }
            child.wait().await
        }
    }
}

pub(crate) fn terminate_process(child: &mut tokio::process::Child, reason: &str) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            info!("Sending SIGTERM to {} process group {}", reason, pid);
            kill_process_group_or_process(pid, TERM_SIGNAL);
        } else {
            warn!("Unable to terminate {} process: PID is unavailable", reason);
        }
    }
    #[cfg(windows)]
    {
        info!("Terminating process ({})", reason);
        if let Err(error) = child.start_kill() {
            warn!("Failed to terminate process ({}): {}", reason, error);
        }
    }
}

#[cfg(unix)]
fn kill_process_group_or_process(pid: u32, signal: i32) {
    #[cfg(unix)]
    {
        // Negative PID targets the process group created with setpgid(0, 0).
        let pgid = -(pid as i32);
        // Safety: we only signal processes we spawned.
        let rc = unsafe { libc::kill(pgid, signal) };
        if rc == 0 {
            return;
        }

        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return;
        }
        warn!(
            "Failed to signal process group {} with signal {}: {}. Falling back to PID {}",
            pid, signal, err, pid
        );

        // Safety: fallback to the direct child PID
        unsafe {
            libc::kill(pid as i32, signal);
        }
    }

    #[cfg(windows)]
    {
        let _ = pid;
        let _ = signal;
        // Process groups / signals not supported on Windows in the same way;
        // process termination is handled by Child::kill / timeout mechanism.
    }
}

fn parse_output(stdout: &str, format: OutputFormat) -> Option<serde_json::Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }

    match format {
        OutputFormat::Text => {
            // No parsing - text output is captured in stdout field
            None
        }
        OutputFormat::Json => {
            // Try to parse full stdout as JSON first (handles multi-line JSON),
            // then fall back to last line only (for scripts that log before output)
            serde_json::from_str(trimmed).ok().or_else(|| {
                trimmed
                    .lines()
                    .last()
                    .and_then(|line| serde_json::from_str(line).ok())
            })
        }
        OutputFormat::Yaml => {
            // Try to parse stdout as YAML
            serde_yaml_ng::from_str(trimmed).ok()
        }
        OutputFormat::Jsonl => {
            // Parse each line as JSON and collect into array
            let mut items = Vec::new();
            for line in trimmed.lines() {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                    items.push(value);
                }
            }
            if items.is_empty() {
                None
            } else {
                Some(serde_json::Value::Array(items))
            }
        }
    }
}

/// Build a `Command` for executing an action script with the given interpreter.
///
/// This configures the command with:
/// - The interpreter binary and any additional args
/// - The action file path as the final argument
/// - Environment variables from the execution context
/// - Working directory (pack directory)
///
/// # Arguments
/// * `interpreter` - Path to the interpreter binary
/// * `interpreter_args` - Additional args before the action file
/// * `action_file` - Path to the action script file
/// * `working_dir` - Working directory for the process (typically the pack dir)
/// * `env_vars` - Environment variables to set
pub fn build_action_command(
    interpreter: &Path,
    interpreter_args: &[String],
    action_file: &Path,
    working_dir: Option<&Path>,
    env_vars: &HashMap<String, String>,
) -> Command {
    let mut cmd = Command::new(interpreter);

    // Add interpreter args (e.g., "-u" for unbuffered Python)
    for arg in interpreter_args {
        cmd.arg(arg);
    }

    // Add the action file as the last argument
    cmd.arg(action_file);

    // Set working directory
    if let Some(dir) = working_dir {
        if dir.exists() {
            cmd.current_dir(dir);
        }
    }

    parameter_passing::apply_runtime_environment(&mut cmd, env_vars);

    cmd
}

/// Build a `Command` for executing inline code with the given interpreter.
///
/// This is used for ad-hoc/inline actions where code is passed as a string
/// rather than a file path.
///
/// # Arguments
/// * `interpreter` - Path to the interpreter binary
/// * `code` - The inline code to execute
/// * `env_vars` - Environment variables to set
pub fn build_inline_command(
    interpreter: &Path,
    code: &str,
    env_vars: &HashMap<String, String>,
) -> Command {
    let mut cmd = Command::new(interpreter);

    // Pass code via -c flag (works for bash, python, etc.)
    cmd.arg("-c").arg(code);

    parameter_passing::apply_runtime_environment(&mut cmd, env_vars);

    cmd
}

#[cfg(unix)]
const KILL_SIGNAL: i32 = libc::SIGKILL;
#[cfg(unix)]
const TERM_SIGNAL: i32 = libc::SIGTERM;

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::NamedTempFile;
    use tokio::fs;
    use tokio::sync::Notify;
    use tokio::time::{sleep, Duration};

    #[derive(Debug, Default)]
    struct HangingLogTransport {
        active: AtomicUsize,
        started: Notify,
    }

    struct ActiveCommit<'a>(&'a AtomicUsize);

    impl Drop for ActiveCommit<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl attune_common::artifact_transport::ArtifactFileTransport for HangingLogTransport {
        async fn write_file(
            &self,
            _: &str,
            _: &[u8],
            _: Option<&str>,
        ) -> attune_common::Result<()> {
            unreachable!()
        }
        async fn file_exists(&self, _: &str) -> attune_common::Result<bool> {
            unreachable!()
        }
        async fn file_size(&self, _: &str) -> attune_common::Result<Option<u64>> {
            unreachable!()
        }
        async fn delete_file(&self, _: &str) -> attune_common::Result<()> {
            unreachable!()
        }
        async fn commit_log_segment(&self, _: i64, _: i64, _: &[u8]) -> attune_common::Result<()> {
            self.active.fetch_add(1, Ordering::SeqCst);
            let _active = ActiveCommit(&self.active);
            self.started.notify_one();
            std::future::pending().await
        }
        async fn seal_log_stream(&self, _: i64, _: bool) -> attune_common::Result<()> {
            Ok(())
        }
        async fn open_reader(
            &self,
            _: &str,
            _: u64,
        ) -> attune_common::Result<attune_common::artifact_transport::BoxAsyncReader> {
            unreachable!()
        }
        fn transport_mode(&self) -> &'static str {
            "test"
        }
        fn base_dir(&self) -> &str {
            ""
        }
    }

    #[test]
    fn test_parse_output_text() {
        let result = parse_output("hello world", OutputFormat::Text);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_output_json() {
        let result = parse_output(r#"{"key": "value"}"#, OutputFormat::Json);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["key"], "value");
    }

    #[test]
    fn test_parse_output_json_with_log_prefix() {
        let result = parse_output(
            "some log line\nanother log\n{\"key\": \"value\"}",
            OutputFormat::Json,
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap()["key"], "value");
    }

    #[test]
    fn test_parse_output_jsonl() {
        let result = parse_output("{\"a\": 1}\n{\"b\": 2}\n{\"c\": 3}", OutputFormat::Jsonl);
        assert!(result.is_some());
        let arr = result.unwrap();
        assert_eq!(arr.as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_parse_output_yaml() {
        let result = parse_output("key: value\nother: 42", OutputFormat::Yaml);
        assert!(result.is_some());
        let val = result.unwrap();
        assert_eq!(val["key"], "value");
        assert_eq!(val["other"], 42);
    }

    #[test]
    fn test_parse_output_empty() {
        assert!(parse_output("", OutputFormat::Json).is_none());
        assert!(parse_output("  ", OutputFormat::Yaml).is_none());
        assert!(parse_output("\n", OutputFormat::Jsonl).is_none());
    }

    #[tokio::test]
    async fn test_execute_streaming_simple() {
        let mut cmd = Command::new("/bin/echo");
        cmd.arg("hello world");

        let result = execute_streaming(
            cmd,
            &HashMap::new(),
            None,
            Some(10),
            1024 * 1024,
            1024 * 1024,
            OutputFormat::Text,
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello world"));
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_execute_streaming_json_output() {
        let mut cmd = Command::new("/bin/bash");
        cmd.arg("-c").arg(r#"echo '{"status": "ok", "count": 42}'"#);

        let result = execute_streaming(
            cmd,
            &HashMap::new(),
            None,
            Some(10),
            1024 * 1024,
            1024 * 1024,
            OutputFormat::Json,
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.result.is_some());
        let parsed = result.result.unwrap();
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["count"], 42);
    }

    #[tokio::test]
    async fn test_execute_streaming_failure() {
        let mut cmd = Command::new("/bin/bash");
        cmd.arg("-c").arg("echo 'error msg' >&2; exit 1");

        let result = execute_streaming(
            cmd,
            &HashMap::new(),
            None,
            Some(10),
            1024 * 1024,
            1024 * 1024,
            OutputFormat::Text,
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 1);
        assert!(result.error.is_some());
        assert!(result.stderr.contains("error msg"));
    }

    #[tokio::test]
    async fn test_build_action_command() {
        let interpreter = Path::new("/usr/bin/python3");
        let args = vec!["-u".to_string()];
        let action_file = Path::new("/opt/attune/packs/mypack/actions/hello.py");
        let mut env = HashMap::new();
        env.insert("ATTUNE_EXEC_ID".to_string(), "123".to_string());

        let cmd = build_action_command(interpreter, &args, action_file, None, &env);

        // We can't easily inspect Command internals, but at least verify it builds without panic
        let _ = cmd;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_execute_streaming_cancellation_kills_shell_child_process() {
        let script = NamedTempFile::new().unwrap();
        let pid_dir = tempfile::tempdir().unwrap();
        let pid_path = pid_dir.path().join("child.pid");
        fs::write(
            script.path(),
            "#!/bin/sh\ntrap 'exit 0' TERM\nsh -c 'trap \"\" TERM; exec sleep 30' &\necho $! > \"$CHILD_PID_FILE\"\nwait\nprintf 'unexpected completion\\n'\n",
        )
        .await
        .unwrap();

        #[cfg(unix)]
        let mut perms = fs::metadata(script.path()).await.unwrap().permissions();
        #[cfg(not(unix))]
        let perms = fs::metadata(script.path()).await.unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
        }
        fs::set_permissions(script.path(), perms).await.unwrap();

        let cancel_token = CancellationToken::new();
        let trigger = cancel_token.clone();
        let readiness_path = pid_path.clone();
        let cancellation_helper = tokio::spawn(async move {
            let child_pid = timeout(Duration::from_secs(2), async {
                loop {
                    if let Ok(contents) = fs::read_to_string(&readiness_path).await {
                        if let Ok(pid) = contents.trim().parse::<i32>() {
                            break pid;
                        }
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("shell child did not publish readiness");
            trigger.cancel();
            child_pid
        });

        let mut cmd = Command::new("/bin/sh");
        cmd.arg(script.path()).env("CHILD_PID_FILE", &pid_path);

        let result = execute_streaming_cancellable(
            cmd,
            &HashMap::new(),
            None,
            Some(60),
            1024 * 1024,
            1024 * 1024,
            OutputFormat::Text,
            None,
            Some(cancel_token),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let child_pid = cancellation_helper.await.unwrap();

        assert!(result
            .error
            .as_deref()
            .is_some_and(|e| e.contains("cancelled")));
        assert!(
            result.duration_ms < 5_000,
            "expected prompt cancellation, got {}ms",
            result.duration_ms
        );
        assert!(!result.stdout.contains("unexpected completion"));

        timeout(Duration::from_secs(2), async {
            loop {
                let rc = unsafe { libc::kill(child_pid, 0) };
                if rc == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled descendant process remained alive");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_owned_command_future_kills_term_ignoring_descendant() {
        let pid_dir = tempfile::tempdir().unwrap();
        let pid_path = pid_dir.path().join("descendant.pid");
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(
            "sh -c 'trap \"\" TERM; echo $$ > \"$DESCENDANT_PID_FILE\"; exec sleep 30' & wait",
        );
        command.env("DESCENDANT_PID_FILE", &pid_path);

        let task = tokio::spawn(run_command_output_owned(command));
        let descendant_pid = timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(contents) = fs::read_to_string(&pid_path).await {
                    if let Ok(pid) = contents.trim().parse::<i32>() {
                        break pid;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("descendant did not publish readiness");

        task.abort();
        let _ = task.await;

        timeout(Duration::from_secs(2), async {
            loop {
                let rc = unsafe { libc::kill(descendant_pid, 0) };
                if rc == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("owned command descendant remained alive after future cancellation");
    }

    #[tokio::test]
    async fn cancellation_aborts_blocked_log_upload() {
        let transport = Arc::new(HangingLogTransport::default());
        let segmented = attune_common::log_stream::SegmentedLogWriter::new(
            transport.clone(),
            1,
            attune_common::log_stream::SegmentedLogConfig {
                initial_segment_bytes: 1,
                max_segment_bytes: 1,
                flush_interval_ms: 60_000,
                retry_max_attempts: 3,
                retry_attempt_timeout_ms: 60_000,
                retry_initial_backoff_ms: 1,
                retry_max_backoff_ms: 2,
                finalization_timeout_ms: 100,
            },
        )
        .unwrap();
        let stdout_writer = BoundedLogFileWriter::from_segmented_writer(segmented, 1024, true);
        let cancel_token = CancellationToken::new();
        let trigger = cancel_token.clone();
        let started = transport.clone();
        let cancellation_helper = tokio::spawn(async move {
            started.started.notified().await;
            trigger.cancel();
        });
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("printf 'x\\n'; sleep 30");

        let result = execute_streaming_cancellable(
            cmd,
            &HashMap::new(),
            None,
            Some(60),
            1024,
            1024,
            OutputFormat::Text,
            None,
            Some(cancel_token),
            None,
            None,
            Some(stdout_writer),
            None,
        )
        .await
        .unwrap();
        cancellation_helper.await.unwrap();

        assert!(result
            .error
            .as_deref()
            .is_some_and(|e| e.contains("cancelled")));
        assert_eq!(transport.active.load(Ordering::SeqCst), 0);
        assert!(result.duration_ms < 5_000);
        assert!(result.logs_incomplete);
    }

    #[tokio::test]
    async fn cancellation_after_child_exit_aborts_blocked_log_upload() {
        let transport = Arc::new(HangingLogTransport::default());
        let segmented = attune_common::log_stream::SegmentedLogWriter::new(
            transport.clone(),
            2,
            attune_common::log_stream::SegmentedLogConfig {
                initial_segment_bytes: 1,
                max_segment_bytes: 1,
                flush_interval_ms: 60_000,
                retry_max_attempts: 3,
                retry_attempt_timeout_ms: 60_000,
                retry_initial_backoff_ms: 1,
                retry_max_backoff_ms: 2,
                finalization_timeout_ms: 5_000,
            },
        )
        .unwrap();
        let stdout_writer = BoundedLogFileWriter::from_segmented_writer(segmented, 1024, true);
        let cancel_token = CancellationToken::new();
        let trigger = cancel_token.clone();
        let started = transport.clone();
        let cancellation_helper = tokio::spawn(async move {
            started.started.notified().await;
            sleep(Duration::from_millis(50)).await;
            trigger.cancel();
        });
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("printf 'x\\n'");

        let result = execute_streaming_cancellable(
            cmd,
            &HashMap::new(),
            None,
            Some(60),
            1024,
            1024,
            OutputFormat::Text,
            None,
            Some(cancel_token),
            None,
            None,
            Some(stdout_writer),
            None,
        )
        .await
        .unwrap();
        cancellation_helper.await.unwrap();

        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cancelled")));
        assert!(result.logs_incomplete);
        assert_eq!(transport.active.load(Ordering::SeqCst), 0);
        assert!(result.duration_ms < 1_000);
    }

    #[tokio::test]
    async fn post_exit_log_finalization_deadline_is_bounded() {
        let transport = Arc::new(HangingLogTransport::default());
        let segmented = attune_common::log_stream::SegmentedLogWriter::new(
            transport.clone(),
            3,
            attune_common::log_stream::SegmentedLogConfig {
                initial_segment_bytes: 1,
                max_segment_bytes: 1,
                flush_interval_ms: 60_000,
                retry_max_attempts: 3,
                retry_attempt_timeout_ms: 60_000,
                retry_initial_backoff_ms: 1,
                retry_max_backoff_ms: 2,
                finalization_timeout_ms: 25,
            },
        )
        .unwrap();
        let stdout_writer = BoundedLogFileWriter::from_segmented_writer(segmented, 1024, true);
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("printf 'x\\n'");

        let result = execute_streaming_cancellable(
            cmd,
            &HashMap::new(),
            None,
            Some(60),
            1024,
            1024,
            OutputFormat::Text,
            None,
            None,
            None,
            None,
            Some(stdout_writer),
            None,
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.error.is_none());
        assert!(result.logs_incomplete);
        assert_eq!(transport.active.load(Ordering::SeqCst), 0);
        assert!(result.duration_ms < 1_000);
    }

    #[tokio::test]
    async fn stdin_delivery_and_child_wait_share_one_execution_deadline() {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("sleep 30; read -r input");
        let parameters = "x".repeat(2 * 1024 * 1024);

        let result = execute_streaming_cancellable(
            cmd,
            &HashMap::new(),
            Some(&parameters),
            Some(1),
            1024,
            1024,
            OutputFormat::Text,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(result.timed_out);
        assert!(result.duration_ms < 5_000);
    }

    #[tokio::test]
    async fn test_execute_streaming_timeout_terminates_process() {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("sleep 30");

        let result = execute_streaming(
            cmd,
            &HashMap::new(),
            None,
            Some(1),
            1024 * 1024,
            1024 * 1024,
            OutputFormat::Text,
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, -1);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|e| e.contains("timed out after 1 seconds")));
        assert!(
            result.duration_ms < 7_000,
            "expected timeout termination, got {}ms",
            result.duration_ms
        );
    }
}
