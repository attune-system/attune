//! Waiting for execution completion.
//!
//! Tries to connect to the notifier WebSocket first so the CLI reacts
//! *immediately* when the execution reaches a terminal state.  If the
//! notifier is unreachable (not configured, different port, Docker network
//! boundary, etc.) it transparently falls back to REST polling.
//!
//! Public surface:
//!   - [`WaitOptions`]  – caller-supplied parameters
//!   - [`wait_for_execution`] – the single entry point

use anyhow::Result;
use chrono::{DateTime, Utc};
use colored::Colorize;
use eventsource_stream::Eventsource;
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use reqwest::header;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{self, IsTerminal, Write};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use terminal_size::{terminal_size, Width};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async, tungstenite::client::IntoClientRequest, tungstenite::Message, MaybeTlsStream,
    WebSocketStream,
};

use crate::client::ApiClient;

// ── terminal status helpers ───────────────────────────────────────────────────

fn is_terminal(status: &str) -> bool {
    matches!(
        status,
        "completed" | "succeeded" | "failed" | "canceled" | "cancelled" | "timeout" | "timed_out"
    )
}

// ── public types ─────────────────────────────────────────────────────────────

/// Result returned when the wait completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSummary {
    pub id: i64,
    pub status: String,
    pub action_ref: String,
    pub result: Option<serde_json::Value>,
    pub created: String,
    pub updated: String,
}

/// Parameters that control how we wait.
pub struct WaitOptions<'a> {
    /// Execution ID to watch.
    pub execution_id: i64,
    /// Overall wall-clock limit (seconds). Defaults to 300 if `None`.
    pub timeout_secs: u64,
    /// REST API client (already authenticated).
    pub api_client: &'a mut ApiClient,
    /// Base URL of the *notifier* WebSocket service, e.g. `ws://localhost:8081`.
    /// Derived from the API URL when not explicitly set.
    pub notifier_ws_url: Option<String>,
    /// If `true`, print progress lines to stderr.
    pub verbose: bool,
}

pub struct OutputWatchTask {
    pub handle: tokio::task::JoinHandle<()>,
    delivered_output: Arc<AtomicBool>,
    root_stdout_completed: Arc<AtomicBool>,
}

impl OutputWatchTask {
    pub async fn finish_or_stop(mut self, grace: Duration) -> (bool, bool) {
        // Terminal status can precede the final SSE chunks. Give active streams
        // a bounded chance to flush, then cancel stale watchers.
        if tokio::time::timeout(grace, &mut self.handle).await.is_err() {
            self.handle.abort();
            let _ = self.handle.await;
        }
        (
            self.delivered_output.load(Ordering::Relaxed),
            self.root_stdout_completed.load(Ordering::Relaxed),
        )
    }
}

// ── notifier WebSocket messages (mirrors websocket_server.rs) ────────────────

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ClientMsg {
    #[serde(rename = "subscribe")]
    Subscribe { filter: String },
    #[serde(rename = "ping")]
    Ping,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ServerMsg {
    #[serde(rename = "welcome")]
    Welcome {
        client_id: String,
        #[allow(dead_code)]
        message: String,
    },
    #[serde(rename = "notification")]
    Notification(NotifierNotification),
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct NotifierNotification {
    pub notification_type: String,
    pub entity_type: String,
    pub entity_id: i64,
    pub payload: serde_json::Value,
}

// ── REST execution shape ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct RestExecution {
    id: i64,
    action_ref: String,
    status: String,
    #[serde(default)]
    result: Option<serde_json::Value>,
    created: String,
    #[serde(default)]
    started_at: Option<String>,
    updated: String,
}

#[derive(Debug, Clone, Deserialize)]
struct WorkflowTaskMetadata {
    task_name: String,
    #[serde(default)]
    task_index: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
struct ExecutionListItem {
    id: i64,
    action_ref: String,
    status: String,
    #[serde(default)]
    parent: Option<i64>,
    #[serde(default)]
    started_at: Option<String>,
    updated: String,
    #[serde(default)]
    workflow_task: Option<WorkflowTaskMetadata>,
}

#[derive(Debug)]
struct ChildWatchState {
    label: String,
    status: String,
    announced_terminal: bool,
    log_streaming_supported: Option<bool>,
    stream_handles: Vec<StreamWatchHandle>,
}

struct RootWatchState {
    stream_handles: Vec<StreamWatchHandle>,
}

#[derive(Debug)]
struct StreamWatchHandle {
    stream_name: &'static str,
    offset: Arc<AtomicU64>,
    handle: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
struct StreamWatchConfig {
    base_url: String,
    token: String,
    execution_id: i64,
    prefix: Option<String>,
    debug: bool,
    emit_output: bool,
    task_id: Option<i64>,
    live_renderer: Option<LiveRenderer>,
    delivered_output: Arc<AtomicBool>,
    root_stdout_completed: Option<Arc<AtomicBool>>,
}

struct StreamLogTask {
    stream_name: &'static str,
    offset: Arc<AtomicU64>,
    config: StreamWatchConfig,
}

type ExecutionWsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type ExecutionWsWrite = SplitSink<ExecutionWsStream, Message>;
type ExecutionWsRead = SplitStream<ExecutionWsStream>;

struct ExecutionNotifier {
    ws_stream: ExecutionWsStream,
    next_ping: Instant,
}

struct ExecutionNotifierUpdates {
    root_execution: Option<RestExecution>,
    descendants: Vec<ExecutionListItem>,
}

struct WatchExecutionContext {
    execution_id: i64,
    notifier_ws_url: Option<String>,
    stream_logs: bool,
    debug: bool,
    base_url: String,
    live_renderer: LiveRenderer,
    plain_progress: bool,
    delivered_output: Arc<AtomicBool>,
    root_stdout_completed: Arc<AtomicBool>,
}

struct ChildUpdateContext<'a> {
    client: &'a mut ApiClient,
    children: &'a mut HashMap<i64, ChildWatchState>,
    stream_logs: bool,
    debug: bool,
    live_renderer: &'a LiveRenderer,
    plain_progress: bool,
    base_url: &'a str,
    delivered_output: &'a Arc<AtomicBool>,
}

const MAX_TASK_TAIL_LINES: usize = 4;
const RENDER_TICK: Duration = Duration::from_millis(120);
const WATCH_ROOT_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const WATCH_DESCENDANT_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const WEBSOCKET_RECONCILE_INTERVAL: Duration = Duration::from_secs(2);
pub const OUTPUT_WATCH_DRAIN_GRACE: Duration = Duration::from_secs(2);
const WEBSOCKET_CLOSE_TIMEOUT: Duration = Duration::from_millis(250);
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Debug, Clone)]
struct LiveTaskState {
    label: String,
    task_name: String,
    is_root: bool,
    is_iterated: bool,
    action_ref: String,
    status: String,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    stderr_lines: VecDeque<String>,
    stdout_lines: VecDeque<String>,
}

#[derive(Debug, Clone)]
struct LiveTaskUpdate {
    id: i64,
    label: String,
    task_name: String,
    is_root: bool,
    is_iterated: bool,
    action_ref: String,
    status: String,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Default)]
struct IteratedTaskSummary {
    task_name: String,
    pending: usize,
    running: usize,
    completed: usize,
    failed: usize,
    first_started_at: Option<DateTime<Utc>>,
    first_seen_id: i64,
}

#[derive(Debug, Default)]
struct LiveRendererState {
    tasks: BTreeMap<i64, LiveTaskState>,
    rendered_lines: usize,
}

#[derive(Clone)]
struct LiveRenderer {
    enabled: bool,
    state: Arc<Mutex<LiveRendererState>>,
    stop: Arc<AtomicBool>,
}

