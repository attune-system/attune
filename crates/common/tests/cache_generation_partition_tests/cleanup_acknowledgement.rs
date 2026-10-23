//! One-connection fault injection for cleanup's real PostgreSQL admission wait.
use super::{
    fixture, limits, CacheGenerationCleanupOutcome, CacheGenerationRepository, TestDatabase,
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{sleep_until, timeout, timeout_at, Instant},
};

const MAX_FRAME: usize = 65_536;
const MAX_NAMES: usize = 16;
const SETTER: &[u8] =
    b"SELECT set_config('lock_timeout', $1, true), set_config('statement_timeout', $2, true)";
type CleanupTask = JoinHandle<Result<CacheGenerationCleanupOutcome>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum GateMode {
    Admission,
    Begin,
    Commit,
    UnexpectedBegin,
}
impl GateMode {
    fn selects(self, sql: &[u8]) -> bool {
        match self {
            Self::Admission => sql == SETTER,
            Self::Begin | Self::UnexpectedBegin => sql == b"BEGIN",
            Self::Commit => sql == b"COMMIT",
        }
    }
    fn completion(self) -> &'static [u8] {
        match self {
            Self::Admission => b"SELECT 1\0",
            Self::Begin | Self::UnexpectedBegin => b"BEGIN\0",
            Self::Commit => b"COMMIT\0",
        }
    }
}

// Store only names and whether SQL matches the closed mode, never SQL or keys.
fn cstring<'a>(body: &mut &'a [u8]) -> Result<&'a [u8]> {
    let end = body
        .iter()
        .position(|byte| *byte == 0)
        .context("missing wire string terminator")?;
    let value = &body[..end];
    *body = &body[end + 1..];
    Ok(value)
}
fn name(body: &mut &[u8]) -> Result<Vec<u8>> {
    let value = cstring(body)?;
    ensure!(value.len() <= 128, "wire name exceeds fixed bound");
    Ok(value.to_vec())
}
fn register(map: &mut BTreeMap<Vec<u8>, bool>, key: Vec<u8>, target: bool) -> Result<()> {
    ensure!(
        map.contains_key(&key) || map.len() < MAX_NAMES,
        "wire name registry is full"
    );
    map.insert(key, target);
    Ok(())
}
async fn frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    buffer: &mut [u8],
) -> Result<Option<([u8; 5], usize)>> {
    let mut header = [0; 5];
    if reader.read(&mut header[..1]).await? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut header[1..]).await?;
    let length = u32::from_be_bytes(header[1..].try_into()?) as usize;
    ensure!(
        (4..=MAX_FRAME).contains(&length),
        "unsupported wire frame length"
    );
    reader.read_exact(&mut buffer[..length - 4]).await?;
    Ok(Some((header, length - 4)))
}
async fn send<W: AsyncWrite + Unpin>(writer: &mut W, header: &[u8], body: &[u8]) -> Result<()> {
    writer.write_all(header).await?;
    writer.write_all(body).await?;
    Ok(())
}

