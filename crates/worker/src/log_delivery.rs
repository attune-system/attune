//! Persistent worker custody and bounded background delivery for object logs.
use crate::log_delivery_spool::{LogDeliverySpool, SpoolDelivery, SpoolSegment};
use attune_common::{
    artifact_transport::ArtifactFileTransport,
    auth::jwt::JwtConfig,
    repositories::{
        delegated_execution::DelegatedExecutionRepository,
        file_evidence_retention::FileEvidenceRetentionRepository,
        log_delivery::{DeliverySegment, LogDeliveryRepository, PrepareLogDelivery},
        log_stream::LogStreamRepository,
    },
    Error, Result,
};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

#[cfg(test)]
#[path = "log_delivery_tests.rs"]
mod tests;

pub struct LogDeliveryManager {
    pool: sqlx::PgPool,
    spool: LogDeliverySpool,
    worker: i64,
    instance: Uuid,
    jwt: JwtConfig,
    api: String,
    client: reqwest::Client,
    tasks: TaskTracker,
    slots: Arc<Semaphore>,
    stop: CancellationToken,
    max_payload: u64,
    segment_bytes: usize,
}
struct Capture {
    segments: Vec<SpoolSegment>,
    bytes: u64,
    ready: Option<SpoolDelivery>,
    finalizing: bool,
}
/// Owns the live producer until settlement starts. Dropping before `seal`
/// cancels its uploader; `seal` transfers settlement to the manager before any
/// persistence await, including failed and ambiguously acknowledged handoffs.
pub struct SpoolingLogWriter {
    manager: Arc<LogDeliveryManager>,
    execution: i64,
    version: i64,
    stream: i64,
    attempt: Uuid,
    capture: Arc<Mutex<Capture>>,
    changed: Arc<Notify>,
    cancel: CancellationToken,
    handed_off: bool,
}
impl Drop for SpoolingLogWriter {
    fn drop(&mut self) {
        if !self.handed_off {
            self.cancel.cancel();
        }
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn hash(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
impl LogDeliveryManager {
    #[expect(
        clippy::too_many_arguments,
        reason = "Worker-owned lifetime and storage configuration"
    )]
    pub async fn open(
        pool: sqlx::PgPool,
        root: std::path::PathBuf,
        key: String,
        worker: i64,
        instance: Uuid,
        jwt: JwtConfig,
        api: String,
        config: attune_common::config::LogDeliveryConfig,
        max_payload: u64,
        segment_bytes: usize,
    ) -> Result<Arc<Self>> {
        let entry_bytes = max_payload
            .checked_mul(4)
            .and_then(|v| v.checked_add(64 * 1024))
            .ok_or_else(|| Error::validation("Log spool size overflow"))?;
        let spool = LogDeliverySpool::open(
            root,
            worker,
            &key,
            config.max_spool_bytes,
            entry_bytes,
            max_payload,
        )
        .await?;
        let result = Arc::new(Self {
            pool,
            spool,
            worker,
            instance,
            jwt,
            api: api.trim_end_matches('/').to_owned(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .map_err(|_| Error::execution("Log delivery HTTP client unavailable"))?,
            tasks: TaskTracker::new(),
            slots: Arc::new(Semaphore::new(config.max_finalizers)),
            stop: CancellationToken::new(),
            max_payload,
            segment_bytes,
        });
        result.recover().await?;
        Ok(result)
    }
    pub async fn writer(
        self: &Arc<Self>,
        execution: i64,
        version: i64,
        transport: Arc<dyn ArtifactFileTransport>,
    ) -> Result<SpoolingLogWriter> {
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::execution("Log delivery concurrency budget exhausted"))?;
        let mut conn = self.pool.acquire().await?;
        let attempt = DelegatedExecutionRepository::current_attempt(&mut conn, execution).await?;
        let stream = LogStreamRepository::find_by_artifact_version_with_conn(&mut conn, version)
            .await?
            .ok_or_else(|| Error::invalid_state("Log stream unavailable"))?
            .id;
        drop(conn);
        let capture = Arc::new(Mutex::new(Capture {
            segments: Vec::new(),
            bytes: 0,
            ready: None,
            finalizing: false,
        }));
        let changed = Arc::new(Notify::new());
        let cancel = CancellationToken::new();
        let writer = SpoolingLogWriter {
            manager: self.clone(),
            execution,
            version,
            stream,
            attempt,
            capture: capture.clone(),
            changed: changed.clone(),
            cancel: cancel.clone(),
            handed_off: false,
        };
        let this = self.clone();
        self.tasks.spawn(async move {
            this.live(version, capture, changed, cancel, transport, slot)
                .await;
        });
        Ok(writer)
    }
    async fn live(
        self: Arc<Self>,
        version: i64,
        capture: Arc<Mutex<Capture>>,
        changed: Arc<Notify>,
        cancel: CancellationToken,
        transport: Arc<dyn ArtifactFileTransport>,
        _slot: OwnedSemaphorePermit,
    ) {
        let mut next = 0usize;
        let mut live_failed = false;
        loop {
            // Register the notification before examining the state to avoid lost EOF.
            let notified = changed.notified();
            let (ready, segment) = {
                let mut c = capture.lock().unwrap();
                (
                    c.ready.take(),
                    if live_failed || c.finalizing {
                        None
                    } else {
                        c.segments.get(next).cloned()
                    },
                )
            };
            if let Some(entry) = ready {
                self.deliver(entry).await;
                return;
            }
            if let Some(segment) = segment {
                let request = tokio::time::timeout(
                    Duration::from_secs(2),
                    transport.commit_log_segment(version, segment.sequence, &segment.bytes),
                );
                tokio::select! {biased;
                 _=cancel.cancelled()=>return,
                 _=self.stop.cancelled()=>return,
                 result=request=>{if matches!(result,Ok(Ok(()))){next+=1;}else{live_failed=true;}},
                }
                continue;
            }
            tokio::select! {_=cancel.cancelled()=>return,_=self.stop.cancelled()=>return,_=notified=>{}}
        }
    }
    async fn recover(self: &Arc<Self>) -> Result<()> {
        let versions = self.spool.versions().await?;
        let pending = LogDeliveryRepository::pending_for_worker(
            &mut *self.pool.acquire().await?,
            self.worker,
        )
        .await?;
        for intent in pending {
            let valid = if versions.contains(&intent.artifact_version) {
                self.spool
                    .load(intent.artifact_version)
                    .await
                    .ok()
                    .is_some_and(|e| e.custody == intent.custody && e.attempt == intent.attempt)
            } else {
                false
            };
            if !valid {
                let mut tx = self.pool.begin().await?;
                LogDeliveryRepository::mark_incomplete(
                    &mut tx,
                    intent.artifact_version,
                    self.worker,
                    intent.custody,
                    "custody_lost",
                )
                .await?;
                tx.commit().await?;
            }
        }
        for version in versions {
            let entry = match self.spool.load(version).await {
                Ok(e) => e,
                Err(error) => {
                    tracing::warn!(version,%error,"Encrypted log spool recovery failed");
                    continue;
                }
            };
            let slot = self
                .slots
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| Error::execution("Log delivery stopped"))?;
            let this = self.clone();
            self.tasks.spawn(async move {
                let _slot = slot;
                this.deliver(entry).await;
            });
        }
        Ok(())
    }
    async fn deliver(&self, entry: SpoolDelivery) {
        loop {
            match tokio::select! {
                _=self.stop.cancelled()=>return,
                result=tokio::time::timeout((entry.finalize_by-chrono::Utc::now()).to_std().unwrap_or_default()+Duration::from_secs(5),self.deliver_inner(&entry))=>result.map_err(|_|Error::execution("Log delivery deadline exceeded")).and_then(|r|r),
            } {
                Ok(()) => return,
                Err(_) => tracing::warn!(
                    execution = entry.execution,
                    version = entry.version,
                    "Log delivery retry; encrypted custody retained"
                ),
            }
            // Expiry ends upload authority, not responsibility for custody.
            // A database outage may span the deadline; keep the bounded slot
            // until canonical terminal/absence evidence permits reclamation.
            if self.stop.is_cancelled() {
                return;
            }
            tokio::select! {_=self.stop.cancelled()=>return,_=tokio::time::sleep(Duration::from_millis(500))=>{}}
        }
    }
    async fn deliver_inner(&self, entry: &SpoolDelivery) -> Result<()> {
        let mut backoff = Duration::from_millis(100);
        loop {
            if self.stop.is_cancelled() {
                return Ok(());
            }
            let mut tx = self.pool.begin().await?;
            let intent = LogDeliveryRepository::find(&mut *tx, entry.version).await?;
            let Some(intent) = intent else {
                let absent = LogDeliveryRepository::custody_absent(&mut tx, entry.version).await?;
                tx.commit().await?;
                if !absent {
                    continue;
                }
                self.spool.remove(entry.version, entry.custody).await?;
                return Ok(());
            };
            if intent.custody != entry.custody
                || intent.worker != entry.worker
                || intent.attempt != entry.attempt
                || intent.source_instance != entry.source_instance
            {
                return Err(Error::PermissionDenied(
                    "Log custody binding changed".into(),
                ));
            }
            let ready =
                attune_common::repositories::artifact::ArtifactVersionRepository::find_by_id(
                    &mut *tx,
                    entry.version,
                )
                .await?
                .is_some_and(|v| {
                    v.body_state == Some(attune_common::models::ArtifactBodyState::Ready)
                });
            if ready || matches!(intent.state.as_str(), "incomplete" | "revoked" | "ready") {
                if ready && matches!(intent.state.as_str(), "prepared" | "active") {
                    LogDeliveryRepository::mark_ready(&mut tx, entry.version).await?;
                }
                tx.commit().await?;
                self.spool.remove(entry.version, entry.custody).await?;
                return Ok(());
            }
            if entry.finalize_by <= chrono::Utc::now() {
                LogDeliveryRepository::mark_incomplete(
                    &mut tx,
                    entry.version,
                    self.worker,
                    entry.custody,
                    "deadline",
                )
                .await?;
                tx.commit().await?;
                self.spool.remove(entry.version, entry.custody).await?;
                return Ok(());
            }
            // Terminal publication activates custody. Do not take source/target
            // locks for remote delivery while completion still needs those rows.
            if intent.state == "prepared" {
                tx.commit().await?;
                tokio::select! {_=self.stop.cancelled()=>return Ok(()),_=tokio::time::sleep(Duration::from_millis(25))=>{}}
                continue;
            }
            let intent = if intent.delivery_instance != self.instance {
                LogDeliveryRepository::claim(
                    &mut tx,
                    entry.version,
                    self.worker,
                    self.instance,
                    entry.custody,
                )
                .await?
            } else {
                intent
            };
            let token = attune_common::log_delivery::mint(&intent, &self.jwt)?;
            tx.commit().await?;
            let upload = async {
                for segment in &entry.segments {
                    let response = self
                        .client
                        .put(format!(
                            "{}/api/v1/internal/logs/{}/segments/{}",
                            self.api, entry.version, segment.sequence
                        ))
                        .bearer_auth(&token)
                        .body(segment.bytes.clone())
                        .send()
                        .await
                        .map_err(|_| Error::execution("Log segment request unavailable"))?;
                    if !response.status().is_success() {
                        return Err(Error::execution(format!(
                            "Log segment rejected ({})",
                            response.status().as_u16()
                        )));
                    }
                }
                let mut request = self
                    .client
                    .post(format!(
                        "{}/api/v1/internal/logs/{}/seal?truncated={}",
                        self.api, entry.version, entry.truncated
                    ))
                    .bearer_auth(&token);
                if let Some(manifest) = &entry.authenticated_manifest {
                    request = request.header("x-attune-log-final-manifest", manifest);
                }
                let response = request
                    .send()
                    .await
                    .map_err(|_| Error::execution("Log seal request unavailable"))?;
                if !response.status().is_success() {
                    return Err(Error::execution(format!(
                        "Log seal rejected ({})",
                        response.status().as_u16()
                    )));
                }
                Ok(())
            };
            let remaining = (entry.finalize_by - chrono::Utc::now())
                .to_std()
                .unwrap_or_default();
            tokio::select! {_=self.stop.cancelled()=>return Ok(()),result=tokio::time::timeout(remaining,upload)=>{if matches!(result,Ok(Ok(()))){tracing::info!(execution=entry.execution,version=entry.version,"Asynchronous log delivery sealed");continue;}}}
            tokio::select! {_=self.stop.cancelled()=>return Ok(()),_=tokio::time::sleep(backoff)=>{}}
            backoff = (backoff * 2).min(Duration::from_secs(2));
        }
    }
    pub async fn drain(&self) {
        self.tasks.close();
        if tokio::time::timeout(Duration::from_secs(65), self.tasks.wait())
            .await
            .is_err()
        {
            self.stop.cancel();
            self.tasks.wait().await;
        }
    }
}
impl SpoolingLogWriter {
    pub fn begin_finalization(&self) {
        self.capture.lock().unwrap().finalizing = true;
        self.changed.notify_one();
    }
    pub async fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut capture = self.capture.lock().unwrap();
        let total = capture
            .bytes
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= self.manager.max_payload)
            .ok_or_else(|| std::io::Error::other("Log custody payload budget exhausted"))?;
        for chunk in bytes.chunks(self.manager.segment_bytes) {
            let sequence = capture.segments.len() as i64;
            if sequence >= 16_384 {
                return Err(std::io::Error::other(
                    "Log custody segment budget exhausted",
                ));
            }
            capture.segments.push(SpoolSegment {
                sequence,
                bytes: chunk.to_vec(),
            });
        }
        capture.bytes = total;
        drop(capture);
        self.changed.notify_one();
        Ok(())
    }
    pub async fn seal(self, truncated: bool, manifest: Option<String>) -> Result<()> {
        // Once settlement starts, caller cancellation must not strand a file
        // published by spawn_blocking or an ambiguously committed DB handoff.
        let manager = self.manager.clone();
        manager
            .tasks
            .spawn(async move { self.seal_owned(truncated, manifest).await })
            .await
            .map_err(|_| Error::execution("Log custody settlement task failed"))?
    }

    async fn seal_owned(mut self, truncated: bool, manifest: Option<String>) -> Result<()> {
        let segments = std::mem::take(&mut self.capture.lock().unwrap().segments);
        if segments.is_empty() {
            return Err(Error::invalid_state("Empty allocated log custody"));
        }
        let entry = SpoolDelivery {
            execution: self.execution,
            version: self.version,
            stream: self.stream,
            worker: self.manager.worker,
            attempt: self.attempt,
            source_instance: self.manager.instance,
            custody: Uuid::new_v4(),
            finalize_by: chrono::Utc::now() + chrono::Duration::seconds(60),
            truncated,
            authenticated_manifest: manifest,
            segments,
        };
        let result = self.prepare_custody(&entry).await;
        // Reconcile even a failed save/prepare or a lost COMMIT acknowledgement.
        // The preparation future has ended before the consumer can prove
        // absence under exclusive graph admission and remove an orphan.
        self.capture.lock().unwrap().ready = Some(entry);
        self.handed_off = true;
        self.changed.notify_one();
        result
    }

    async fn prepare_custody(&self, entry: &SpoolDelivery) -> Result<()> {
        self.manager.spool.save(entry.clone()).await?;
        let mut digest = Sha256::new();
        let mut total = 0i64;
        let descriptors = entry
            .segments
            .iter()
            .map(|s| {
                digest.update(&s.bytes);
                total += s.bytes.len() as i64;
                DeliverySegment {
                    sequence: s.sequence,
                    size_bytes: s.bytes.len() as i64,
                    sha256: hash(&s.bytes),
                }
            })
            .collect();
        // Bound settlement independently of the upload deadline. SQLx owns a
        // cancelled transaction; reconciliation fences its eventual rollback
        // or COMMIT before treating missing custody as deletion evidence.
        let preparation = async {
            let mut tx = self.manager.pool.begin().await?;
            FileEvidenceRetentionRepository::lock_capture(&mut tx).await?;
            LogDeliveryRepository::prepare(
                &mut tx,
                PrepareLogDelivery {
                    execution: entry.execution,
                    version: entry.version,
                    stream: entry.stream,
                    worker: entry.worker,
                    instance: entry.source_instance,
                    custody: entry.custody,
                    segments: descriptors,
                    total_bytes: total,
                    sha256: hex(&digest.finalize()),
                    truncated: entry.truncated,
                    authenticated_manifest: entry.authenticated_manifest.clone(),
                    finalize_by: entry.finalize_by,
                },
            )
            .await?;
            tx.commit().await?;
            Ok(())
        };
        tokio::time::timeout(Duration::from_secs(10), preparation)
            .await
            .map_err(|_| Error::execution("Log custody preparation timed out"))?
    }
}