impl LiveRenderer {
    fn new(show_progress: bool) -> Self {
        Self {
            enabled: show_progress && io::stderr().is_terminal(),
            state: Arc::new(Mutex::new(LiveRendererState::default())),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn spawn(&self) -> Option<tokio::task::JoinHandle<()>> {
        if !self.enabled {
            return None;
        }

        let renderer = self.clone();
        Some(tokio::spawn(async move {
            loop {
                renderer.render(false);
                if renderer.stop.load(Ordering::Relaxed) {
                    renderer.render(true);
                    break;
                }
                tokio::time::sleep(RENDER_TICK).await;
            }
        }))
    }

    fn stop(&self) {
        if self.enabled {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    fn upsert_task(&self, update: LiveTaskUpdate) {
        if !self.enabled {
            return;
        }

        let LiveTaskUpdate {
            id,
            label,
            task_name,
            is_root,
            is_iterated,
            action_ref,
            status,
            started_at,
            finished_at,
        } = update;

        let mut state = self.state.lock().expect("live renderer poisoned");
        let entry = state.tasks.entry(id).or_insert_with(|| LiveTaskState {
            label: label.clone(),
            task_name: task_name.clone(),
            is_root,
            is_iterated,
            action_ref: action_ref.clone(),
            status: status.clone(),
            started_at,
            finished_at,
            stderr_lines: VecDeque::new(),
            stdout_lines: VecDeque::new(),
        });
        entry.label = label;
        entry.task_name = task_name;
        entry.is_root = is_root;
        entry.is_iterated = is_iterated;
        entry.action_ref = action_ref;
        entry.status = status.clone();
        entry.started_at = started_at.or(entry.started_at);
        entry.finished_at = if is_terminal(&status) {
            finished_at
        } else {
            None
        };
        if should_clear_task_tail(&status) {
            entry.stderr_lines.clear();
            entry.stdout_lines.clear();
        }
    }

    fn push_line(&self, id: i64, stream_name: &str, line: String) {
        if !self.enabled || line.is_empty() {
            return;
        }

        let mut state = self.state.lock().expect("live renderer poisoned");
        if let Some(task) = state.tasks.get_mut(&id) {
            if should_clear_task_tail(&task.status) {
                task.stderr_lines.clear();
                task.stdout_lines.clear();
                return;
            }

            let target_lines = if stream_name == "stdout" {
                &mut task.stdout_lines
            } else {
                &mut task.stderr_lines
            };
            target_lines.push_back(truncate_log_line(&line));
            while target_lines.len() > MAX_TASK_TAIL_LINES {
                target_lines.pop_front();
            }
        }
    }

    fn render(&self, force: bool) {
        if !self.enabled {
            return;
        }

        let mut state = self.state.lock().expect("live renderer poisoned");
        if state.tasks.is_empty() && !force {
            return;
        }

        let now = Instant::now();
        let width = current_terminal_width();
        let has_child_tasks = state.tasks.values().any(|task| !task.is_root);
        let iterated_summaries = build_iterated_summaries(&state.tasks);
        let mut lines = Vec::new();
        let mut summary_by_name = HashMap::new();
        for summary in &iterated_summaries {
            summary_by_name.insert(summary.task_name.as_str(), summary);
        }

        let mut items = state
            .tasks
            .iter()
            .filter_map(|(id, task)| {
                if task.is_root && has_child_tasks {
                    return None;
                }
                if task.is_iterated {
                    return should_render_iterated_task(task).then(|| RenderItem {
                        group_started_at: summary_by_name
                            .get(task.task_name.as_str())
                            .and_then(|summary| summary.first_started_at.as_ref())
                            .map(DateTime::timestamp_millis),
                        group_id: summary_by_name
                            .get(task.task_name.as_str())
                            .map_or(*id, |summary| summary.first_seen_id),
                        within_group_rank: 1,
                        started_at: task.started_at.as_ref().map(DateTime::timestamp_millis),
                        id: *id,
                        kind: RenderItemKind::Task(task),
                    });
                }

                Some(RenderItem {
                    group_started_at: task.started_at.as_ref().map(DateTime::timestamp_millis),
                    group_id: *id,
                    within_group_rank: 0,
                    started_at: task.started_at.as_ref().map(DateTime::timestamp_millis),
                    id: *id,
                    kind: RenderItemKind::Task(task),
                })
            })
            .collect::<Vec<_>>();

        items.extend(iterated_summaries.iter().map(|summary| {
            RenderItem {
                group_started_at: summary
                    .first_started_at
                    .as_ref()
                    .map(DateTime::timestamp_millis),
                group_id: summary.first_seen_id,
                within_group_rank: 0,
                started_at: summary
                    .first_started_at
                    .as_ref()
                    .map(DateTime::timestamp_millis),
                id: summary.first_seen_id,
                kind: RenderItemKind::IteratedSummary(summary),
            }
        }));
        items.sort_by(render_item_cmp);

        for item in items {
            match item.kind {
                RenderItemKind::Task(task) => lines.extend(render_task_lines(task, now, width)),
                RenderItemKind::IteratedSummary(summary) => {
                    lines.push(render_iterated_summary_line(summary, now, width))
                }
            }
        }

        let mut stderr = io::stderr().lock();
        if state.rendered_lines > 0 {
            let _ = write!(stderr, "\x1b[{}F\x1b[J", state.rendered_lines);
        }
        for line in &lines {
            let _ = writeln!(stderr, "{line}");
        }
        let _ = stderr.flush();
        state.rendered_lines = lines.len();
    }
}

#[derive(Clone, Copy)]
struct RenderItem<'a> {
    group_started_at: Option<i64>,
    group_id: i64,
    within_group_rank: u8,
    started_at: Option<i64>,
    id: i64,
    kind: RenderItemKind<'a>,
}

#[derive(Clone, Copy)]
enum RenderItemKind<'a> {
    Task(&'a LiveTaskState),
    IteratedSummary(&'a IteratedTaskSummary),
}

impl From<RestExecution> for ExecutionSummary {
    fn from(e: RestExecution) -> Self {
        Self {
            id: e.id,
            status: e.status,
            action_ref: e.action_ref,
            result: e.result,
            created: e.created,
            updated: e.updated,
        }
    }
}

// ── entry point ───────────────────────────────────────────────────────────────

/// Wait for `execution_id` to reach a terminal status.
///
/// 1. Attempts a WebSocket connection to the notifier and subscribes to the
///    specific execution with the filter `entity:execution:<id>`.
/// 2. If the connection fails (or the notifier URL can't be derived) it falls
///    back to polling `GET /executions/<id>` every 2 seconds.
/// 3. In both cases, an overall `timeout_secs` wall-clock limit is enforced.
///
/// Returns the final [`ExecutionSummary`] on success or an error if the
/// timeout is exceeded or a fatal error occurs.
pub async fn wait_for_execution(opts: WaitOptions<'_>) -> Result<ExecutionSummary> {
    let overall_deadline = Instant::now() + Duration::from_secs(opts.timeout_secs);

    // Reserve at least this long for polling after WebSocket gives up.
    // This ensures the polling fallback always gets a fair chance even when
    // the WS path consumes most of the timeout budget.
    const MIN_POLL_BUDGET: Duration = Duration::from_secs(10);

    // Try WebSocket path first; fall through to polling on any connection error.
    if let Some(ws_url) = resolve_ws_url(&opts) {
        // Give WS at most (timeout - MIN_POLL_BUDGET) so polling always has headroom.
        let ws_deadline = if overall_deadline > Instant::now() + MIN_POLL_BUDGET {
            overall_deadline - MIN_POLL_BUDGET
        } else {
            // Timeout is very short; skip WS entirely and go straight to polling.
            overall_deadline
        };

        match wait_via_websocket(
            &ws_url,
            opts.execution_id,
            ws_deadline,
            opts.verbose,
            opts.api_client,
        )
        .await
        {
            Ok(summary) => return Ok(summary),
            Err(ws_err) => {
                if opts.verbose {
                    eprintln!("  [notifier: {}] falling back to polling", ws_err);
                }
                // Fall through to polling below.
            }
        }
    } else if opts.verbose {
        eprintln!("  [notifier URL not configured] using polling");
    }

    // Polling always uses the full overall deadline, so at minimum MIN_POLL_BUDGET
    // remains (and often the full timeout if WS failed at connect time).
    wait_via_polling(
        opts.api_client,
        opts.execution_id,
        overall_deadline,
        opts.verbose,
    )
    .await
}

pub fn spawn_execution_output_watch(
    mut client: ApiClient,
    execution_id: i64,
    notifier_ws_url: Option<String>,
    show_progress: bool,
    stream_logs: bool,
    debug: bool,
) -> OutputWatchTask {
    let delivered_output = Arc::new(AtomicBool::new(false));
    let root_stdout_completed = Arc::new(AtomicBool::new(false));
    let live_renderer = LiveRenderer::new(show_progress);
    let plain_progress = show_progress && !live_renderer.enabled();
    let watch_ctx = WatchExecutionContext {
        execution_id,
        notifier_ws_url,
        stream_logs,
        debug,
        base_url: client.base_url().to_string(),
        live_renderer,
        plain_progress,
        delivered_output: delivered_output.clone(),
        root_stdout_completed: root_stdout_completed.clone(),
    };
    let handle = tokio::spawn(async move {
        if let Err(err) = watch_execution_output(&mut client, watch_ctx).await {
            if debug {
                eprintln!("  [watch] {}", err);
            }
        }
    });

    OutputWatchTask {
        handle,
        delivered_output,
        root_stdout_completed,
    }
}

async fn watch_execution_output(client: &mut ApiClient, ctx: WatchExecutionContext) -> Result<()> {
    let WatchExecutionContext {
        execution_id,
        notifier_ws_url,
        stream_logs,
        debug,
        base_url,
        live_renderer,
        plain_progress,
        delivered_output,
        root_stdout_completed,
    } = ctx;
    let render_handle = live_renderer.spawn();
    let mut root_watch: Option<RootWatchState> = None;
    let mut children: HashMap<i64, ChildWatchState> = HashMap::new();
    let mut next_descendant_refresh = Instant::now();
    let mut known_execution_ids = HashSet::from([execution_id]);
    let mut execution = client
        .get::<RestExecution>(&format!("/executions/{}", execution_id))
        .await?;
    let mut execution_notifier =
        connect_execution_notifier(notifier_ws_url.as_deref(), &base_url, client, debug)
            .await
            .ok();

    if let Ok(initial_children) = list_descendant_executions(client, execution_id).await {
        for child in initial_children {
            known_execution_ids.insert(child.id);
            apply_child_execution_update(
                ChildUpdateContext {
                    client,
                    children: &mut children,
                    stream_logs,
                    debug,
                    live_renderer: &live_renderer,
                    plain_progress,
                    base_url: &base_url,
                    delivered_output: &delivered_output,
                },
                child,
            )
            .await;
        }
        next_descendant_refresh = Instant::now() + WATCH_DESCENDANT_REFRESH_INTERVAL;
    }

    loop {
        if execution_notifier.is_none() {
            execution = client.get(&format!("/executions/{}", execution_id)).await?;
        }

        let root_started_at = execution
            .started_at
            .as_deref()
            .and_then(parse_api_timestamp);
        let root_finished_at = if is_terminal(&execution.status) {
            parse_api_timestamp(&execution.updated)
        } else {
            None
        };

        if live_renderer.enabled() {
            live_renderer.upsert_task(LiveTaskUpdate {
                id: execution.id,
                label: format!("execution#{}", execution.id),
                task_name: execution.action_ref.clone(),
                is_root: true,
                is_iterated: false,
                action_ref: execution.action_ref.clone(),
                status: execution.status.clone(),
                started_at: root_started_at,
                finished_at: root_finished_at,
            });
        }

        if stream_logs
            && should_stream_root_logs(&execution)
            && root_watch
                .as_ref()
                .is_none_or(|state| streams_need_restart(&state.stream_handles))
        {
            if let Some(token) = client.auth_token().map(str::to_string) {
                match root_watch.as_mut() {
                    Some(state) => restart_finished_streams(
                        &mut state.stream_handles,
                        &StreamWatchConfig {
                            base_url: base_url.clone(),
                            token,
                            execution_id,
                            prefix: None,
                            debug,
                            emit_output: !live_renderer.enabled(),
                            task_id: live_renderer.enabled().then_some(execution_id),
                            live_renderer: live_renderer.enabled().then_some(live_renderer.clone()),
                            delivered_output: delivered_output.clone(),
                            root_stdout_completed: Some(root_stdout_completed.clone()),
                        },
                    ),
                    None => {
                        root_watch = Some(RootWatchState {
                            stream_handles: spawn_execution_log_streams(StreamWatchConfig {
                                base_url: base_url.clone(),
                                token,
                                execution_id,
                                debug,
                                prefix: None,
                                emit_output: !live_renderer.enabled(),
                                task_id: live_renderer.enabled().then_some(execution_id),
                                live_renderer: live_renderer
                                    .enabled()
                                    .then_some(live_renderer.clone()),
                                delivered_output: delivered_output.clone(),
                                root_stdout_completed: Some(root_stdout_completed.clone()),
                            }),
                        });
                    }
                }
            }
        }

        let mut used_notifier_update = false;
        if let Some(notifier) = execution_notifier.as_mut() {
            match notifier
                .drain_execution_updates(execution_id, &mut known_execution_ids, debug)
                .await
            {
                Ok(notifier_updates) => {
                    used_notifier_update = true;
                    if let Some(root_update) = notifier_updates.root_execution {
                        execution = root_update;
                    }
                    for child in notifier_updates.descendants {
                        apply_child_execution_update(
                            ChildUpdateContext {
                                client,
                                children: &mut children,
                                stream_logs,
                                debug,
                                live_renderer: &live_renderer,
                                plain_progress,
                                base_url: &base_url,
                                delivered_output: &delivered_output,
                            },
                            child,
                        )
                        .await;
                    }
                }
                Err(err) => {
                    if debug {
                        eprintln!("  [watch] execution notifier unavailable: {}", err);
                    }
                    execution_notifier = None;
                    next_descendant_refresh = Instant::now();
                }
            }
        }

        if !used_notifier_update
            && (Instant::now() >= next_descendant_refresh || is_terminal(&execution.status))
        {
            let child_items = list_descendant_executions(client, execution_id)
                .await
                .unwrap_or_default();
            next_descendant_refresh = Instant::now() + WATCH_DESCENDANT_REFRESH_INTERVAL;

            for child in child_items {
                known_execution_ids.insert(child.id);
                apply_child_execution_update(
                    ChildUpdateContext {
                        client,
                        children: &mut children,
                        stream_logs,
                        debug,
                        live_renderer: &live_renderer,
                        plain_progress,
                        base_url: &base_url,
                        delivered_output: &delivered_output,
                    },
                    child,
                )
                .await;
            }
        }

        if is_terminal(&execution.status) {
            break;
        }

        tokio::time::sleep(WATCH_ROOT_REFRESH_INTERVAL).await;
    }

    if let Some(mut notifier) = execution_notifier {
        notifier.close().await;
    }

    if let Some(root_watch) = root_watch {
        wait_for_stream_handles(root_watch.stream_handles).await;
    }

    for child in children.into_values() {
        wait_for_stream_handles(child.stream_handles).await;
    }

    if live_renderer.enabled() {
        if let Ok(final_children) = list_descendant_executions(client, execution_id).await {
            for child in final_children {
                let label = format_task_label(&child.workflow_task, &child.action_ref, child.id);
                let task_name = child
                    .workflow_task
                    .as_ref()
                    .map(|task| task.task_name.clone())
                    .unwrap_or_else(|| label.clone());
                let is_iterated = child
                    .workflow_task
                    .as_ref()
                    .and_then(|task| task.task_index)
                    .is_some();
                let started_at = child.started_at.as_deref().and_then(parse_api_timestamp);
                let finished_at = if is_terminal(&child.status) {
                    parse_api_timestamp(&child.updated)
                } else {
                    None
                };
                live_renderer.upsert_task(LiveTaskUpdate {
                    id: child.id,
                    label,
                    task_name,
                    is_root: false,
                    is_iterated,
                    action_ref: child.action_ref,
                    status: child.status,
                    started_at,
                    finished_at,
                });
            }
        }
    }

    live_renderer.stop();
    if let Some(handle) = render_handle {
        let _ = handle.await;
    }

    Ok(())
}

fn spawn_execution_log_streams(config: StreamWatchConfig) -> Vec<StreamWatchHandle> {
    ["stdout", "stderr"]
        .into_iter()
        .map(|stream_name| {
            let offset = Arc::new(AtomicU64::new(0));
            let completion_flag = if stream_name == "stdout" {
                config.root_stdout_completed.clone()
            } else {
                None
            };
            StreamWatchHandle {
                stream_name,
                handle: tokio::spawn(stream_execution_log(StreamLogTask {
                    stream_name,
                    offset: offset.clone(),
                    config: StreamWatchConfig {
                        base_url: config.base_url.clone(),
                        token: config.token.clone(),
                        execution_id: config.execution_id,
                        prefix: config.prefix.clone(),
                        debug: config.debug,
                        emit_output: config.emit_output,
                        task_id: config.task_id,
                        live_renderer: config.live_renderer.clone(),
                        delivered_output: config.delivered_output.clone(),
                        root_stdout_completed: completion_flag,
                    },
                })),
                offset,
            }
        })
        .collect()
}

fn streams_need_restart(handles: &[StreamWatchHandle]) -> bool {
    handles.is_empty() || handles.iter().any(|handle| handle.handle.is_finished())
}

fn ensure_streams_running(handles: &mut Vec<StreamWatchHandle>, config: &StreamWatchConfig) {
    if handles.is_empty() {
        *handles = spawn_execution_log_streams(config.clone());
        return;
    }

    restart_finished_streams(handles, config);
}

fn restart_finished_streams(handles: &mut [StreamWatchHandle], config: &StreamWatchConfig) {
    for stream in handles.iter_mut() {
        if stream.handle.is_finished() {
            let offset = stream.offset.clone();
            let completion_flag = if stream.stream_name == "stdout" {
                config.root_stdout_completed.clone()
            } else {
                None
            };
            stream.handle = tokio::spawn(stream_execution_log(StreamLogTask {
                stream_name: stream.stream_name,
                offset,
                config: StreamWatchConfig {
                    base_url: config.base_url.clone(),
                    token: config.token.clone(),
                    execution_id: config.execution_id,
                    prefix: config.prefix.clone(),
                    debug: config.debug,
                    emit_output: config.emit_output,
                    task_id: config.task_id,
                    live_renderer: config.live_renderer.clone(),
                    delivered_output: config.delivered_output.clone(),
                    root_stdout_completed: completion_flag,
                },
            }));
        }
    }
}

async fn wait_for_stream_handles(handles: Vec<StreamWatchHandle>) {
    for handle in handles {
        let _ = handle.handle.await;
    }
}

async fn list_direct_child_executions(
    client: &mut ApiClient,
    execution_id: i64,
) -> Result<Vec<ExecutionListItem>> {
    const PER_PAGE: u32 = 100;

    let mut page = 1;
    let mut all_children = Vec::new();

    loop {
        let path = format!("/executions?parent={execution_id}&page={page}&per_page={PER_PAGE}");
        let mut page_items: Vec<ExecutionListItem> = client.get_paginated(&path).await?;
        page_items.sort_by_key(|item| item.id);
        let page_len = page_items.len();
        all_children.append(&mut page_items);

        if page_len < PER_PAGE as usize {
            break;
        }

        page += 1;
    }

    Ok(all_children)
}

async fn list_descendant_executions(
    client: &mut ApiClient,
    execution_id: i64,
) -> Result<Vec<ExecutionListItem>> {
    let mut pending_parents = VecDeque::from([execution_id]);
    let mut seen_ids = HashSet::new();
    let mut descendants = Vec::new();

    while let Some(parent_id) = pending_parents.pop_front() {
        for child in list_direct_child_executions(client, parent_id).await? {
            if seen_ids.insert(child.id) {
                pending_parents.push_back(child.id);
                descendants.push(child);
            }
        }
    }

    descendants.sort_by_key(|item| item.id);
    Ok(descendants)
}

async fn apply_child_execution_update(ctx: ChildUpdateContext<'_>, child: ExecutionListItem) {
    let label = format_task_label(&child.workflow_task, &child.action_ref, child.id);
    let task_name = child
        .workflow_task
        .as_ref()
        .map(|task| task.task_name.clone())
        .unwrap_or_else(|| label.clone());
    let is_iterated = child
        .workflow_task
        .as_ref()
        .and_then(|task| task.task_index)
        .is_some();
    let started_at = child.started_at.as_deref().and_then(parse_api_timestamp);
    let finished_at = if is_terminal(&child.status) {
        parse_api_timestamp(&child.updated)
    } else {
        None
    };
    let (child_is_terminal, mut log_streaming_supported) = {
        let entry = ctx.children.entry(child.id).or_insert_with(|| {
            if ctx.plain_progress {
                eprintln!("  [{}] started ({})", label, child.action_ref);
            }
            if ctx.live_renderer.enabled() {
                ctx.live_renderer.upsert_task(LiveTaskUpdate {
                    id: child.id,
                    label: label.clone(),
                    task_name: task_name.clone(),
                    is_root: false,
                    is_iterated,
                    action_ref: child.action_ref.clone(),
                    status: child.status.clone(),
                    started_at,
                    finished_at,
                });
            }
            ChildWatchState {
                label,
                status: child.status.clone(),
                announced_terminal: false,
                log_streaming_supported: None,
                stream_handles: Vec::new(),
            }
        });

        if entry.status != child.status {
            entry.status = child.status.clone();
        }
        if ctx.live_renderer.enabled() {
            ctx.live_renderer.upsert_task(LiveTaskUpdate {
                id: child.id,
                label: entry.label.clone(),
                task_name: task_name.clone(),
                is_root: false,
                is_iterated,
                action_ref: child.action_ref.clone(),
                status: child.status.clone(),
                started_at,
                finished_at,
            });
        }

        (is_terminal(&entry.status), entry.log_streaming_supported)
    };

    if ctx.stream_logs
        && !child_is_terminal
        && should_stream_logs(&child)
        && log_streaming_supported.is_none()
    {
        log_streaming_supported = Some(true);
    }

    let entry = ctx
        .children
        .get_mut(&child.id)
        .expect("child state should exist after insertion");
    if entry.log_streaming_supported.is_none() {
        entry.log_streaming_supported = log_streaming_supported;
    }

    if ctx.stream_logs
        && !child_is_terminal
        && entry.log_streaming_supported == Some(true)
        && streams_need_restart(&entry.stream_handles)
    {
        if let Some(token) = ctx.client.auth_token().map(str::to_string) {
            ensure_streams_running(
                &mut entry.stream_handles,
                &StreamWatchConfig {
                    base_url: ctx.base_url.to_string(),
                    token,
                    execution_id: child.id,
                    prefix: Some(entry.label.clone()),
                    debug: ctx.debug,
                    emit_output: !ctx.live_renderer.enabled(),
                    task_id: Some(child.id),
                    live_renderer: ctx
                        .live_renderer
                        .enabled()
                        .then_some(ctx.live_renderer.clone()),
                    delivered_output: ctx.delivered_output.clone(),
                    root_stdout_completed: None,
                },
            );
        }
    }

    if !entry.announced_terminal && is_terminal(&child.status) {
        entry.announced_terminal = true;
        if ctx.plain_progress {
            eprintln!("  [{}] {}", entry.label, child.status);
        }
    }
}

async fn connect_execution_notifier(
    explicit_ws_url: Option<&str>,
    api_base_url: &str,
    client: &ApiClient,
    verbose: bool,
) -> Result<ExecutionNotifier> {
    let ws_base_url = explicit_ws_url
        .map(ToOwned::to_owned)
        .or_else(|| derive_notifier_url(api_base_url))
        .ok_or_else(|| anyhow::anyhow!("notifier URL not configured"))?;
    let ws_url = format!("{}/ws", ws_base_url.trim_end_matches('/'));
    let request = build_notifier_ws_request(&ws_url, client.auth_token())?;

    let (mut ws_stream, _) = tokio::time::timeout(Duration::from_secs(5), connect_async(request))
        .await
        .map_err(|_| anyhow::anyhow!("WebSocket connect timed out"))??;

    let subscribe_result: Result<()> = async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(msg) = ws_stream.next().await {
                if let Ok(Message::Text(txt)) = msg {
                    if let Ok(ServerMsg::Welcome { client_id, .. }) =
                        serde_json::from_str::<ServerMsg>(&txt)
                    {
                        if verbose {
                            eprintln!("  [watch:notifier] session id {}", client_id);
                        }
                        return Ok(());
                    }
                }
            }
            anyhow::bail!("connection closed before welcome")
        })
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for welcome message"))??;
        let subscribe_msg = ClientMsg::Subscribe {
            filter: "entity_type:execution".to_string(),
        };
        ws_stream
            .send(Message::Text(serde_json::to_string(&subscribe_msg)?.into()))
            .await?;

        if verbose {
            eprintln!("  [watch:notifier] subscribed to entity_type:execution");
        }

        Ok(())
    }
    .await;

    if subscribe_result.is_err() {
        graceful_close_websocket(&mut ws_stream).await;
    }
    subscribe_result?;

    Ok(ExecutionNotifier {
        ws_stream,
        next_ping: Instant::now() + Duration::from_secs(15),
    })
}

fn build_notifier_ws_request(
    ws_url: &str,
    auth_token: Option<&str>,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>> {
    let mut request = ws_url.into_client_request()?;
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", "attune.v1".parse()?);
    if let Some(token) = auth_token.map(str::trim).filter(|token| !token.is_empty()) {
        request
            .headers_mut()
            .insert("Authorization", format!("Bearer {token}").parse()?);
    }
    Ok(request)
}

impl ExecutionNotifier {
    async fn close(&mut self) {
        graceful_close_websocket(&mut self.ws_stream).await;
    }

    async fn drain_execution_updates(
        &mut self,
        root_execution_id: i64,
        known_execution_ids: &mut HashSet<i64>,
        verbose: bool,
    ) -> Result<ExecutionNotifierUpdates> {
        let mut root_execution = None;
        let mut descendants = Vec::new();

        loop {
            match tokio::time::timeout(Duration::from_millis(1), self.ws_stream.next()).await {
                Err(_) => break,
                Ok(Some(Ok(Message::Text(txt)))) => match serde_json::from_str::<ServerMsg>(&txt) {
                    Ok(ServerMsg::Notification(notification)) => {
                        match notification_to_execution_update(
                            root_execution_id,
                            known_execution_ids,
                            notification,
                        ) {
                            Some(ExecutionUpdate::Root(execution)) => {
                                root_execution = Some(execution);
                            }
                            Some(ExecutionUpdate::Descendant(execution)) => {
                                known_execution_ids.insert(execution.id);
                                descendants.push(execution);
                            }
                            None => {}
                        }
                    }
                    Ok(ServerMsg::Error { message }) => {
                        anyhow::bail!("notifier error: {}", message);
                    }
                    Ok(ServerMsg::Welcome { .. } | ServerMsg::Unknown) => {}
                    Err(err) => {
                        if verbose {
                            eprintln!("  [watch:notifier] ignoring unrecognised message: {}", err);
                        }
                    }
                },
                Ok(Some(Ok(
                    Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_),
                ))) => {}
                Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                    anyhow::bail!("notifier WebSocket closed unexpectedly");
                }
                Ok(Some(Err(err))) => {
                    anyhow::bail!("WebSocket error: {}", err);
                }
            }
        }

        let now = Instant::now();
        if now >= self.next_ping {
            self.ws_stream
                .send(Message::Text(
                    serde_json::to_string(&ClientMsg::Ping)?.into(),
                ))
                .await?;
            self.next_ping = now + Duration::from_secs(15);
        }

        Ok(ExecutionNotifierUpdates {
            root_execution,
            descendants,
        })
    }
}