async fn frontend<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut reader: R,
    mut writer: W,
    cycles: mpsc::Sender<bool>,
    mode: GateMode,
) -> Result<()> {
    let mut buffer = vec![0; MAX_FRAME];
    let mut header = [0; 4];
    reader.read_exact(&mut header).await?;
    let length = u32::from_be_bytes(header) as usize;
    ensure!(
        (8..=MAX_FRAME).contains(&length),
        "unsupported startup length"
    );
    reader.read_exact(&mut buffer[..length - 4]).await?;
    ensure!(
        buffer[..4] == 196608_u32.to_be_bytes(),
        "test requires plain PostgreSQL startup"
    );
    send(&mut writer, &header, &buffer[..length - 4]).await?;
    let (mut statements, mut portals) = (BTreeMap::new(), BTreeMap::new());
    let mut execution = None;
    while let Some((header, length)) = frame(&mut reader, &mut buffer).await? {
        let mut body = &buffer[..length];
        match header[0] {
            b'P' => {
                let key = name(&mut body)?;
                let target = mode.selects(cstring(&mut body)?);
                ensure!(body.len() >= 2, "short Parse parameters");
                let count = u16::from_be_bytes(body[..2].try_into()?) as usize;
                ensure!(body.len() == 2 + count * 4, "invalid Parse parameters");
                register(&mut statements, key, target)?;
            }
            b'B' => {
                let portal = name(&mut body)?;
                let statement = name(&mut body)?;
                let target = *statements
                    .get(&statement)
                    .context("unknown Bind statement")?;
                register(&mut portals, portal, target)?;
            }
            b'E' => {
                let portal = name(&mut body)?;
                ensure!(body == [0, 0, 0, 0], "test requires unrestricted Execute");
                ensure!(
                    execution.is_none(),
                    "multiple Execute messages in one cycle"
                );
                execution = Some(*portals.get(&portal).context("unknown Execute portal")?);
            }
            b'S' => {
                ensure!(body.is_empty(), "nonempty Sync");
                cycles
                    .try_send(execution.take().unwrap_or(false))
                    .context("wire cycle queue full or closed")?;
            }
            b'Q' => {
                ensure!(
                    execution.is_none(),
                    "simple query inside extended execution"
                );
                let target = mode.selects(cstring(&mut body)?);
                ensure!(body.is_empty(), "trailing simple-query bytes");
                cycles
                    .try_send(target)
                    .context("wire cycle queue full or closed")?;
            }
            b'C' | b'D' => {
                let kind = *body.first().context("short Close/Describe")?;
                body = &body[1..];
                let key = name(&mut body)?;
                ensure!(
                    body.is_empty() && matches!(kind, b'S' | b'P'),
                    "invalid Close/Describe"
                );
                if header[0] == b'C' {
                    if kind == b'S' {
                        statements.remove(&key);
                    } else {
                        portals.remove(&key);
                    }
                }
            }
            b'H' | b'X' => ensure!(body.is_empty(), "nonempty Flush/Terminate"),
            b'p' => {} // Authentication bytes are forwarded, never retained or logged.
            _ => bail!("unsupported frontend message kind"),
        }
        send(&mut writer, &header, &buffer[..length]).await?;
    }
    // Preserve upstream custody after the client drops its timed-out socket.
    // Propagating FIN here could end the very waiter the immediate assertion
    // must observe. The one owning task releases this socket on explicit stop.
    let result = std::future::pending::<Result<()>>().await;
    drop(writer);
    result
}

async fn backend<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut reader: R,
    mut writer: W,
    mut cycles: mpsc::Receiver<bool>,
    installed: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
    mode: GateMode,
    armed: Arc<AtomicBool>,
) -> Result<()> {
    let mut buffer = vec![0; MAX_FRAME];
    let (mut installed, mut release) = (Some(installed), Some(release));
    let (mut startup, mut cycle) = (true, None);
    while let Some((header, length)) = frame(&mut reader, &mut buffer).await? {
        ensure!(
            b"RSKZ123tTnDCENI".contains(&header[0]),
            "unsupported backend message kind"
        );
        if !startup && matches!(header[0], b'C' | b'Z') {
            if cycle.is_none() {
                cycle = Some(cycles.recv().await.context("missing response cycle")?);
            }
            if header[0] == b'C'
                && cycle == Some(true)
                && installed.is_some()
                && armed.load(Ordering::Acquire)
            {
                // Correlate an exact executed statement or standalone Query.
                // Preparation Sync and RELEASE SAVEPOINT cannot select it.
                ensure!(
                    &buffer[..length] == mode.completion(),
                    "selected command completed with the wrong tag"
                );
                let _ = installed.take().context("missing install signal")?.send(());
                timeout(
                    Duration::from_secs(2),
                    release.take().context("missing release receiver")?,
                )
                .await
                .context("wire gate release deadline")?
                .context("wire gate controller dropped")?;
                if mode == GateMode::UnexpectedBegin {
                    // Deliberately synthetic transport fault, not a claim that
                    // the real accepted BEGIN raised this database error.
                    let error = b"SERROR\0VERROR\0C25P02\0Mowned synthetic recovery error\0\0";
                    let mut fault_header = [b'E', 0, 0, 0, 0];
                    fault_header[1..].copy_from_slice(&((error.len() + 4) as u32).to_be_bytes());
                    send(&mut writer, &fault_header, error).await?;
                    continue;
                }
            }
            if header[0] == b'Z' {
                cycle = None;
            }
        }
        if startup && header[0] == b'Z' {
            startup = false;
        }
        send(&mut writer, &header, &buffer[..length]).await?;
    }
    writer.shutdown().await?;
    Ok(())
}

