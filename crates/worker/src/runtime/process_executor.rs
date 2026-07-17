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

use super::{BoundedLogFileWriter, BoundedLogWriter, ExecutionResult, OutputFormat, RuntimeResult};
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

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
    cancel_token: Option<CancellationToken>,
    stdout_log_path: Option<&Path>,
    stderr_log_path: Option<&Path>,
    stdout_log_writer: Option<BoundedLogFileWriter>,
    stderr_log_writer: Option<BoundedLogFileWriter>,
) -> RuntimeResult<ExecutionResult> {
    let start = Instant::now();

    configure_child_process(&mut cmd)?;

    // Spawn process with piped I/O
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    // A Job Object makes Windows cancellation/timeout apply to the action's
    // entire process tree, not just its interpreter wrapper.
    #[cfg(windows)]
    let process_tree = WindowsProcessTree::assign(&child)?;

    // Write to stdin - parameters (with secrets already merged in by the caller).
    // If this fails, the process has already started, so we continue and capture output.
    let stdin_write_error = if let Some(mut stdin) = child.stdin.take() {
        let mut error = None;

        // Write parameters to stdin as a single JSON line.
        // Secrets are merged into the parameters map by the caller, so the
        // action reads everything with a single readline().
        if let Some(params_data) = parameters_stdin {
            if let Err(e) = stdin.write_all(params_data.as_bytes()).await {
                error = Some(format!("Failed to write parameters to stdin: {}", e));
            } else if let Err(e) = stdin.write_all(b"\n").await {
                error = Some(format!("Failed to write newline to stdin: {}", e));
            }
        }

        drop(stdin);
        error
    } else {
        None
    };

    // Create bounded writers
    let mut stdout_writer = BoundedLogWriter::new_stdout(max_stdout_bytes);
    let mut stderr_writer = BoundedLogWriter::new_stderr(max_stderr_bytes);
    // Prefer pre-opened transport writers over path-based file writers
    let mut stdout_file =
        stdout_log_writer.or_else(|| open_live_log_file(stdout_log_path, max_stdout_bytes, true));
    let mut stderr_file =
        stderr_log_writer.or_else(|| open_live_log_file(stderr_log_path, max_stderr_bytes, false));

    // Take stdout and stderr streams
    let stdout = child.stdout.take().expect("stdout not captured");
    let stderr = child.stderr.take().expect("stderr not captured");

    // Create buffered readers
    let mut stdout_reader = BufReader::new(stdout);
    let mut stderr_reader = BufReader::new(stderr);

    // Stream both outputs concurrently
    let stdout_task = async {
        let mut line = Vec::new();
        loop {
            line.clear();
            match stdout_reader.read_until(b'\n', &mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    if stdout_writer.write_all(&line).await.is_err() {
                        break;
                    }
                    if let Some(file) = stdout_file.as_mut() {
                        let _ = file.write_all(&line).await;
                    }
                }
                Err(_) => break,
            }
        }
        stdout_writer
    };

    let stderr_task = async {
        let mut line = Vec::new();
        loop {
            line.clear();
            match stderr_reader.read_until(b'\n', &mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    if stderr_writer.write_all(&line).await.is_err() {
                        break;
                    }
                    if let Some(file) = stderr_file.as_mut() {
                        let _ = file.write_all(&line).await;
                    }
                }
                Err(_) => break,
            }
        }
        stderr_writer
    };

    // Build the wait future that handles timeout, cancellation, and normal completion.
    //
    // The result is a tuple: (exit_status, was_cancelled, was_timed_out)
    let wait_future = async {
        match (cancel_token.as_ref(), timeout_secs) {
            (Some(token), Some(timeout_secs)) => {
                tokio::select! {
                    result = child.wait() => (result, false, false),
                    _ = token.cancelled() => {
                        #[cfg(windows)]
                        process_tree.terminate();
                        terminate_process(&mut child, "cancel");
                        (wait_for_terminated_child(&mut child).await, true, false)
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(timeout_secs)) => {
                        warn!("Process timed out after {} seconds, terminating", timeout_secs);
                        #[cfg(windows)]
                        process_tree.terminate();
                        terminate_process(&mut child, "timeout");
                        (wait_for_terminated_child(&mut child).await, false, true)
                    }
                }
            }
            (Some(token), None) => {
                tokio::select! {
                    result = child.wait() => (result, false, false),
                    _ = token.cancelled() => {
                        #[cfg(windows)]
                        process_tree.terminate();
                        terminate_process(&mut child, "cancel");
                        (wait_for_terminated_child(&mut child).await, true, false)
                    }
                }
            }
            (None, Some(timeout_secs)) => {
                tokio::select! {
                    result = child.wait() => (result, false, false),
                    _ = tokio::time::sleep(std::time::Duration::from_secs(timeout_secs)) => {
                        warn!("Process timed out after {} seconds, terminating", timeout_secs);
                        #[cfg(windows)]
                        process_tree.terminate();
                        terminate_process(&mut child, "timeout");
                        (wait_for_terminated_child(&mut child).await, false, true)
                    }
                }
            }
            (None, None) => (child.wait().await, false, false),
        }
    };

    // Wait for both streams and the process
    let (stdout_writer, stderr_writer, (wait_result, was_cancelled, was_timed_out)) =
        tokio::join!(stdout_task, stderr_task, wait_future);

    let duration_ms = start.elapsed().as_millis() as u64;

    // Get results from bounded writers
    let stdout_result = stdout_writer.into_result();
    let stderr_result = stderr_writer.into_result();

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
    })
}

fn open_live_log_file(
    path: Option<&Path>,
    max_bytes: usize,
    is_stdout: bool,
) -> Option<BoundedLogFileWriter> {
    let path = path?;
    let writer = if is_stdout {
        BoundedLogFileWriter::new_stdout(path, max_bytes)
    } else {
        BoundedLogFileWriter::new_stderr(path, max_bytes)
    };
    Some(writer)
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

pub(crate) async fn wait_for_terminated_child(
    child: &mut tokio::process::Child,
) -> io::Result<std::process::ExitStatus> {
    match timeout(std::time::Duration::from_secs(10), child.wait()).await {
        Ok(status) => status,
        Err(_) => {
            warn!("Process did not exit after SIGTERM + 10s, sending SIGKILL");
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                kill_process_group_or_process(pid, KILL_SIGNAL);
            }
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

    // Set environment variables
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

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

    // Set environment variables
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    cmd
}

#[cfg(unix)]
const KILL_SIGNAL: i32 = libc::SIGKILL;
#[cfg(unix)]
const TERM_SIGNAL: i32 = libc::SIGTERM;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use tokio::fs;
    use tokio::time::{sleep, Duration};

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

    #[tokio::test]
    async fn test_execute_streaming_cancellation_kills_shell_child_process() {
        let script = NamedTempFile::new().unwrap();
        fs::write(
            script.path(),
            "#!/bin/sh\nsleep 30\nprintf 'unexpected completion\\n'\n",
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
        tokio::spawn(async move {
            sleep(Duration::from_millis(200)).await;
            trigger.cancel();
        });

        let mut cmd = Command::new("/bin/sh");
        cmd.arg(script.path());

        let result = execute_streaming_cancellable(
            cmd,
            &HashMap::new(),
            None,
            Some(60),
            1024 * 1024,
            1024 * 1024,
            OutputFormat::Text,
            Some(cancel_token),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

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