async fn graceful_close_websocket(ws_stream: &mut ExecutionWsStream) {
    let _ = tokio::time::timeout(WEBSOCKET_CLOSE_TIMEOUT, ws_stream.close(None)).await;
    let _ = tokio::time::timeout(WEBSOCKET_CLOSE_TIMEOUT, async {
        while let Some(message) = ws_stream.next().await {
            match message {
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(Message::Text(_))
                | Ok(Message::Ping(_))
                | Ok(Message::Pong(_))
                | Ok(Message::Binary(_))
                | Ok(Message::Frame(_)) => {}
            }
        }
    })
    .await;
}

async fn graceful_close_split_websocket(write: &mut ExecutionWsWrite, read: &mut ExecutionWsRead) {
    let _ = tokio::time::timeout(WEBSOCKET_CLOSE_TIMEOUT, write.close()).await;
    let _ = tokio::time::timeout(WEBSOCKET_CLOSE_TIMEOUT, async {
        while let Some(message) = read.next().await {
            match message {
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(Message::Text(_))
                | Ok(Message::Ping(_))
                | Ok(Message::Pong(_))
                | Ok(Message::Binary(_))
                | Ok(Message::Frame(_)) => {}
            }
        }
    })
    .await;
}

enum ExecutionUpdate {
    Root(RestExecution),
    Descendant(ExecutionListItem),
}