struct WireGate {
    address: SocketAddr,
    armed: Arc<AtomicBool>,
    installed: oneshot::Receiver<()>,
    release: Option<oneshot::Sender<()>>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<()>>>,
}
impl WireGate {
    async fn start(options: Arc<PgConnectOptions>) -> Result<Self> {
        Self::start_mode(options, GateMode::Admission).await
    }
    async fn start_mode(options: Arc<PgConnectOptions>, mode: GateMode) -> Result<Self> {
        ensure!(
            !options.get_host().starts_with('/'),
            "wire gate requires owned TCP database"
        );
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let (installed, observed) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let (stop, stopped) = oneshot::channel();
        // The original admission gate's activation is unchanged. New modes
        // explicitly arm only after the same one connection is prewarmed.
        let armed = Arc::new(AtomicBool::new(mode == GateMode::Admission));
        let backend_armed = armed.clone();
        let task = tokio::spawn(async move {
            let work = async {
                let (client, _) = listener.accept().await?;
                drop(listener); // One accepted cleanup connection, no accept loop.
                let server = TcpStream::connect((options.get_host(), options.get_port())).await?;
                client.set_nodelay(true)?;
                server.set_nodelay(true)?;
                let (client_read, client_write) = client.into_split();
                let (server_read, server_write) = server.into_split();
                let (send_cycle, receive_cycle) = mpsc::channel(16);
                tokio::try_join!(
                    frontend(client_read, server_write, send_cycle, mode),
                    backend(
                        server_read,
                        client_write,
                        receive_cycle,
                        installed,
                        released,
                        mode,
                        backend_armed,
                    )
                )?;
                Ok::<(), anyhow::Error>(())
            };
            tokio::select! {
                result = work => match result {
                    // Old production deliberately drops its socket before the
                    // upstream waiter ends. Forwarding its final response into
                    // that closed client can reset the transport. Observation
                    // must still succeed before this can become a passing case.
                    Err(error) if error.downcast_ref::<std::io::Error>().is_some_and(|error|
                        matches!(error.kind(), std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset)) => Ok(()),
                    result => result,
                },
                _ = stopped => Ok(()),
            }
        });
        Ok(Self {
            address,
            armed,
            installed: observed,
            release: Some(release),
            stop: Some(stop),
            task: Some(task),
        })
    }
    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
    fn arm(&self) {
        self.armed.store(true, Ordering::Release);
    }
    async fn shutdown(mut self) -> Result<()> {
        self.release();
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let mut task = self.task.take().context("missing owned wire task")?;
        match timeout(Duration::from_secs(3), &mut task).await {
            Ok(result) => result.context("wire task panicked")?,
            Err(_) => {
                task.abort();
                let _ = task.await;
                bail!("wire task required abort and join");
            }
        }
    }
}

