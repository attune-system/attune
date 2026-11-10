//! Owned plaintext PostgreSQL fault relay. Diagnostics deliberately exclude wire payloads.
use std::{collections::HashMap, io, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, Mutex},
    task::JoinSet,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    SelectedOnly,
    Publication,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Forward,
    Disconnect,
}
#[derive(Debug)]
pub struct CommitHeld {
    pub connection: u64,
    pub backend_pid: i32,
    pub class: Class,
    pub status: u8,
    decision: oneshot::Sender<Decision>,
}
impl CommitHeld {
    pub fn decide(self, decision: Decision) -> io::Result<()> {
        self.decision.send(decision).map_err(|_| invalid())
    }
}
#[derive(Default, Debug)]
pub struct Report {
    pub backend_pids: Vec<i32>,
    pub held: usize,
    pub disconnected: usize,
    pub publications: usize,
}
pub struct OwnedCommitRelay {
    pub port: u16,
    events: mpsc::Receiver<CommitHeld>,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<io::Result<Report>>,
    release: tokio::sync::watch::Sender<bool>,
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "unsupported or invalid PostgreSQL protocol shape",
    )
}
impl OwnedCommitRelay {
    pub async fn start(target: String, stream: i64, plan: Vec<Class>) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let (events_tx, events) = mpsc::channel(16);
        let (stop, mut stop_rx) = oneshot::channel();
        let plan = Arc::new(Mutex::new(std::collections::VecDeque::from(plan)));
        let report = Arc::new(Mutex::new(Report::default()));
        let (release, release_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            let mut id = 0;
            let (cancel, _) = tokio::sync::watch::channel(false);
            let failure = loop {
                tokio::select! {
                    _ = &mut stop_rx => break None,
                    accepted=listener.accept()=> { let (client,_)=match accepted {Ok(v)=>v,Err(e)=>break Some(e)}; id+=1;
                        let target=target.clone(); let events=events_tx.clone(); let plan=plan.clone(); let report=report.clone();
                        let mut cancelled=cancel.subscribe(); let release=release_rx.clone(); tasks.spawn(async move { tokio::select! { result=connection(client,target,stream,id,events,plan,(report,release))=>result, _=cancelled.changed()=>Ok(()) } });
                    }
                    result=tasks.join_next(), if !tasks.is_empty()=> { match result {Some(Ok(Ok(())))=>(),Some(Ok(Err(e)))=>break Some(e),Some(Err(e))=>break Some(io::Error::other(e)),None=>break Some(invalid())} }
                }
            };
            // Explicit shutdown cancels both halves and joins every owned socket task.
            let _ = cancel.send(true);
            drain_tasks(&mut tasks, failure, std::time::Duration::from_secs(5)).await?;
            let value = std::mem::take(&mut *report.lock().await);
            Ok(value)
        });
        Ok(Self {
            port,
            events,
            stop,
            task,
            release,
        })
    }
    /// Release current and future gates before awaiting a failed test's owned work.
    pub fn release_holds(&self) {
        self.release.send_replace(true);
    }
    pub async fn next_event(&mut self) -> io::Result<CommitHeld> {
        self.events.recv().await.ok_or_else(invalid)
    }
    pub async fn shutdown(self) -> io::Result<Report> {
        let _ = self.stop.send(());
        self.task.await.map_err(io::Error::other)?
    }
}
async fn drain_tasks(
    tasks: &mut JoinSet<io::Result<()>>,
    mut failure: Option<io::Error>,
    deadline: std::time::Duration,
) -> io::Result<()> {
    let joined = tokio::time::timeout(deadline, async {
        while let Some(result) = tasks.join_next().await {
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e),
                Err(e) => Some(io::Error::other(e)),
            };
            if failure.is_none() {
                failure = error;
            }
        }
    })
    .await;
    if joined.is_err() {
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        failure.get_or_insert_with(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "relay tasks required watchdog abort",
            )
        });
    }
    match failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