fn notification_to_execution_update(
    root_execution_id: i64,
    known_execution_ids: &HashSet<i64>,
    notification: NotifierNotification,
) -> Option<ExecutionUpdate> {
    if notification.entity_type != "execution" {
        return None;
    }

    if notification.entity_id == root_execution_id {
        return serde_json::from_value(notification.payload)
            .ok()
            .map(ExecutionUpdate::Root);
    }

    let execution: ExecutionListItem = serde_json::from_value(notification.payload).ok()?;
    let parent_id = execution.parent?;
    if known_execution_ids.contains(&execution.id)
        || parent_id == root_execution_id
        || known_execution_ids.contains(&parent_id)
    {
        Some(ExecutionUpdate::Descendant(execution))
    } else {
        None
    }
}

// ── WebSocket path ────────────────────────────────────────────────────────────

async fn wait_via_websocket(
    ws_base_url: &str,
    execution_id: i64,
    deadline: Instant,
    verbose: bool,
    api_client: &mut ApiClient,
) -> Result<ExecutionSummary> {
    // Build the full WS endpoint URL.
    let ws_url = format!("{}/ws", ws_base_url.trim_end_matches('/'));

    let connect_timeout = Duration::from_secs(5);
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        anyhow::bail!("WS budget exhausted before connect");
    }
    let effective_connect_timeout = connect_timeout.min(remaining);

    let request = build_notifier_ws_request(&ws_url, api_client.auth_token())?;
    let connect_result =
        tokio::time::timeout(effective_connect_timeout, connect_async(request)).await;

    let (ws_stream, _response) = match connect_result {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => anyhow::bail!("WebSocket connect failed: {}", e),
        Err(_) => anyhow::bail!("WebSocket connect timed out"),
    };

    if verbose {
        eprintln!("  [notifier] connected to {}", ws_url);
    }

    let (mut write, mut read) = ws_stream.split();

    let wait_result: Result<ExecutionSummary> = async {
        // Wait for the welcome message before subscribing.
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(msg) = read.next().await {
                if let Ok(Message::Text(txt)) = msg {
                    if let Ok(ServerMsg::Welcome { client_id, .. }) =
                        serde_json::from_str::<ServerMsg>(&txt)
                    {
                        if verbose {
                            eprintln!("  [notifier] session id {}", client_id);
                        }
                        return Ok(());
                    }
                }
            }
            anyhow::bail!("connection closed before welcome")
        })
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for welcome message"))??;

        // Subscribe to this specific execution.
        let subscribe_msg = ClientMsg::Subscribe {
            filter: format!("entity:execution:{}", execution_id),
        };
        let subscribe_json = serde_json::to_string(&subscribe_msg)?;
        SinkExt::send(&mut write, Message::Text(subscribe_json.into())).await?;

        if verbose {
            eprintln!(
                "  [notifier] subscribed to entity:execution:{}",
                execution_id
            );
        }

        // ── Race-condition guard ──────────────────────────────────────────────
        // The execution may have already completed in the window between the
        // initial POST and when the WS subscription became active.  Check once
        // with the REST API *after* subscribing so there is no gap: either the
        // notification arrives after this check (and we'll catch it in the loop
        // below) or we catch the terminal state here.
        {
            let path = format!("/executions/{}", execution_id);
            if let Ok(Ok(exec)) = tokio::time::timeout(
                deadline.saturating_duration_since(Instant::now()),
                api_client.get::<RestExecution>(&path),
            )
            .await
            {
                if is_terminal(&exec.status) {
                    if verbose {
                        eprintln!(
                            "  [notifier] execution {} already terminal ('{}') — caught by post-subscribe check",
                            execution_id, exec.status
                        );
                    }
                    return Ok(exec.into());
                }
            }
        }

        // Reconcile through REST while the socket is quiet. A successfully
        // written subscribe frame is not an acknowledgement that the notifier
        // has installed its filter, so a fast execution can otherwise finish in
        // that window and leave a healthy WebSocket silent until timeout.
        let mut next_reconcile = Instant::now() + WEBSOCKET_RECONCILE_INTERVAL;

        // Periodically ping to keep the connection alive and check the deadline.
        let ping_interval = Duration::from_secs(15);
        let mut next_ping = Instant::now() + ping_interval;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("timed out waiting for execution {}", execution_id);
            }

            // Wait up to the earliest of: REST reconciliation, next ping, or deadline.
            let wait_for = remaining
                .min(next_ping.saturating_duration_since(Instant::now()))
                .min(next_reconcile.saturating_duration_since(Instant::now()));

            let msg_result = tokio::time::timeout(wait_for, read.next()).await;

            match msg_result {
                // Received a message within the window.
                Ok(Some(Ok(Message::Text(txt)))) => {
                    match serde_json::from_str::<ServerMsg>(&txt) {
                        Ok(ServerMsg::Notification(n)) => {
                            if n.entity_type == "execution" && n.entity_id == execution_id {
                                if verbose {
                                    eprintln!(
                                        "  [notifier] {} for execution {} — status={:?}",
                                        n.notification_type,
                                        execution_id,
                                        n.payload.get("status").and_then(|s| s.as_str()),
                                    );
                                }

                                // Extract status from the notification payload.
                                // The notifier broadcasts a small subset of the
                                // execution row (no `result` / large fields) so
                                // we fetch the full record via REST once the
                                // status is terminal.
                                if let Some(status) =
                                    n.payload.get("status").and_then(|s| s.as_str())
                                {
                                    if is_terminal(status) {
                                        return fetch_summary_or_fallback(
                                            api_client,
                                            execution_id,
                                            &n.payload,
                                            verbose,
                                        )
                                        .await;
                                    }
                                }
                            }
                            // Not our execution or not yet terminal — keep waiting.
                        }
                        Ok(ServerMsg::Error { message }) => {
                            anyhow::bail!("notifier error: {}", message);
                        }
                        Ok(ServerMsg::Welcome { .. } | ServerMsg::Unknown) => {
                            // Ignore unexpected / unrecognised messages.
                        }
                        Err(e) => {
                            // Log parse failures at trace level — they can happen if the
                            // server sends a message format we don't recognise yet.
                            if verbose {
                                eprintln!("  [notifier] ignoring unrecognised message: {}", e);
                            }
                        }
                    }
                }
                // Connection closed cleanly.
                Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                    anyhow::bail!("notifier WebSocket closed unexpectedly");
                }
                // Ping/pong frames — ignore.
                Ok(Some(Ok(
                    Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_),
                ))) => {}
                // WebSocket transport error.
                Ok(Some(Err(e))) => {
                    anyhow::bail!("WebSocket error: {}", e);
                }
                // Timeout waiting for a message — time to ping.
                Err(_timeout) => {
                    let now = Instant::now();
                    if now >= next_reconcile {
                        let path = format!("/executions/{}", execution_id);
                        let reconcile_timeout = deadline.saturating_duration_since(now);
                        if let Ok(Ok(exec)) = tokio::time::timeout(
                            reconcile_timeout,
                            api_client.get::<RestExecution>(&path),
                        )
                        .await
                        {
                            if is_terminal(&exec.status) {
                                return Ok(exec.into());
                            }
                        }
                        next_reconcile = now + WEBSOCKET_RECONCILE_INTERVAL;
                    }
                    if now >= next_ping {
                        let _ = SinkExt::send(
                            &mut write,
                            Message::Text(serde_json::to_string(&ClientMsg::Ping)?.into()),
                        )
                        .await;
                        next_ping = now + ping_interval;
                    }
                }
            }
        }
    }
    .await;

    graceful_close_split_websocket(&mut write, &mut read).await;
    wait_result
}