struct Evidence {
    outcome: CacheGenerationCleanupOutcome,
    elapsed: Duration,
    pid: i32,
    parent_locks: i64,
    waiters: i64,
    own_waiters: i64,
    activity: serde_json::Value,
    before: Option<(i64, i64)>,
    after: Option<(i64, i64)>,
}
async fn usage(db: &TestDatabase, generation: i64) -> Result<Option<(i64, i64)>> {
    sqlx::query_as(
        "SELECT record_count, physical_bytes FROM cache_generation_entry_usage WHERE generation=$1",
    )
    .bind(generation)
    .fetch_optional(db.pool())
    .await
    .context("read owned generation usage")
}

async fn proxy_pool(db: &TestDatabase, proxy: &WireGate) -> Result<sqlx::PgPool> {
    let options = db
        .pool()
        .connect_options()
        .as_ref()
        .clone()
        .host(&proxy.address.ip().to_string())
        .port(proxy.address.port())
        .ssl_mode(PgSslMode::Disable);
    let schema = db.schema().to_string();
    PgPoolOptions::new()
        .min_connections(0)
        .max_connections(1)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path', $1, false)")
                    .bind(schema)
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .context("connect owned proxy pool")
}
async fn exercise(db: &TestDatabase, generation: i64) -> Result<Evidence> {
    let (mut gate, mut pool, mut holder, mut task): (_, _, _, Option<CleanupTask>) =
        (None, None, None, None);
    let mut cleanup_pid = None;
    let result: Result<Evidence> = async {
        let before = usage(db, generation).await?;
        holder = Some(db.pool().begin().await.context("begin owned A holder")?);
        let holder_tx = holder.as_mut().context("missing holder")?;
        sqlx::query("SELECT pg_advisory_xact_lock(7821101,0)").execute(&mut **holder_tx).await.context("hold owned A")?;
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut **holder_tx).await.context("read holder PID")?;
        gate = Some(WireGate::start(db.pool().connect_options()).await.context("start owned wire gate")?);
        let proxy = gate.as_mut().context("missing gate")?;
        pool = Some(proxy_pool(db, proxy).await?);
        // Warm authentication and search_path outside the measured call. This
        // test controls network response skew, not connection startup speed.
        sqlx::query("SELECT 1").execute(pool.as_ref().context("missing pool")?).await.context("warm proxy connection")?;
        let mut observer = db.pool().acquire().await.context("reserve owned observer")?;
        let cleanup_pool = pool.as_ref().context("missing pool")?.clone();
        let (started_send, started_receive) = oneshot::channel();
        task = Some(tokio::spawn(async move {
            let _ = started_send.send(Instant::now());
            CacheGenerationRepository::drop_if_cleanup_eligible(&cleanup_pool, generation, &limits(300)).await.context("cleanup wrapper returned error")
        }));
        let started = timeout(Duration::from_secs(2), started_receive).await.context("cleanup startup deadline")?.context("cleanup startup signal lost")?;
        let release_at = started + Duration::from_millis(100);
        let business_deadline = started + Duration::from_millis(300);
        timeout_at(release_at, &mut proxy.installed).await.context("setter not installed before injection point")?.context("setter installation signal lost")?;
        ensure!(Instant::now() < release_at, "setter gate arrived too late for injection");
        sleep_until(release_at).await;
        proxy.release();
        let (pid, parent_locks) = timeout_at(business_deadline, async {
            loop {
                if let Some(observed) = sqlx::query_as::<_, (i32, i64)>(
                    "SELECT a.pid, (SELECT count(*) FROM pg_locks p WHERE p.pid=a.pid AND p.relation='cache_entry'::regclass) \
                     FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE l.locktype='advisory' \
                     AND l.classid=7821101 AND l.objid=0 AND l.objsubid=2 AND NOT l.granted \
                     AND l.database=(SELECT oid FROM pg_database WHERE datname=current_database()) \
                     AND $1=ANY(pg_blocking_pids(a.pid)) AND a.query='SELECT pg_advisory_xact_lock($1, $2)'",
                ).bind(holder_pid).fetch_optional(&mut *observer).await? { return Ok::<_, sqlx::Error>(observed); }
                tokio::task::yield_now().await;
            }
        }).await.context("controlled cleanup did not request A before D")?.context("observe controlled A waiter")?;
        cleanup_pid = Some(pid);
        let joined = timeout_at(started + Duration::from_secs(2), task.as_mut().context("missing cleanup task")?).await.context("cleanup join deadline")?;
        task.take();
        let outcome = joined.context("cleanup task panicked")??;
        let elapsed = started.elapsed();
        // Immediate post-return sample, before releasing A. No completion poll.
        let (waiters, own_waiters, activity): (i64, i64, sqlx::types::Json<serde_json::Value>) = sqlx::query_as(
            "SELECT count(*), count(*) FILTER(WHERE l.pid=$1), COALESCE(jsonb_agg(jsonb_build_object(\
             'pid',l.pid,'state',a.state,'wait',a.wait_event,'query_kind',CASE \
             WHEN a.query='SELECT pg_advisory_xact_lock($1, $2)' THEN 'controlled admission SQL' ELSE 'redacted' END)), '[]'::jsonb) \
             FROM pg_locks l LEFT JOIN pg_stat_activity a ON a.pid=l.pid WHERE l.locktype='advisory' \
             AND l.classid=7821101 AND l.objid=0 AND l.objsubid=2 AND NOT l.granted \
             AND l.database=(SELECT oid FROM pg_database WHERE datname=current_database())",
        ).bind(pid).fetch_one(&mut *observer).await.context("sample immediate post-return A waiters")?;
        let after = usage(db, generation).await?;
        Ok(Evidence { outcome, elapsed, pid, parent_locks, waiters, own_waiters, activity: activity.0, before, after })
    }.await;
    // Fallible observation never bypasses teardown. Accumulate only a boolean,
    // not error/packet data, and report teardown failure alongside observation.
    let clean = teardown(db, gate, pool, holder, task, cleanup_pid).await;
    match result {
        Err(error) if !clean => Err(error.context("observation and owned teardown failed")),
        Ok(_) if !clean => Err(anyhow!("owned acknowledgement case teardown failed")),
        result => result,
    }
}

