use super::*;
use attune_common::{
    isolated_process::{ProcessControl, ProcessIo, ResourceIdentity},
    workload_launch::{LaunchExitOutcome, LaunchStopReason},
    workload_stdin::LaunchStdinPayload,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

struct Control {
    input: Mutex<Option<Vec<u8>>>,
    writes: AtomicUsize,
    stops: AtomicUsize,
    lose_ack: bool,
}
#[async_trait::async_trait]
impl ProcessControl for Control {
    async fn write_stdin_once(&self, payload: LaunchStdinPayload) -> attune_common::Result<()> {
        if self.writes.fetch_add(1, Ordering::SeqCst) != 0 {
            return Err(attune_common::Error::execution("Repeated input"));
        }
        *self.input.lock().unwrap() = Some(payload.as_bytes().to_vec());
        if self.lose_ack {
            return Err(attune_common::Error::execution(
                "Uncertain input acknowledgement",
            ));
        }
        Ok(())
    }
    async fn wait(&self) -> attune_common::Result<LaunchExitOutcome> {
        Ok(LaunchExitOutcome {
            exit_code: 0,
            oom_killed: false,
        })
    }
    async fn stop(&self, _: LaunchStopReason) -> attune_common::Result<LaunchExitOutcome> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        self.wait().await
    }
}
fn fixture(stdout: Vec<u8>, stderr: Vec<u8>, lose_ack: bool) -> (ProcessIo, Arc<Control>) {
    let control = Arc::new(Control {
        input: Mutex::new(None),
        writes: AtomicUsize::new(0),
        stops: AtomicUsize::new(0),
        lose_ack,
    });
    let process = ProcessIo {
        resource: ResourceIdentity::Kubernetes {
            cluster: "owned-fixture".into(),
            namespace: "owned".into(),
            pod_name: "owned-process".into(),
            pod_uid: uuid::Uuid::new_v4().to_string(),
        },
        stdout: Box::pin(std::io::Cursor::new(stdout)),
        stderr: Box::pin(std::io::Cursor::new(stderr)),
        control: control.clone(),
    };
    (process, control)
}
fn options() -> IsolatedCaptureOptions<'static> {
    IsolatedCaptureOptions {
        timeout_secs: Some(10),
        max_stdout_bytes: 1024,
        max_stderr_bytes: 1024,
        output_format: OutputFormat::Text,
        output_policy: OutputPolicy::default()
            .resolve(None, OutputFormat::Text)
            .unwrap(),
        out_schema: None,
        cancel_token: None,
        stdout_log_writer: None,
        stderr_log_writer: None,
    }
}

#[tokio::test]
async fn isolated_capture_preserves_separate_streams_and_exact_input_including_empty() {
    for input in [Vec::new(), b"{\"value\":\"private\"}\n".to_vec()] {
        let (process, control) =
            fixture(b"early stdout".to_vec(), b"separate stderr".to_vec(), false);
        let result = execute_isolated_cancellable(
            process,
            LaunchStdinPayload::from_bytes(input.clone()).unwrap(),
            options(),
        )
        .await
        .unwrap();
        assert!(result.is_success());
        assert!(!result.logs_incomplete);
        assert_eq!(result.stdout, "early stdout");
        assert_eq!(result.stderr, "separate stderr");
        assert_eq!(*control.input.lock().unwrap(), Some(input));
        assert_eq!(control.writes.load(Ordering::SeqCst), 1);
        assert_eq!(control.stops.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn isolated_capture_lost_stdin_ack_stops_without_replay_or_success() {
    let (process, control) = fixture(Vec::new(), Vec::new(), true);
    let result = execute_isolated_cancellable(
        process,
        LaunchStdinPayload::from_bytes(b"private".to_vec()).unwrap(),
        options(),
    )
    .await
    .unwrap();
    assert!(!result.is_success());
    assert!(result.error.unwrap().contains("not replayed"));
    assert_eq!(control.writes.load(Ordering::SeqCst), 1);
    assert_eq!(control.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn isolated_capture_cancellation_before_gate_never_writes_input() {
    let (process, control) = fixture(Vec::new(), Vec::new(), false);
    let mut settings = options();
    let cancel = CancellationToken::new();
    cancel.cancel();
    settings.cancel_token = Some(cancel);
    let result = execute_isolated_cancellable(
        process,
        LaunchStdinPayload::from_bytes(Vec::new()).unwrap(),
        settings,
    )
    .await
    .unwrap();
    assert!(!result.is_success());
    assert_eq!(result.error.as_deref(), Some("Execution cancelled"));
    assert_eq!(control.writes.load(Ordering::SeqCst), 0);
    assert_eq!(control.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn isolated_capture_bounds_streams_and_reports_failed_custody_without_changing_exit() {
    let (process, _) = fixture(vec![b'x'; 8192], b"stderr".to_vec(), false);
    let mut settings = options();
    settings.stdout_log_writer = Some(BoundedLogFileWriter::lazy(
        || async { Err(attune_common::Error::execution("Owned spool failure")) },
        1024,
        true,
        100,
        None,
    ));
    let result = execute_isolated_cancellable(
        process,
        LaunchStdinPayload::from_bytes(Vec::new()).unwrap(),
        settings,
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.len() <= 1024);
    assert!(result.stdout_truncated);
    assert!(result.stdout_bytes_truncated > 0);
    assert_eq!(result.stderr, "stderr");
    assert!(result.logs_incomplete);
    assert_eq!(result.log_capture_incomplete, Some((true, false)));
}