/// Fetch the full execution via REST once the notifier signals terminal status.
/// Falls back to a partial summary built from the payload if the REST call fails
/// (e.g. network error during shutdown).
async fn fetch_summary_or_fallback(
    client: &mut ApiClient,
    execution_id: i64,
    payload: &serde_json::Value,
    verbose: bool,
) -> Result<ExecutionSummary> {
    let path = format!("/executions/{}", execution_id);
    match client.get::<RestExecution>(&path).await {
        Ok(exec) => Ok(exec.into()),
        Err(e) => {
            if verbose {
                eprintln!(
                    "  [notifier] terminal received but REST refetch failed ({}); using partial payload",
                    e
                );
            }
            build_summary_from_payload(execution_id, payload)
        }
    }
}

/// Build an [`ExecutionSummary`] from the notification payload.
/// The notifier payload matches the REST execution shape closely enough that
/// we can deserialize it directly.
fn build_summary_from_payload(
    execution_id: i64,
    payload: &serde_json::Value,
) -> Result<ExecutionSummary> {
    // Try a full deserialize first.
    if let Ok(exec) = serde_json::from_value::<RestExecution>(payload.clone()) {
        return Ok(exec.into());
    }

    // Partial payload — assemble what we can.
    Ok(ExecutionSummary {
        id: execution_id,
        status: payload
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string(),
        action_ref: payload
            .get("action_ref")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        result: payload.get("result").cloned(),
        created: payload
            .get("created")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        updated: payload
            .get("updated")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

// ── polling fallback ──────────────────────────────────────────────────────────

const POLL_INTERVAL: Duration = Duration::from_millis(500);
const POLL_INTERVAL_MAX: Duration = Duration::from_secs(2);
/// How quickly the poll interval grows on each successive check.
const POLL_BACKOFF_FACTOR: f64 = 1.5;

async fn wait_via_polling(
    client: &mut ApiClient,
    execution_id: i64,
    deadline: Instant,
    verbose: bool,
) -> Result<ExecutionSummary> {
    if verbose {
        eprintln!("  [poll] watching execution {}", execution_id);
    }

    let mut interval = POLL_INTERVAL;

    loop {
        // Poll immediately first, before sleeping — catches the case where the
        // execution already finished while we were connecting to the notifier.
        let path = format!("/executions/{}", execution_id);
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            anyhow::bail!("timed out waiting for execution {}", execution_id);
        }
        match tokio::time::timeout(remaining, client.get::<RestExecution>(&path)).await {
            Ok(Ok(exec)) => {
                if is_terminal(&exec.status) {
                    if verbose {
                        eprintln!("  [poll] execution {} is {}", execution_id, exec.status);
                    }
                    return Ok(exec.into());
                }
                if verbose {
                    eprintln!(
                        "  [poll] status = {} — checking again in {:.1}s",
                        exec.status,
                        interval.as_secs_f64()
                    );
                }
            }
            Ok(Err(e)) => {
                if verbose {
                    eprintln!("  [poll] request failed ({}), retrying…", e);
                }
            }
            Err(_) => anyhow::bail!("timed out waiting for execution {}", execution_id),
        }

        // Check deadline *after* the poll attempt so we always do at least one check.
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for execution {}", execution_id);
        }

        // Sleep, but wake up if we'd overshoot the deadline.
        let sleep_for = interval.min(deadline.saturating_duration_since(Instant::now()));
        tokio::time::sleep(sleep_for).await;

        // Exponential back-off up to the cap.
        interval = Duration::from_secs_f64(
            (interval.as_secs_f64() * POLL_BACKOFF_FACTOR).min(POLL_INTERVAL_MAX.as_secs_f64()),
        );
    }
}

// ── URL resolution ────────────────────────────────────────────────────────────

/// Derive the notifier WebSocket base URL.
///
/// Priority:
/// 1. Explicit `notifier_ws_url` in [`WaitOptions`].
/// 2. Replace the API base URL scheme (`http` → `ws`) and port (`8080` → `8081`).
///    This covers the standard single-host layout where both services share the
///    same hostname.
fn resolve_ws_url(opts: &WaitOptions<'_>) -> Option<String> {
    if let Some(url) = &opts.notifier_ws_url {
        return Some(url.clone());
    }

    // Ask the client for its base URL by building a dummy request path
    // and stripping the path portion — we don't have direct access to
    // base_url here so we derive it from the config instead.
    let api_url = opts.api_client.base_url();

    // Transform http(s)://host:PORT/... → ws(s)://host:8081
    let ws_url = derive_notifier_url(api_url)?;
    Some(ws_url)
}

/// Convert an HTTP API base URL into the expected notifier WebSocket URL.
///
/// - `http://localhost:8080`  → `ws://localhost:8081`
/// - `https://api.example.com` → `wss://api.example.com:8081`
/// - `http://api.example.com:9000` → `ws://api.example.com:8081` // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Explicit http API URLs intentionally derive ws notifier URLs for local/plain-HTTP deployments.
fn derive_notifier_url(api_url: &str) -> Option<String> {
    // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- The function upgrades https->wss and only returns ws for explicit http base URLs or test examples.
    let url = url::Url::parse(api_url).ok()?;
    let ws_scheme = match url.scheme() {
        "https" => "wss",
        _ => "ws",
    };
    let host = url.host_str()?;
    Some(format!("{}://{}:8081", ws_scheme, host))
}

pub fn extract_stdout(result: &Option<serde_json::Value>) -> Option<String> {
    result
        .as_ref()
        .and_then(|value| value.get("stdout"))
        .and_then(|stdout| stdout.as_str())
        .filter(|stdout| !stdout.is_empty())
        .map(ToOwned::to_owned)
}

fn format_task_label(
    workflow_task: &Option<WorkflowTaskMetadata>,
    action_ref: &str,
    execution_id: i64,
) -> String {
    if let Some(workflow_task) = workflow_task {
        if let Some(index) = workflow_task.task_index {
            format!("{}[{}]", workflow_task.task_name, index)
        } else {
            workflow_task.task_name.clone()
        }
    } else {
        format!("{}#{}", action_ref, execution_id)
    }
}

async fn stream_execution_log(task: StreamLogTask) {
    let StreamLogTask {
        stream_name,
        offset,
        config:
            StreamWatchConfig {
                base_url,
                token,
                execution_id,
                prefix,
                debug,
                emit_output,
                task_id,
                live_renderer,
                delivered_output,
                root_stdout_completed,
            },
    } = task;

    let mut stream_url = match url::Url::parse(&format!(
        "{}/api/v1/executions/{}/logs/{}/stream",
        base_url.trim_end_matches('/'),
        execution_id,
        stream_name
    )) {
        Ok(url) => url,
        Err(err) => {
            if debug {
                eprintln!("  [watch] failed to build stream URL: {}", err);
            }
            return;
        }
    };
    let current_offset = offset.load(Ordering::Relaxed).to_string();
    stream_url
        .query_pairs_mut()
        .append_pair("offset", &current_offset);

    let response = match reqwest::Client::new()
        .get(stream_url)
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            if debug {
                eprintln!("  [watch] failed to open stream source: {}", err);
            }
            return;
        }
    };
    if !response.status().is_success() {
        if debug {
            eprintln!(
                "  [watch] failed to open stream source: HTTP {}",
                response.status()
            );
        }
        return;
    }
    let mut event_source = response.bytes_stream().eventsource();
    let mut carry = String::new();

    while let Some(event) = event_source.next().await {
        match event {
            Ok(message) => match message.event.as_str() {
                "content" | "append" => {
                    if let Ok(server_offset) = message.id.parse::<u64>() {
                        offset.store(server_offset, Ordering::Relaxed);
                    }
                    if !message.data.is_empty() {
                        delivered_output.store(true, Ordering::Relaxed);
                    }
                    let lines = consume_stream_chunk(&message.data, &mut carry);
                    emit_stream_lines(
                        prefix.as_deref(),
                        stream_name,
                        task_id,
                        &live_renderer,
                        emit_output,
                        lines,
                    );
                }
                "done" => {
                    if let Some(flag) = &root_stdout_completed {
                        flag.store(true, Ordering::Relaxed);
                    }
                    if let Some(line) = take_remaining_stream_chunk(&mut carry) {
                        emit_stream_lines(
                            prefix.as_deref(),
                            stream_name,
                            task_id,
                            &live_renderer,
                            emit_output,
                            vec![line],
                        );
                    }
                    break;
                }
                "error" => {
                    if debug && !message.data.is_empty() {
                        eprintln!("  [watch] {}", message.data);
                    }
                    break;
                }
                _ => {}
            },
            Err(err) => {
                if let Some(line) = take_remaining_stream_chunk(&mut carry) {
                    emit_stream_lines(
                        prefix.as_deref(),
                        stream_name,
                        task_id,
                        &live_renderer,
                        emit_output,
                        vec![line],
                    );
                }
                if debug {
                    eprintln!(
                        "  [watch] stream error for execution {}: {}",
                        execution_id, err
                    );
                }
                break;
            }
        }
    }

    if let Some(line) = take_remaining_stream_chunk(&mut carry) {
        emit_stream_lines(
            prefix.as_deref(),
            stream_name,
            task_id,
            &live_renderer,
            emit_output,
            vec![line],
        );
    }
}