async fn frame<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> io::Result<(u8, Vec<u8>)> {
    let kind = r.read_u8().await?;
    let len = r.read_u32().await?;
    if !(4..=16 * 1024 * 1024).contains(&len) {
        return Err(invalid());
    }
    let mut body = vec![0; len as usize - 4];
    r.read_exact(&mut body).await?;
    Ok((kind, body))
}
async fn write<W: tokio::io::AsyncWrite + Unpin>(w: &mut W, k: u8, b: &[u8]) -> io::Result<()> {
    w.write_u8(k).await?;
    w.write_u32((b.len() + 4) as u32).await?;
    w.write_all(b).await
}
#[derive(Default)]
struct Parser {
    statements: HashMap<Vec<u8>, u8>,
    portals: HashMap<Vec<u8>, (Vec<u8>, u8)>,
    savepoints: Vec<(String, bool, bool)>,
    publication_executions: usize,
    selected: bool,
    publication: bool,
}
fn string<'a>(b: &mut &'a [u8]) -> io::Result<&'a [u8]> {
    let n = b.iter().position(|x| *x == 0).ok_or_else(invalid)?;
    if n > 1024 * 1024 {
        return Err(invalid());
    }
    let (v, rest) = b.split_at(n);
    *b = &rest[1..];
    Ok(v)
}
fn name<'a>(b: &mut &'a [u8]) -> io::Result<&'a [u8]> {
    let n = string(b)?;
    if n.len() > 256 {
        return Err(invalid());
    }
    Ok(n)
}
fn take<'a>(b: &mut &'a [u8], n: usize) -> io::Result<&'a [u8]> {
    if b.len() < n {
        return Err(invalid());
    }
    let (v, r) = b.split_at(n);
    *b = r;
    Ok(v)
}
fn short(b: &mut &[u8]) -> io::Result<usize> {
    Ok(u16::from_be_bytes(take(b, 2)?.try_into().map_err(|_| invalid())?) as usize)
}
fn fingerprint(sql: &[u8]) -> u8 {
    let s = String::from_utf8_lossy(sql);
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.starts_with(
        "WITH input AS (SELECT jsonb_build_object('snapshot',object_log_close_snapshot($1),",
    ) && s
        .contains("UPDATE log_stream s SET close_phase='closing',close_revision=close_revision+1,")
    {
        1
    } else if s.starts_with(
        "UPDATE log_stream SET close_phase='published',sealed=true,sealed_at=clock_timestamp(),",
    ) {
        2
    } else {
        0
    }
}
impl Parser {
    fn frontend(&mut self, k: u8, mut b: &[u8], stream: i64) -> io::Result<Option<Option<Class>>> {
        if self.statements.len() > 4096 || self.portals.len() > 4096 {
            return Err(invalid());
        }
        match k {
            b'P' => {
                let name = name(&mut b)?.to_vec();
                let sql = string(&mut b)?;
                let upper = String::from_utf8_lossy(sql).trim().to_ascii_uppercase();
                if matches!(upper.as_str(), "COMMIT" | "ROLLBACK" | "BEGIN")
                    || upper.contains("SAVEPOINT")
                {
                    return Err(invalid());
                }
                self.statements.insert(name, fingerprint(sql));
            }
            b'B' => {
                let portal = name(&mut b)?.to_vec();
                let stmt = name(&mut b)?;
                let class = *self.statements.get(stmt).ok_or_else(invalid)?;
                let n = short(&mut b)?;
                let mut formats = Vec::new();
                for _ in 0..n {
                    formats.push(short(&mut b)?)
                }
                let count = short(&mut b)?;
                if n != 0 && n != 1 && n != count {
                    return Err(invalid());
                }
                let mut matched = false;
                for i in 0..count {
                    let len =
                        i32::from_be_bytes(take(&mut b, 4)?.try_into().map_err(|_| invalid())?);
                    if len < -1 {
                        return Err(invalid());
                    }
                    let value = if len < 0 {
                        &[][..]
                    } else {
                        take(&mut b, len as usize)?
                    };
                    if i == 0 && class != 0 {
                        let f = if n == 0 {
                            0
                        } else {
                            formats[if n == 1 { 0 } else { i }]
                        };
                        matched = match f {
                            0 => {
                                std::str::from_utf8(value)
                                    .ok()
                                    .and_then(|s| s.parse::<i64>().ok())
                                    == Some(stream)
                            }
                            1 => {
                                value.len() == 8
                                    && i64::from_be_bytes(value.try_into().map_err(|_| invalid())?)
                                        == stream
                            }
                            _ => return Err(invalid()),
                        };
                    }
                }
                if class != 0 && !matched {
                    return Err(invalid());
                }
                self.portals.insert(portal, (stmt.to_vec(), class));
            }
            b'E' => {
                let portal = name(&mut b)?;
                match self
                    .portals
                    .get(portal)
                    .map(|(_, class)| *class)
                    .ok_or_else(invalid)?
                {
                    1 => self.selected = true,
                    2 => {
                        self.publication = true;
                        self.publication_executions += 1;
                    }
                    _ => {}
                }
            }
            b'C' => {
                let which = take(&mut b, 1)?[0];
                let name = name(&mut b)?;
                match which {
                    b'S' => {
                        self.statements.remove(name);
                        self.portals.retain(|_, (statement, _)| statement != name);
                    }
                    b'P' => {
                        self.portals.remove(name);
                    }
                    _ => return Err(invalid()),
                }
            }
            b'Q' => {
                let sql = string(&mut b)?;
                if fingerprint(sql) != 0 {
                    return Err(invalid());
                }
                let sql = std::str::from_utf8(sql)
                    .map_err(|_| invalid())?
                    .trim()
                    .trim_end_matches(';')
                    .to_ascii_uppercase();
                if sql == "COMMIT" {
                    let class = if self.publication {
                        Some(Class::Publication)
                    } else if self.selected {
                        Some(Class::SelectedOnly)
                    } else {
                        None
                    };
                    self.selected = false;
                    self.publication = false;
                    self.savepoints.clear();
                    return Ok(Some(class));
                }
                let words = sql.split_whitespace().collect::<Vec<_>>();
                let valid_name = |n: &str| {
                    !n.is_empty()
                        && n.len() <= 256
                        && n.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                };
                match words.as_slice() {
                    ["BEGIN", ..] | ["ROLLBACK"] => {
                        self.selected = false;
                        self.publication = false;
                        self.savepoints.clear();
                    }
                    ["SAVEPOINT", n] if valid_name(n) => {
                        if self.savepoints.len() >= 64 {
                            return Err(invalid());
                        }
                        self.savepoints
                            .push((n.to_string(), self.selected, self.publication));
                    }
                    ["ROLLBACK", "TO", n] | ["ROLLBACK", "TO", "SAVEPOINT", n] if valid_name(n) => {
                        let i = self
                            .savepoints
                            .iter()
                            .rposition(|(name, _, _)| name == n)
                            .ok_or_else(invalid)?;
                        self.selected = self.savepoints[i].1;
                        self.publication = self.savepoints[i].2;
                        self.savepoints.truncate(i + 1);
                    }
                    ["RELEASE", n] | ["RELEASE", "SAVEPOINT", n] if valid_name(n) => {
                        let i = self
                            .savepoints
                            .iter()
                            .rposition(|(name, _, _)| name == n)
                            .ok_or_else(invalid)?;
                        self.savepoints.truncate(i);
                    }
                    _ if sql.contains("COMMIT")
                        || sql.contains("SAVEPOINT")
                        || sql.starts_with("ROLLBACK")
                        || sql.starts_with("RELEASE") =>
                    {
                        return Err(invalid())
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(None)
    }
}
async fn connection(
    client: TcpStream,
    target: String,
    stream: i64,
    id: u64,
    events: mpsc::Sender<CommitHeld>,
    plan: Arc<Mutex<std::collections::VecDeque<Class>>>,
    ownership: (Arc<Mutex<Report>>, tokio::sync::watch::Receiver<bool>),
) -> io::Result<()> {
    let (report, mut release) = ownership;
    let server = TcpStream::connect(target).await?;
    client.set_nodelay(true)?;
    server.set_nodelay(true)?;
    let (mut cr, mut cw) = client.into_split();
    let (mut sr, mut sw) = server.into_split();
    let (tx, mut rx) = mpsc::channel(32);
    let up = async {
        let len = cr.read_u32().await?;
        if !(8..=65536).contains(&len) {
            return Err(invalid());
        }
        let mut startup = vec![0; len as usize - 4];
        cr.read_exact(&mut startup).await?;
        if startup[..4] != 196608u32.to_be_bytes() {
            return Err(invalid());
        }
        sw.write_u32(len).await?;
        sw.write_all(&startup).await?;
        let mut parser = Parser::default();
        loop {
            let (k, b) = frame(&mut cr).await?;
            let before = parser.publication_executions;
            let event = parser.frontend(k, &b, stream)?;
            report.lock().await.publications += parser.publication_executions - before;
            if let Some(class) = event {
                tx.send(class).await.map_err(|_| invalid())?;
            }
            write(&mut sw, k, &b).await?;
        }
    };
    let down = async {
        let mut pid = 0;
        let mut backend_error = false;
        loop {
            let (k, b) = frame(&mut sr).await?;
            if k == b'K' {
                if b.len() != 8 {
                    return Err(invalid());
                }
                pid = i32::from_be_bytes(b[..4].try_into().map_err(|_| invalid())?);
                report.lock().await.backend_pids.push(pid);
            }
            if k == b'E' {
                backend_error = true;
            }
            if k == b'Z' && backend_error {
                // A deferred COMMIT failure has ErrorResponse + idle ReadyForQuery,
                // without CommandComplete. Do not allow stale classification on reuse.
                // Fail closed: unsupported pipelining cannot be correlated safely here.
                return Err(invalid());
            }
            if k == b'C' && (b == b"COMMIT\0" || b == b"ROLLBACK\0") {
                let class = if b == b"COMMIT\0" {
                    rx.recv().await.ok_or_else(invalid)?
                } else {
                    rx.try_recv().unwrap_or(None)
                };
                if b == b"COMMIT\0" {
                    let hold = {
                        let mut p = plan.lock().await;
                        if !*release.borrow() && class.is_some() && p.front().copied() == class {
                            p.pop_front();
                            true
                        } else {
                            false
                        }
                    };
                    if hold {
                        let mut buffered = Vec::new();
                        loop {
                            let (z, status) = frame(&mut sr).await?;
                            if z == b'E' {
                                return Err(invalid());
                            }
                            let idle = z == b'Z';
                            if idle && status != b"I" {
                                return Err(invalid());
                            }
                            buffered.push((z, status));
                            if idle {
                                break;
                            }
                            if buffered.len() > 64 {
                                return Err(invalid());
                            }
                        }
                        report.lock().await.held += 1;
                        let (decision, answer) = oneshot::channel();
                        events
                            .send(CommitHeld {
                                connection: id,
                                backend_pid: pid,
                                class: class.ok_or_else(invalid)?,
                                status: b'I',
                                decision,
                            })
                            .await
                            .map_err(|_| invalid())?;
                        let decision = if *release.borrow() {
                            Decision::Forward
                        } else {
                            tokio::select! {
                                value = answer => value.unwrap_or(Decision::Forward),
                                _ = release.changed() => Decision::Forward,
                            }
                        };
                        match decision {
                            Decision::Disconnect => {
                                report.lock().await.disconnected += 1;
                                return Ok(());
                            }
                            Decision::Forward => {
                                write(&mut cw, k, &b).await?;
                                for (z, status) in buffered {
                                    write(&mut cw, z, &status).await?;
                                }
                                continue;
                            }
                        }
                    }
                }
            }
            write(&mut cw, k, &b).await?;
        }
    };
    let result = tokio::select! {r=up=>r,r=down=>r};
    match result {
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
            ) =>
        {
            Ok(())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(p: &mut Parser, name: &[u8], sql: &[u8]) {
        let mut b = name.to_vec();
        b.push(0);
        b.extend(sql);
        b.extend([0, 0, 0]);
        p.frontend(b'P', &b, 42).unwrap();
    }
    fn bind(p: &mut Parser, value: i64) -> io::Result<Option<Option<Class>>> {
        let mut b = b"\0s\0".to_vec();
        b.extend(1u16.to_be_bytes());
        b.extend(1u16.to_be_bytes());
        b.extend(1u16.to_be_bytes());
        b.extend(8i32.to_be_bytes());
        b.extend(value.to_be_bytes());
        b.extend(0u16.to_be_bytes());
        p.frontend(b'B', &b, 42)
    }
    const SELECT:&[u8]=b"WITH input AS (SELECT jsonb_build_object('snapshot',object_log_close_snapshot($1), UPDATE log_stream s SET close_phase='closing',close_revision=close_revision+1,";
    #[test]
    fn preparation_sync_cached_bind_execute_and_unrelated_commit() {
        let mut p = Parser::default();
        parse(&mut p, b"s", SELECT);
        assert!(p.frontend(b'S', b"", 42).unwrap().is_none());
        assert_eq!(p.frontend(b'Q', b"COMMIT\0", 42).unwrap(), Some(None));
        for _ in 0..2 {
            bind(&mut p, 42).unwrap();
            p.frontend(b'E', b"\0\0\0\0\0", 42).unwrap();
            assert_eq!(
                p.frontend(b'Q', b"COMMIT\0", 42).unwrap(),
                Some(Some(Class::SelectedOnly))
            );
        }
    }
    #[test]
    fn rollback_discards_selection_and_close_invalidates_names() {
        let mut p = Parser::default();
        parse(&mut p, b"s", SELECT);
        bind(&mut p, 42).unwrap();
        p.frontend(b'E', b"\0\0\0\0\0", 42).unwrap();
        p.frontend(b'Q', b"ROLLBACK\0", 42).unwrap();
        assert_eq!(p.frontend(b'Q', b"COMMIT\0", 42).unwrap(), Some(None));
        p.frontend(b'C', b"Ss\0", 42).unwrap();
        assert!(bind(&mut p, 42).is_err());
    }
    #[test]
    fn mismatched_bind_and_unsupported_shapes_fail_without_payloads() {
        let mut p = Parser::default();
        parse(&mut p, b"s", SELECT);
        let e = bind(&mut p, 43).unwrap_err();
        assert_eq!(
            e.to_string(),
            "unsupported or invalid PostgreSQL protocol shape"
        );
        assert!(p.frontend(b'Q', b"SELECT 1; COMMIT\0", 42).is_err());
        assert!(p.frontend(b'E', b"missing\0", 42).is_err());
    }
    #[test]
    fn text_int8_bind_is_validated_at_execution() {
        let mut p = Parser::default();
        parse(&mut p, b"s", SELECT);
        let mut b = b"\0s\0".to_vec();
        b.extend([0, 0, 0, 1]);
        b.extend(2i32.to_be_bytes());
        b.extend(b"42");
        b.extend([0, 0]);
        p.frontend(b'B', &b, 42).unwrap();
        p.frontend(b'E', b"\0\0\0\0\0", 42).unwrap();
        assert_eq!(
            p.frontend(b'Q', b"COMMIT\0", 42).unwrap(),
            Some(Some(Class::SelectedOnly))
        );
        p.frontend(b'C', b"Ss\0", 42).unwrap();
        assert!(p.frontend(b'E', b"\0\0\0\0\0", 42).is_err());
    }
    #[test]
    fn publication_overrides_selection() {
        let mut p = Parser::default();
        parse(&mut p, b"s", SELECT);
        bind(&mut p, 42).unwrap();
        p.frontend(b'E', b"\0\0\0\0\0", 42).unwrap();
        parse(&mut p,b"s",b"UPDATE log_stream SET close_phase='published',sealed=true,sealed_at=clock_timestamp(),");
        bind(&mut p, 42).unwrap();
        p.frontend(b'E', b"\0\0\0\0\0", 42).unwrap();
        assert_eq!(
            p.frontend(b'Q', b"COMMIT\0", 42).unwrap(),
            Some(Some(Class::Publication))
        );
    }
}

#[cfg(test)]
mod savepoint_tests {
    use super::*;
    #[test]
    fn savepoint_rollback_restores_classification_and_release_is_not_root_commit() {
        let mut p = Parser {
            selected: true,
            ..Parser::default()
        };
        assert!(p
            .frontend(b'Q', b"SAVEPOINT owned\0", 42)
            .unwrap()
            .is_none());
        p.publication = true;
        assert!(p
            .frontend(b'Q', b"ROLLBACK TO SAVEPOINT owned\0", 42)
            .unwrap()
            .is_none());
        assert!(!p.publication);
        assert!(p.selected);
        assert!(p
            .frontend(b'Q', b"RELEASE SAVEPOINT owned\0", 42)
            .unwrap()
            .is_none());
        assert_eq!(
            p.frontend(b'Q', b"COMMIT\0", 42).unwrap(),
            Some(Some(Class::SelectedOnly))
        );
        assert!(p.frontend(b'Q', b"ROLLBACK TO absent\0", 42).is_err());
    }
}

#[cfg(test)]
mod backend_tests {
    use super::*;
    async fn rejected_completion(error: bool, status: u8) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let len = socket.read_u32().await.unwrap();
            let mut startup = vec![0; len as usize - 4];
            socket.read_exact(&mut startup).await.unwrap();
            write(&mut socket, b'R', &0u32.to_be_bytes()).await.unwrap();
            write(&mut socket, b'K', &[0, 0, 0, 42, 0, 0, 0, 0])
                .await
                .unwrap();
            write(&mut socket, b'Z', b"I").await.unwrap();
            loop {
                let (k, b) = frame(&mut socket).await.unwrap();
                if k == b'Q' && b == b"COMMIT\0" {
                    break;
                }
            }
            if error {
                write(&mut socket, b'E', b"SERROR\0C40001\0\0")
                    .await
                    .unwrap();
            }
            if !error {
                write(&mut socket, b'C', b"COMMIT\0").await.unwrap();
            }
            write(&mut socket, b'Z', &[status]).await.unwrap();
            // Keep upstream alive until the client or explicit relay shutdown closes it.
            let _ = socket.read_u8().await;
        });
        let mut relay = OwnedCommitRelay::start(target, 42, vec![Class::SelectedOnly])
            .await
            .unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", relay.port)).await.unwrap();
        client.write_u32(8).await.unwrap();
        client.write_u32(196608).await.unwrap();
        for _ in 0..3 {
            frame(&mut client).await.unwrap();
        }
        let mut parse=b"s\0WITH input AS (SELECT jsonb_build_object('snapshot',object_log_close_snapshot($1), UPDATE log_stream s SET close_phase='closing',close_revision=close_revision+1,".to_vec();
        parse.extend([0, 0, 0]);
        write(&mut client, b'P', &parse).await.unwrap();
        let mut bind = b"\0s\0".to_vec();
        bind.extend([0, 1, 0, 1, 0, 1]);
        bind.extend(8i32.to_be_bytes());
        bind.extend(42i64.to_be_bytes());
        bind.extend([0, 0]);
        write(&mut client, b'B', &bind).await.unwrap();
        write(&mut client, b'E', b"\0\0\0\0\0").await.unwrap();
        write(&mut client, b'S', b"").await.unwrap();
        write(&mut client, b'Q', b"COMMIT\0").await.unwrap();
        if error {
            assert_eq!(frame(&mut client).await.unwrap().0, b'E');
            // Attempt socket reuse after a real failed-COMMIT response shape.
            let _ = write(&mut client, b'Q', b"COMMIT\0").await;
            assert!(frame(&mut client).await.is_err());
            drop(client);
            assert!(relay.shutdown().await.is_err());
        } else {
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(2), relay.next_event())
                    .await
                    .unwrap()
                    .is_err()
            );
            assert!(relay.shutdown().await.is_err());
            drop(client);
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }
    #[tokio::test]
    async fn failed_commit_without_completion_rejects_unrelated_commit_reuse() {
        rejected_completion(true, b'I').await
    }
    #[tokio::test]
    async fn commit_with_nonidle_ready_is_not_successful_commit_proof() {
        rejected_completion(false, b'T').await
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };
    #[tokio::test]
    async fn first_error_does_not_skip_sibling_join() {
        let done = Arc::new(AtomicBool::new(false));
        let mut tasks = JoinSet::new();
        tasks.spawn(async { Err(io::Error::other("first")) });
        let flag = done.clone();
        tasks.spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            flag.store(true, Ordering::SeqCst);
            Ok(())
        });
        assert_eq!(
            drain_tasks(&mut tasks, None, Duration::from_secs(1))
                .await
                .unwrap_err()
                .to_string(),
            "first"
        );
        assert!(done.load(Ordering::SeqCst));
        assert!(tasks.is_empty());
    }
    #[tokio::test]
    async fn watchdog_aborts_and_joins_remainder_preserving_first_error() {
        struct Finished(Arc<AtomicBool>);
        impl Drop for Finished {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let done = Arc::new(AtomicBool::new(false));
        let flag = Finished(done.clone());
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            let _flag = flag;
            std::future::pending::<()>().await;
            Ok(())
        });
        assert_eq!(
            drain_tasks(
                &mut tasks,
                Some(io::Error::other("first")),
                Duration::from_millis(20)
            )
            .await
            .unwrap_err()
            .to_string(),
            "first"
        );
        assert!(done.load(Ordering::SeqCst));
        assert!(tasks.is_empty());
    }
}
