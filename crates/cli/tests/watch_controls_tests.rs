#![cfg(unix)]

mod common;

use common::TestFixture;
use serde_json::{json, Value};
use std::{
    fs::File,
    io::Write,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    process::{Output, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::{
    process::{Child, Command},
    sync::Notify,
};
use wiremock::{
    matchers::{method, path},
    Mock, Request, Respond, ResponseTemplate,
};

#[derive(Clone)]
struct WatchReady {
    ready: Arc<Notify>,
    execution: Value,
}

impl Respond for WatchReady {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        self.ready.notify_one();
        ResponseTemplate::new(200).set_body_json(json!({"data": self.execution}))
    }
}

async fn fixture(
    status: &str,
    cancellation_status: u16,
    cancel_count: u64,
) -> (TestFixture, Arc<Notify>) {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("watch-access-token", "watch-refresh-token");
    let ready = Arc::new(Notify::new());
    let execution = json!({
        "id": 43, "action_ref": "core.echo", "status": status,
        "config": {"message": "Hello POETs"},
        "created": "2026-10-02T00:00:00Z", "updated": "2026-10-02T00:00:00Z",
    });
    Mock::given(method("POST"))
        .and(path("/api/v1/executions/execute"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"data": execution})))
        .mount(&fixture.mock_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/executions/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": execution})))
        .mount(&fixture.mock_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/executions/43"))
        .respond_with(WatchReady {
            ready: ready.clone(),
            execution,
        })
        .mount(&fixture.mock_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/executions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": []})))
        .mount(&fixture.mock_server)
        .await;
    let cancellation = if cancellation_status == 200 {
        json!({"data": {"id": 43, "status": "canceling"}})
    } else {
        json!({"error": "Cancellation forbidden"})
    };
    Mock::given(method("POST"))
        .and(path("/api/v1/executions/43/cancel"))
        .respond_with(ResponseTemplate::new(cancellation_status).set_body_json(cancellation))
        .expect(cancel_count)
        .mount(&fixture.mock_server)
        .await;
    (fixture, ready)
}

fn command(fixture: &TestFixture, args: &[&str], timeout: &str) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("attune"));
    command
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .env_remove("ATTUNE_API_TOKEN")
        .env_remove("ATTUNE_AUTH_TOKEN")
        .env_remove("ATTUNE_REFRESH_TOKEN")
        .args(["--api-url", &fixture.server_url(), "--json"])
        .args(args)
        .args([
            "--timeout",
            timeout,
            "--notifier-url",
            &fixture.server_url().replace("http:", "ws:"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn finish(child: Child) -> Output {
    tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .expect("CLI failed to finish and clean up watchers")
        .unwrap()
}

async fn ready(ready: &Notify) {
    tokio::time::timeout(Duration::from_secs(5), ready.notified())
        .await
        .expect("CLI did not begin watching");
}

fn interrupt(child: &Child) {
    // SAFETY: the child is owned by this test and remains alive until finish().
    assert_eq!(
        unsafe { libc::kill(child.id().unwrap() as i32, libc::SIGINT) },
        0
    );
}

fn watch_commands() -> Vec<Vec<&'static str>> {
    vec![
        vec!["run", "core.echo", "--param", "message=Hello POETs", "-w"],
        vec![
            "action",
            "execute",
            "core.echo",
            "--param",
            "message=Hello POETs",
            "--watch",
        ],
        vec!["execution", "rerun", "42", "--watch"],
        vec!["execution", "watch", "43"],
    ]
}

#[tokio::test]
async fn sigint_cancels_only_the_watched_execution_for_every_entry_point() {
    for args in watch_commands() {
        let (fixture, watching) = fixture("running", 200, 1).await;
        let child = command(&fixture, &args, "30")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        ready(&watching).await;
        interrupt(&child);
        let output = finish(child).await;
        assert_eq!(
            output.status.code(),
            Some(130),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output["execution_id"], 43);
        assert_eq!(output["watch_status"], "cancel_requested");
        assert_eq!(output["execution_status"], "canceling");
        fixture.mock_server.verify().await;
    }
}

struct Pty {
    master: File,
    slave: OwnedFd,
    original: libc::termios,
}

impl Pty {
    fn new() -> Self {
        let mut master = -1;
        let mut slave = -1;
        // SAFETY: openpty initializes both descriptors on success; other arguments are optional.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        // SAFETY: both descriptors are newly allocated and this object takes sole ownership.
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { OwnedFd::from_raw_fd(slave) };
        let original = attributes(&slave);
        Self {
            master,
            slave,
            original,
        }
    }

    fn attach(&self, command: &mut Command) {
        command.stdin(Stdio::from(self.slave.try_clone().unwrap()));
        // SAFETY: these are async-signal-safe operations before exec; no allocator or locks are used.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    fn assert_restored(&self) {
        let restored = attributes(&self.slave);
        assert_eq!(restored.c_iflag, self.original.c_iflag);
        assert_eq!(restored.c_oflag, self.original.c_oflag);
        assert_eq!(restored.c_cflag, self.original.c_cflag);
        assert_eq!(restored.c_lflag, self.original.c_lflag);
        assert_eq!(restored.c_cc, self.original.c_cc);
    }
}

fn attributes(slave: &OwnedFd) -> libc::termios {
    let mut value = std::mem::MaybeUninit::uninit();
    // SAFETY: the descriptor is a live PTY and tcgetattr initializes value on success.
    assert_eq!(
        unsafe { libc::tcgetattr(slave.as_raw_fd(), value.as_mut_ptr()) },
        0
    );
    // SAFETY: tcgetattr succeeded.
    unsafe { value.assume_init() }
}

#[tokio::test]
async fn ctrl_d_detaches_without_enter_or_cancellation_and_restores_the_terminal() {
    for args in watch_commands() {
        let (fixture, watching) = fixture("running", 200, 0).await;
        let mut pty = Pty::new();
        let mut cmd = command(&fixture, &args, "30");
        pty.attach(&mut cmd);
        let child = cmd.spawn().unwrap();
        ready(&watching).await;
        let watching_mode = attributes(&pty.slave);
        assert_eq!(watching_mode.c_lflag & libc::ICANON, 0);
        assert_ne!(watching_mode.c_lflag & libc::ISIG, 0);
        assert_eq!(watching_mode.c_oflag, pty.original.c_oflag);
        pty.master.write_all(b"ordinary text\x04").unwrap();
        let output = finish(child).await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output["watch_status"], "detached");
        assert_eq!(output["execution_id"], 43);
        pty.assert_restored();
        fixture.mock_server.verify().await;
    }
}

#[tokio::test]
async fn terminal_ctrl_c_requests_cancellation_without_confirmation() {
    let (fixture, watching) = fixture("running", 200, 1).await;
    let mut pty = Pty::new();
    let mut cmd = command(&fixture, &["execution", "watch", "43"], "30");
    pty.attach(&mut cmd);
    let child = cmd.spawn().unwrap();
    ready(&watching).await;
    pty.master.write_all(b"\x03").unwrap();
    let output = finish(child).await;
    assert_eq!(
        output.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    pty.assert_restored();
    fixture.mock_server.verify().await;
}

#[tokio::test]
async fn cancellation_failure_is_reported_and_restores_the_terminal() {
    let (fixture, watching) = fixture("running", 403, 1).await;
    let pty = Pty::new();
    let mut cmd = command(&fixture, &["execution", "watch", "43"], "30");
    pty.attach(&mut cmd);
    let child = cmd.spawn().unwrap();
    ready(&watching).await;
    interrupt(&child);
    let output = finish(child).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cancellation forbidden"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("cancel_requested"));
    pty.assert_restored();
    fixture.mock_server.verify().await;
}

#[tokio::test]
async fn closed_stdin_does_not_detach_or_cancel_a_scripted_watch() {
    let (fixture, watching) = fixture("running", 200, 0).await;
    let mut child = command(&fixture, &["execution", "watch", "43"], "1")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdin.take());
    ready(&watching).await;
    let output = finish(child).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("timed out"));
    assert!(output.stdout.is_empty());
    fixture.mock_server.verify().await;
}

#[tokio::test]
async fn completion_and_timeout_restore_terminal_without_cancellation() {
    for status in ["completed", "running"] {
        let (fixture, _) = fixture(status, 200, 0).await;
        let pty = Pty::new();
        let mut cmd = command(&fixture, &["execution", "watch", "43"], "1");
        pty.attach(&mut cmd);
        let output = finish(cmd.spawn().unwrap()).await;
        assert_eq!(
            output.status.success(),
            status == "completed",
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        pty.assert_restored();
        fixture.mock_server.verify().await;
    }
}