fn consume_stream_chunk(chunk: &str, carry: &mut String) -> Vec<String> {
    carry.push_str(chunk);
    let mut lines = Vec::new();

    while let Some(idx) = carry.find('\n') {
        let mut line = carry.drain(..=idx).collect::<String>();
        if line.ends_with('\n') {
            line.pop();
        }
        if line.ends_with('\r') {
            line.pop();
        }
        lines.push(line);
    }

    lines
}

fn take_remaining_stream_chunk(carry: &mut String) -> Option<String> {
    if carry.is_empty() {
        return None;
    }
    let line = carry.clone();
    carry.clear();
    Some(line)
}

fn emit_stream_lines(
    prefix: Option<&str>,
    _stream_name: &str,
    task_id: Option<i64>,
    live_renderer: &Option<LiveRenderer>,
    emit_output: bool,
    lines: Vec<String>,
) {
    if lines.is_empty() {
        return;
    }

    if let (Some(renderer), Some(task_id)) = (live_renderer.as_ref(), task_id) {
        for line in lines {
            renderer.push_line(task_id, _stream_name, line);
        }
        return;
    }

    if emit_output {
        for line in lines {
            if let Some(prefix) = prefix {
                eprintln!("[{}] {}", prefix, line);
            } else {
                eprintln!("{}", line);
            }
        }
    }
}