async fn teardown(
    db: &TestDatabase,
    mut gate: Option<WireGate>,
    mut pool: Option<sqlx::PgPool>,
    mut holder: Option<sqlx::Transaction<'static, sqlx::Postgres>>,
    mut task: Option<CleanupTask>,
    cleanup_pid: Option<i32>,
) -> bool {
    let mut clean = true;
    if let Some(proxy) = gate.as_mut() {
        proxy.release();
    }
    if let Some(holder) = holder.take() {
        clean &= matches!(
            timeout(Duration::from_secs(3), holder.rollback()).await,
            Ok(Ok(()))
        );
    }
    if let Some(mut pending) = task.take() {
        match timeout(Duration::from_secs(3), &mut pending).await {
            Ok(joined) => clean &= matches!(joined, Ok(Ok(_))),
            Err(_) => {
                pending.abort();
                let _ = pending.await;
                clean = false;
            }
        }
    }
    if let Some(pool) = pool.take() {
        clean &= timeout(Duration::from_secs(3), pool.close()).await.is_ok();
    }
    if let Some(proxy) = gate.take() {
        clean &= proxy.shutdown().await.is_ok();
    }
    if let Some(pid) = cleanup_pid {
        // Teardown-only disappearance check, never substitutes for the sample.
        let gone = timeout(Duration::from_secs(3), async {
            loop {
                if sqlx::query_scalar::<_, bool>(
                    "SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1)",
                )
                .bind(pid)
                .fetch_one(db.pool())
                .await?
                {
                    return Ok::<_, sqlx::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        clean &= matches!(gone, Ok(Ok(())));
    }
    clean
}

#[tokio::test]
async fn deadline_deferral_acknowledges_admission_rollback_before_return() {
    let (db, generation) = fixture().await;
    let result = exercise(&db, generation).await;
    let cleaned = db.cleanup().await;
    assert!(
        cleaned.is_ok(),
        "owned acknowledgement database teardown failed"
    );
    let evidence = match result {
        Ok(evidence) => evidence,
        Err(error) => panic!("owned acknowledgement case failed: {error}"),
    };
    assert_eq!(evidence.parent_locks, 0);
    assert_eq!(
        evidence.outcome,
        CacheGenerationCleanupOutcome::DeferredDeadline
    );
    assert!(
        evidence.elapsed < Duration::from_secs(1),
        "cleanup elapsed {:?}",
        evidence.elapsed
    );
    assert_eq!(
        evidence.waiters, 0,
        "cleanup_pid={}; controlled_waiters={}; pending={}",
        evidence.pid, evidence.own_waiters, evidence.activity
    );
    assert_eq!(evidence.after, evidence.before);
}

type Counters = (i64, i64, i64, bool);
async fn counters(db: &TestDatabase, generation: i64) -> Result<Counters> {
    sqlx::query_as(
        "SELECT (SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id=1), \
         (SELECT COALESCE(SUM(physical_bytes),0)::BIGINT FROM cache_owner_physical_byte_usage), \
         (SELECT partitions_dropped FROM cache_entry_statistics_state WHERE id=TRUE), \
         EXISTS(SELECT 1 FROM cache_generation WHERE id=$1)",
    )
    .bind(generation)
    .fetch_one(db.pool())
    .await
    .context("read owned durable cleanup counters")
}
async fn boundary_case(mode: GateMode, exhaust: bool) {
    let (db, generation) = fixture().await;
    let (mut gate, mut pool, mut task): (_, _, Option<CleanupTask>) = (None, None, None);
    let mut cleanup_pid = None;
    let observed: Result<()> = async {
        let before = usage(&db, generation).await?;
        let counters_before = counters(&db, generation).await?;
        gate = Some(WireGate::start_mode(db.pool().connect_options(), mode).await.context("start boundary gate")?);
        let proxy = gate.as_mut().context("missing boundary gate")?;
        pool = Some(proxy_pool(&db, proxy).await?);
        // PID comes from the real prewarmed single physical proxy connection.
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(pool.as_ref().context("missing boundary pool")?).await.context("read actual boundary backend PID")?;
        cleanup_pid = Some(pid);
        let mut observer = db.pool().acquire().await.context("reserve boundary observer")?;
        proxy.arm();
        let cleanup_pool = pool.as_ref().context("missing boundary pool")?.clone();
        let (started_send, started_receive) = oneshot::channel();
        let (finished_send, finished_receive) = oneshot::channel();
        task = Some(tokio::spawn(async move {
            let _ = started_send.send(Instant::now());
            let result = CacheGenerationRepository::drop_if_cleanup_eligible(&cleanup_pool, generation, &limits(300))
                .await.map_err(anyhow::Error::from);
            let _ = finished_send.send(Instant::now());
            result
        }));
        let started = timeout(Duration::from_secs(2), started_receive).await.context("boundary startup deadline")?.context("boundary startup signal lost")?;
        let business_deadline = started + Duration::from_millis(300);
        timeout_at(business_deadline, &mut proxy.installed).await.context("selected command not accepted before D")?.context("boundary install signal lost")?;
        ensure!(Instant::now() < business_deadline, "boundary gate installed too late");
        if !exhaust {
            // Intentional absolute fault-injection point, D + 100ms. Only an
            // accepted server command's response is delayed, not business SQL.
            sleep_until(business_deadline + Duration::from_millis(100)).await;
            proxy.release();
        }
        let joined = timeout_at(started + Duration::from_secs(2), task.as_mut().context("missing boundary cleanup")?)
            .await.context("boundary cleanup join deadline")?;
        task.take();
        let returned = joined.context("boundary cleanup task panicked")?;
        let finished = finished_receive.await.context("boundary finish signal lost")?;
        let elapsed = finished.duration_since(started);
        // Sample before releasing an exhausted gate or closing the proxy. In
        // the successful BEGIN case, this must prove the real backend is idle.
        let state: Option<String> = sqlx::query_scalar("SELECT state FROM pg_stat_activity WHERE pid=$1")
            .bind(pid).fetch_optional(&mut *observer).await.context("sample boundary backend state")?;
        let after = usage(&db, generation).await?;
        let counters_after = counters(&db, generation).await?;
        // Checks return errors rather than panic, so every failed expectation
        // still reaches the shared owned teardown and database cleanup below.
        ensure!(elapsed < Duration::from_secs(1), "boundary elapsed {elapsed:?}");
        if exhaust {
            ensure!(elapsed >= Duration::from_millis(800), "recovery did not consume its absolute grace");
            ensure!(matches!(returned.as_ref().err().and_then(|error| error.downcast_ref::<attune_common::Error>()),
                Some(attune_common::Error::InvalidState(message)) if message == "cache cleanup rollback acknowledgement exceeded its completion deadline"), "wrong grace-exhaustion outcome");
            // This accepted BEGIN is still unacknowledged, not backend-gone.
            ensure!(state.as_deref() == Some("idle in transaction"), "exhausted gate lost its owned transaction");
        } else {
            ensure!(elapsed >= Duration::from_millis(400), "selected response was not awaited");
            match mode {
                GateMode::Commit => ensure!(matches!(returned.as_ref().err().and_then(|error| error.downcast_ref::<attune_common::Error>()),
                    Some(attune_common::Error::InvalidState(message)) if message == "cache cleanup commit outcome is unconfirmed after cycle deadline"), "commit uncertainty was hidden"),
                GateMode::UnexpectedBegin => ensure!(matches!(returned.as_ref().err().and_then(|error| error.downcast_ref::<attune_common::Error>()),
                    Some(attune_common::Error::Database(error)) if error.as_database_error().and_then(|error| error.code()).is_some_and(|code| code == "25P02")), "unexpected SQLSTATE was swallowed"),
                GateMode::Begin => {
                    ensure!(matches!(returned, Ok(CacheGenerationCleanupOutcome::DeferredDeadline)), "BEGIN recovery did not defer successfully");
                    ensure!(state.as_deref() == Some("idle"), "BEGIN recovery did not acknowledge real backend idle");
                }
                GateMode::Admission => bail!("admission case must use its unchanged counterexample"),
            }
        }
        if mode == GateMode::Commit {
            let bytes = before.context("fixture generation usage missing")?.1;
            ensure!(after.is_none() && !counters_after.3, "accepted COMMIT did not durably remove generation");
            ensure!((counters_after.0, counters_after.1, counters_after.2) ==
                (counters_before.0 - bytes, counters_before.1 - bytes, counters_before.2 + 1),
                "durable quota/statistics release was not exactly once");
        } else {
            ensure!(after == before && counters_after == counters_before, "non-commit recovery changed durable storage");
        }
        Ok(())
    }.await;
    // Teardown's disappearance poll never substitutes for the return snapshot.
    let clean = teardown(&db, gate, pool, None, task, cleanup_pid).await;
    let cleaned = db.cleanup().await;
    assert!(clean && cleaned.is_ok(), "owned boundary teardown failed");
    if let Err(error) = observed {
        panic!("boundary setup/observation failed: {error}");
    }
}

#[tokio::test]
async fn delayed_begin_acknowledgement_recovers_real_backend_idle() {
    boundary_case(GateMode::Begin, false).await;
}
#[tokio::test]
async fn delayed_commit_acknowledgement_preserves_uncertainty_and_durable_drop() {
    boundary_case(GateMode::Commit, false).await;
}
#[tokio::test]
async fn exhausted_rollback_grace_errors_without_claiming_backend_disposal() {
    boundary_case(GateMode::Begin, true).await;
}
#[tokio::test]
async fn synthetic_unexpected_sqlstate_during_recovery_fails_closed() {
    boundary_case(GateMode::UnexpectedBegin, false).await;
}