fn should_clear_task_tail(status: &str) -> bool {
    matches!(status.to_lowercase().as_str(), "completed" | "succeeded")
}

fn build_iterated_summaries(tasks: &BTreeMap<i64, LiveTaskState>) -> Vec<IteratedTaskSummary> {
    let mut summaries: BTreeMap<String, IteratedTaskSummary> = BTreeMap::new();

    for (id, task) in tasks.iter().filter(|(_, task)| task.is_iterated) {
        let summary =
            summaries
                .entry(task.task_name.clone())
                .or_insert_with(|| IteratedTaskSummary {
                    task_name: task.task_name.clone(),
                    first_seen_id: *id,
                    ..Default::default()
                });

        if *id < summary.first_seen_id {
            summary.first_seen_id = *id;
        }
        match (&summary.first_started_at, &task.started_at) {
            (None, Some(started_at)) => summary.first_started_at = Some(*started_at),
            (Some(current), Some(started_at)) if started_at < current => {
                summary.first_started_at = Some(*started_at);
            }
            _ => {}
        }

        match normalized_task_state(&task.status) {
            TaskStateBucket::Pending => summary.pending += 1,
            TaskStateBucket::Running => summary.running += 1,
            TaskStateBucket::Completed => summary.completed += 1,
            TaskStateBucket::Failed => summary.failed += 1,
        }
    }

    summaries.into_values().collect()
}

fn render_iterated_summary_line(
    summary: &IteratedTaskSummary,
    now: Instant,
    width: usize,
) -> String {
    let (icon, icon_width) = if summary.failed > 0 {
        ("✗".red().bold().to_string(), 1)
    } else if summary.running == 0 && summary.pending == 0 {
        ("✓".green().bold().to_string(), 1)
    } else {
        (spinner_frame(now).cyan().to_string(), 1)
    };
    let left = format!(
        "{}: {} running, {} pending, {} completed, {} failed",
        summary.task_name, summary.running, summary.pending, summary.completed, summary.failed
    );
    format_row(&icon, icon_width, &left, None, None, width)
}

fn render_item_cmp(left: &RenderItem<'_>, right: &RenderItem<'_>) -> std::cmp::Ordering {
    render_sort_tuple(left)
        .cmp(&render_sort_tuple(right))
        .then_with(|| left.started_at.cmp(&right.started_at))
        .then_with(|| left.id.cmp(&right.id))
}

fn render_sort_tuple(item: &RenderItem<'_>) -> (bool, i64, i64, u8) {
    (
        item.group_started_at.is_none(),
        item.group_started_at.unwrap_or(i64::MAX),
        item.group_id,
        item.within_group_rank,
    )
}

fn should_render_iterated_task(task: &LiveTaskState) -> bool {
    normalized_task_state(&task.status) == TaskStateBucket::Running && task.started_at.is_some()
}

fn render_task_lines(task: &LiveTaskState, now: Instant, width: usize) -> Vec<String> {
    let elapsed = task
        .started_at
        .map(|started_at| {
            let ended_at = task.finished_at.unwrap_or_else(Utc::now);
            format_elapsed(
                ended_at
                    .signed_duration_since(started_at)
                    .to_std()
                    .unwrap_or_default(),
            )
        })
        .unwrap_or_else(|| "--:--.-".to_string());
    let status = task.status.to_lowercase();
    let (icon, icon_width) = match status.as_str() {
        "completed" | "succeeded" => ("✓".green().bold().to_string(), 1),
        "failed" => ("✗".red().bold().to_string(), 1),
        "cancelled" | "canceled" => ("○".bright_black().to_string(), 1),
        "timeout" | "timed_out" => ("◷".yellow().bold().to_string(), 1),
        _ => (spinner_frame(now).cyan().to_string(), 1),
    };

    let left = format!("{} {}", task.label, task.status);
    let mut lines = vec![format_row(
        &icon,
        icon_width,
        &left,
        Some(&format!("[{}]", task.action_ref)),
        Some(&elapsed),
        width,
    )];
    for line in task.stderr_lines.iter().chain(task.stdout_lines.iter()) {
        lines.push(format!(
            "    {}",
            truncate_to_width(line, width.saturating_sub(4))
        ));
    }
    lines
}

fn spinner_frame(_now: Instant) -> &'static str {
    let frame = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        / RENDER_TICK.as_millis();
    let frame = frame as usize % SPINNER_FRAMES.len();
    SPINNER_FRAMES[frame]
}

fn format_elapsed(duration: Duration) -> String {
    let total_tenths = (duration.as_secs_f64() * 10.0).round() as u64;
    let total = total_tenths / 10;
    let tenths = total_tenths % 10;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}.{tenths}")
    } else {
        format!("{minutes:02}:{seconds:02}.{tenths}")
    }
}

fn truncate_log_line(line: &str) -> String {
    truncate_to_width(line, 120)
}

fn format_row(
    icon: &str,
    icon_width: usize,
    left: &str,
    right_prefix: Option<&str>,
    right: Option<&str>,
    width: usize,
) -> String {
    let min_gap = 2;
    let right_prefix_width = right_prefix.map_or(0, display_width);
    let right_width = right.map_or(0, display_width);
    let right_total_width =
        right_prefix_width + usize::from(right_prefix.is_some() && right.is_some()) + right_width;
    let reserved = icon_width + 1 + min_gap + right_total_width;
    let available_left = width.saturating_sub(reserved).max(10);
    let left = truncate_to_width(left, available_left);
    let left_width = display_width(&left);

    if right_prefix.is_some() || right.is_some() {
        let gap = width
            .saturating_sub(icon_width + 1 + left_width + right_total_width)
            .max(min_gap);
        let mut row = format!("{icon} {left}{}", " ".repeat(gap));
        if let Some(right_prefix) = right_prefix {
            row.push_str(right_prefix);
        }
        if let Some(right) = right {
            if right_prefix.is_some() {
                row.push(' ');
            }
            row.push_str(&right.bright_black().to_string());
        }
        row
    } else {
        format!("{icon} {left}")
    }
}

fn current_terminal_width() -> usize {
    terminal_size()
        .map(|(Width(width), _)| width as usize)
        .filter(|width| *width > 20)
        .unwrap_or(100)
}

fn display_width(value: &str) -> usize {
    value.chars().count()
}

fn truncate_to_width(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.to_string();
    }

    value
        .chars()
        .take(width.saturating_sub(1))
        .collect::<String>()
        + "…"
}

fn should_stream_logs(execution: &ExecutionListItem) -> bool {
    execution.started_at.is_some()
}

fn should_stream_root_logs(execution: &RestExecution) -> bool {
    execution.started_at.is_some()
}

fn parse_api_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskStateBucket {
    Pending,
    Running,
    Completed,
    Failed,
}

fn normalized_task_state(status: &str) -> TaskStateBucket {
    match status {
        "requested" | "scheduling" | "scheduled" => TaskStateBucket::Pending,
        "completed" | "succeeded" => TaskStateBucket::Completed,
        "failed" | "timeout" | "timed_out" | "cancelled" | "canceled" | "abandoned" => {
            TaskStateBucket::Failed
        }
        _ => TaskStateBucket::Running,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn output_watch_drain_grace_cancels_a_stale_notifier_update() {
        let task = OutputWatchTask {
            handle: tokio::spawn(async { std::future::pending::<()>().await }),
            delivered_output: Arc::new(AtomicBool::new(false)),
            root_stdout_completed: Arc::new(AtomicBool::new(false)),
        };

        let result = tokio::time::timeout(
            Duration::from_millis(100),
            task.finish_or_stop(Duration::from_millis(1)),
        )
        .await;
        assert_eq!(result.unwrap(), (false, false));
    }

    #[test]
    fn test_is_terminal() {
        assert!(is_terminal("completed"));
        assert!(is_terminal("succeeded"));
        assert!(is_terminal("failed"));
        assert!(is_terminal("canceled"));
        assert!(is_terminal("cancelled"));
        assert!(is_terminal("timeout"));
        assert!(is_terminal("timed_out"));
        assert!(!is_terminal("requested"));
        assert!(!is_terminal("scheduled"));
        assert!(!is_terminal("running"));
    }

    #[test]
    fn test_derive_notifier_url() {
        assert_eq!(
            derive_notifier_url("http://localhost:8080"),
            Some("ws://localhost:8081".to_string())
        );
        assert_eq!(
            derive_notifier_url("https://api.example.com"),
            Some("wss://api.example.com:8081".to_string())
        );
        assert_eq!(
            derive_notifier_url("http://api.example.com:9000"),
            Some("ws://api.example.com:8081".to_string()) // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Unit test for explicit plain-HTTP API URL handling.
        );
        assert_eq!(
            derive_notifier_url("http://10.0.0.5:8080"),
            Some("ws://10.0.0.5:8081".to_string()) // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- Unit test for explicit plain-HTTP private-network API URL handling.
        );
    }

    #[test]
    fn test_build_summary_from_full_payload() {
        let payload = serde_json::json!({
            "id": 42,
            "action_ref": "core.echo",
            "status": "completed",
            "result": { "stdout": "hi" },
            "created": "2026-01-01T00:00:00Z",
            "updated": "2026-01-01T00:00:01Z"
        });
        let summary = build_summary_from_payload(42, &payload).unwrap();
        assert_eq!(summary.id, 42);
        assert_eq!(summary.status, "completed");
        assert_eq!(summary.action_ref, "core.echo");
    }

    #[test]
    fn test_build_summary_from_partial_payload() {
        let payload = serde_json::json!({ "status": "failed" });
        let summary = build_summary_from_payload(7, &payload).unwrap();
        assert_eq!(summary.id, 7);
        assert_eq!(summary.status, "failed");
        assert_eq!(summary.action_ref, "");
    }

    #[test]
    fn test_extract_stdout() {
        let result = Some(serde_json::json!({
            "stdout": "hello world",
            "stderr_log": "/tmp/stderr.log"
        }));
        assert_eq!(extract_stdout(&result).as_deref(), Some("hello world"));
    }

    #[test]
    fn test_should_clear_task_tail() {
        assert!(should_clear_task_tail("completed"));
        assert!(should_clear_task_tail("succeeded"));
        assert!(!should_clear_task_tail("failed"));
        assert!(!should_clear_task_tail("running"));
    }

    #[test]
    fn test_push_line_ignores_late_output_for_successful_task() {
        let renderer = LiveRenderer {
            enabled: true,
            state: Arc::new(Mutex::new(LiveRendererState::default())),
            stop: Arc::new(AtomicBool::new(false)),
        };

        renderer.upsert_task(LiveTaskUpdate {
            id: 7,
            label: "task".to_string(),
            task_name: "task".to_string(),
            is_root: false,
            is_iterated: false,
            action_ref: "core.echo".to_string(),
            status: "completed".to_string(),
            started_at: None,
            finished_at: Some(Utc::now()),
        });
        renderer.push_line(7, "stderr", "should not persist".to_string());

        let state = renderer.state.lock().expect("live renderer poisoned");
        let task = state.tasks.get(&7).expect("task exists");
        assert!(task.stderr_lines.is_empty());
        assert!(task.stdout_lines.is_empty());
    }

    #[test]
    fn test_render_task_lines_places_stdout_after_stderr() {
        let task = LiveTaskState {
            label: "task".to_string(),
            task_name: "task".to_string(),
            is_root: false,
            is_iterated: false,
            action_ref: "core.echo".to_string(),
            status: "running".to_string(),
            started_at: None,
            finished_at: None,
            stderr_lines: VecDeque::from(vec!["stderr line".to_string()]),
            stdout_lines: VecDeque::from(vec!["stdout line".to_string()]),
        };

        let lines = render_task_lines(&task, Instant::now(), 100);

        assert!(lines[1].contains("stderr line"));
        assert!(lines[2].contains("stdout line"));
    }

    #[test]
    fn test_format_task_label() {
        let workflow_task = Some(WorkflowTaskMetadata {
            task_name: "build".to_string(),
            task_index: Some(2),
        });
        assert_eq!(
            format_task_label(&workflow_task, "core.echo", 42),
            "build[2]"
        );
        assert_eq!(format_task_label(&None, "core.echo", 42), "core.echo#42");
    }

    #[test]
    fn test_consume_stream_chunk_splits_lines() {
        let mut carry = String::new();
        let lines = consume_stream_chunk("one\ntwo", &mut carry);
        assert_eq!(lines, vec!["one".to_string()]);
        assert_eq!(carry, "two");

        let lines = consume_stream_chunk("\nthree\n", &mut carry);
        assert_eq!(lines, vec!["two".to_string(), "three".to_string()]);
        assert!(carry.is_empty());
    }

    #[test]
    fn test_format_elapsed() {
        assert_eq!(format_elapsed(Duration::from_millis(5300)), "00:05.3");
        assert_eq!(format_elapsed(Duration::from_millis(65100)), "01:05.1");
        assert_eq!(format_elapsed(Duration::from_millis(3665100)), "01:01:05.1");
    }

    #[test]
    fn test_truncate_to_width() {
        assert_eq!(truncate_to_width("abcdef", 4), "abc…");
        assert_eq!(truncate_to_width("abc", 4), "abc");
    }

    #[test]
    fn test_notification_to_execution_update_accepts_known_descendant() {
        let notification = NotifierNotification {
            notification_type: "execution_status_changed".to_string(),
            entity_type: "execution".to_string(),
            entity_id: 12,
            payload: serde_json::json!({
                "id": 12,
                "action_ref": "core.echo",
                "status": "running",
                "parent": 10,
                "started_at": "2026-01-01T00:00:00Z",
                "updated": "2026-01-01T00:00:01Z",
                "workflow_task": {"task_name": "child"}
            }),
        };
        let known_ids = HashSet::from([1_i64, 10_i64]);

        let update = notification_to_execution_update(1, &known_ids, notification)
            .expect("notification should be accepted");

        match update {
            ExecutionUpdate::Descendant(update) => {
                assert_eq!(update.id, 12);
                assert_eq!(update.parent, Some(10));
            }
            ExecutionUpdate::Root(_) => panic!("expected descendant update"),
        }
    }

    #[test]
    fn test_notification_to_execution_update_accepts_root_execution() {
        let notification = NotifierNotification {
            notification_type: "execution_status_changed".to_string(),
            entity_type: "execution".to_string(),
            entity_id: 1,
            payload: serde_json::json!({
                "id": 1,
                "action_ref": "core.workflow",
                "status": "running",
                "created": "2026-01-01T00:00:00Z",
                "started_at": "2026-01-01T00:00:00Z",
                "updated": "2026-01-01T00:00:01Z"
            }),
        };
        let known_ids = HashSet::from([1_i64, 10_i64]);

        let update = notification_to_execution_update(1, &known_ids, notification)
            .expect("root notification should be accepted");

        match update {
            ExecutionUpdate::Root(update) => {
                assert_eq!(update.id, 1);
                assert_eq!(update.status, "running");
                assert!(update.started_at.is_some());
            }
            ExecutionUpdate::Descendant(_) => panic!("expected root update"),
        }
    }

    #[test]
    fn test_notification_to_execution_update_rejects_unrelated_execution() {
        let notification = NotifierNotification {
            notification_type: "execution_created".to_string(),
            entity_type: "execution".to_string(),
            entity_id: 99,
            payload: serde_json::json!({
                "id": 99,
                "action_ref": "core.echo",
                "status": "requested",
                "parent": 77,
                "updated": "2026-01-01T00:00:01Z"
            }),
        };
        let known_ids = HashSet::from([1_i64, 10_i64]);

        assert!(notification_to_execution_update(1, &known_ids, notification).is_none());
    }

    #[tokio::test]
    async fn test_ensure_streams_running_spawns_for_empty_handles() {
        let mut handles = Vec::new();
        let config = StreamWatchConfig {
            base_url: "not a url".to_string(),
            token: "token".to_string(),
            execution_id: 42,
            prefix: Some("process_items[1]".to_string()),
            debug: false,
            emit_output: false,
            task_id: Some(42),
            live_renderer: None,
            delivered_output: Arc::new(AtomicBool::new(false)),
            root_stdout_completed: None,
        };

        ensure_streams_running(&mut handles, &config);

        assert_eq!(handles.len(), 2);

        wait_for_stream_handles(handles).await;
    }

    #[test]
    fn test_root_task_is_hidden_when_child_tasks_exist() {
        let renderer = LiveRenderer {
            enabled: true,
            state: Arc::new(Mutex::new(LiveRendererState::default())),
            stop: Arc::new(AtomicBool::new(false)),
        };

        renderer.upsert_task(LiveTaskUpdate {
            id: 1,
            label: "execution#1".to_string(),
            task_name: "core.workflow".to_string(),
            is_root: true,
            is_iterated: false,
            action_ref: "core.workflow".to_string(),
            status: "running".to_string(),
            started_at: None,
            finished_at: None,
        });
        renderer.upsert_task(LiveTaskUpdate {
            id: 2,
            label: "task_a".to_string(),
            task_name: "task_a".to_string(),
            is_root: false,
            is_iterated: false,
            action_ref: "core.echo".to_string(),
            status: "running".to_string(),
            started_at: None,
            finished_at: None,
        });

        renderer.render(false);

        let state = renderer.state.lock().expect("live renderer poisoned");
        assert_eq!(state.rendered_lines, 1);
    }
}
