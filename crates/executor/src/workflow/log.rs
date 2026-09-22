//! Durable workflow activity-log outbox and dispatcher.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use attune_common::artifact_transport::ArtifactFileTransport;
use attune_common::models::{
    ArtifactClassification, ArtifactType, ArtifactVisibility, LogStreamBackend, OwnerType,
    RetentionPolicyType,
};
use attune_common::repositories::artifact::{
    ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
};
use attune_common::repositories::log_stream::LogStreamRepository;
use attune_common::repositories::workflow::WorkflowExecutionRepository;
use attune_common::repositories::workflow_log_outbox::{
    DeliveryRebase, WorkflowLogOutboxRepository,
};
use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};
use tracing::warn;
use uuid::Uuid;

const LOG_CONTENT_TYPE: &str = "text/plain";
const WORKFLOW_LOG_RETENTION: i32 = 50;
const CLAIM_LEASE: Duration = Duration::from_secs(120);
const IDLE_DELAY: Duration = Duration::from_millis(100);

enum AppendReconciliation {
    Ready,
    AlreadyCommitted,
    Rebased,
    Permanent(&'static str),
}

enum DeliveryResult {
    Delivered,
    AlreadyCommitted,
    Rebased,
    Permanent(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

pub fn workflow_log_ref(action_ref: &str) -> String {
    format!("{action_ref}.workflow.log")
}

#[derive(Clone)]
pub struct WorkflowLogger {
    workflow_execution_id: i64,
    segment_max_bytes: usize,
}

impl WorkflowLogger {
    pub fn new(workflow_execution_id: i64, segment_max_bytes: usize) -> Self {
        Self {
            workflow_execution_id,
            segment_max_bytes,
        }
    }

    pub async fn info(&self, pool: &PgPool, message: impl AsRef<str>) -> Result<()> {
        self.log(pool, LogLevel::Info, message).await
    }

    pub async fn warn(&self, pool: &PgPool, message: impl AsRef<str>) -> Result<()> {
        self.log(pool, LogLevel::Warn, message).await
    }

    async fn log(&self, pool: &PgPool, level: LogLevel, message: impl AsRef<str>) -> Result<()> {
        let mut transaction = pool.begin().await?;
        self.log_with_conn(&mut transaction, level, message).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn log_with_conn(
        &self,
        connection: &mut PgConnection,
        level: LogLevel,
        message: impl AsRef<str>,
    ) -> Result<()> {
        let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let line = format!("{timestamp} [{}] {}\n", level.as_str(), message.as_ref());
        let segment_max_bytes = WorkflowLogOutboxRepository::effective_segment_max_bytes(
            connection,
            self.workflow_execution_id,
            self.segment_max_bytes,
        )
        .await?;
        for chunk in line.as_bytes().chunks(segment_max_bytes) {
            WorkflowLogOutboxRepository::enqueue_append(
                connection,
                self.workflow_execution_id,
                chunk,
            )
            .await?;
        }
        Ok(())
    }

    pub async fn seal_with_conn(&self, connection: &mut PgConnection) -> Result<()> {
        WorkflowLogOutboxRepository::enqueue_seal(connection, self.workflow_execution_id).await?;
        Ok(())
    }
}

pub struct WorkflowLogDispatcher {
    pool: PgPool,
    transport: Arc<dyn ArtifactFileTransport>,
    owner: Uuid,
    segment_max_bytes: usize,
    flush_interval_ms: u64,
}

impl WorkflowLogDispatcher {
    pub fn new(
        pool: PgPool,
        transport: Arc<dyn ArtifactFileTransport>,
        segment_max_bytes: usize,
        flush_interval_ms: u64,
    ) -> Self {
        Self {
            pool,
            transport,
            owner: Uuid::new_v4(),
            segment_max_bytes,
            flush_interval_ms,
        }
    }

    pub async fn start(self, mut shutdown: tokio::sync::broadcast::Receiver<()>) -> Result<()> {
        loop {
            match shutdown.try_recv() {
                Ok(()) | Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return Ok(()),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => return Ok(()),
            }
            match self.dispatch_once().await {
                Ok(true) => {}
                Ok(false) => {
                    tokio::select! {
                        _ = shutdown.recv() => return Ok(()),
                        _ = tokio::time::sleep(IDLE_DELAY) => {}
                    }
                }
                Err(dispatch_error) => {
                    warn!(error = %dispatch_error, "Workflow log dispatcher cycle failed");
                    tokio::select! {
                        _ = shutdown.recv() => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                    }
                }
            }
        }
    }

    pub async fn dispatch_once(&self) -> Result<bool> {
        let Some(record) =
            WorkflowLogOutboxRepository::claim_next(&self.pool, self.owner, CLAIM_LEASE).await?
        else {
            return Ok(false);
        };

        let mut append_attempt = None;
        let result = async {
            let stream = ensure_log_artifact(
                &self.pool,
                record.workflow_execution,
                &record.action_ref,
                record.parent_execution,
                self.segment_max_bytes,
                self.flush_interval_ms,
            )
            .await?;
            let delivery_sequence = WorkflowLogOutboxRepository::assign_delivery_sequence(
                &self.pool,
                record.id,
                record.claimed_by,
                stream.next_sequence,
            )
            .await?;
            match record.kind.as_str() {
                "append" => {
                    let payload = record
                        .payload
                        .as_deref()
                        .ok_or_else(|| anyhow!("append outbox record has no payload"))?;
                    append_attempt = Some((stream.version_id, delivery_sequence, payload));
                    match self
                        .reconcile_append(&record, stream.version_id, delivery_sequence, payload)
                        .await?
                    {
                        AppendReconciliation::Ready => {}
                        AppendReconciliation::AlreadyCommitted => {
                            return Ok(DeliveryResult::AlreadyCommitted)
                        }
                        AppendReconciliation::Rebased => return Ok(DeliveryResult::Rebased),
                        AppendReconciliation::Permanent(code) => {
                            return Ok(DeliveryResult::Permanent(code))
                        }
                    }
                    self.transport
                        .commit_log_segment(stream.version_id, delivery_sequence, payload)
                        .await?;
                }
                "seal" => {
                    self.transport
                        .seal_log_stream(stream.version_id, false)
                        .await?;
                    ArtifactVersionRepository::mark_log_ready(&self.pool, stream.version_id)
                        .await?;
                }
                kind => return Err(anyhow!("unknown workflow log outbox kind {kind}")),
            }
            Result::<DeliveryResult>::Ok(DeliveryResult::Delivered)
        }
        .await;

        match result {
            Ok(DeliveryResult::Delivered | DeliveryResult::AlreadyCommitted) => {
                if !WorkflowLogOutboxRepository::mark_delivered(
                    &self.pool,
                    record.id,
                    record.claimed_by,
                )
                .await?
                {
                    warn!(
                        outbox_id = record.id,
                        "Workflow log claim expired after delivery"
                    );
                }
            }
            Ok(DeliveryResult::Rebased) => {
                WorkflowLogOutboxRepository::release_after_failure(
                    &self.pool,
                    record.id,
                    record.claimed_by,
                    Utc::now(),
                    "append:sequence_rebased",
                )
                .await?;
            }
            Ok(DeliveryResult::Permanent(code)) => {
                self.mark_permanent(&record, code).await?;
            }
            Err(delivery_error) => {
                if let Some((version_id, delivery_sequence, payload)) = append_attempt {
                    match self
                        .reconcile_append(&record, version_id, delivery_sequence, payload)
                        .await?
                    {
                        AppendReconciliation::AlreadyCommitted => {
                            WorkflowLogOutboxRepository::mark_delivered(
                                &self.pool,
                                record.id,
                                record.claimed_by,
                            )
                            .await?;
                            return Ok(true);
                        }
                        AppendReconciliation::Rebased => {
                            WorkflowLogOutboxRepository::release_after_failure(
                                &self.pool,
                                record.id,
                                record.claimed_by,
                                Utc::now(),
                                "append:sequence_rebased",
                            )
                            .await?;
                            return Ok(true);
                        }
                        AppendReconciliation::Permanent(code) => {
                            self.mark_permanent(&record, code).await?;
                            return Ok(true);
                        }
                        AppendReconciliation::Ready => {}
                    }
                }
                let failure = delivery_failure_code(&record.kind, &delivery_error);
                if matches!(
                    delivery_error.downcast_ref::<attune_common::Error>(),
                    Some(attune_common::Error::LogSegmentConflict)
                ) && record.attempt_count >= 7
                {
                    self.mark_permanent(&record, "append:unresolved_sequence_conflict")
                        .await?;
                    return Ok(true);
                }
                if delivery_failure_is_retryable(&delivery_error) {
                    let retry_at = Utc::now()
                        + chrono::Duration::milliseconds(retry_delay_ms(record.attempt_count));
                    let released = WorkflowLogOutboxRepository::release_after_failure(
                        &self.pool,
                        record.id,
                        record.claimed_by,
                        retry_at,
                        &failure,
                    )
                    .await?;
                    if released {
                        warn!(
                            outbox_id = record.id,
                            workflow_execution = record.workflow_execution,
                            sequence = record.sequence,
                            error = %delivery_error,
                            "Workflow log delivery failed and was scheduled for retry"
                        );
                    } else {
                        warn!(
                            outbox_id = record.id,
                            error = %delivery_error,
                            "Workflow log claim expired before retry could be scheduled"
                        );
                    }
                } else {
                    self.mark_permanent(&record, &failure).await?;
                }
            }
        }
        Ok(true)
    }

    async fn reconcile_append(
        &self,
        record: &attune_common::models::log_stream::WorkflowLogOutboxRecord,
        artifact_version: i64,
        delivery_sequence: i64,
        payload: &[u8],
    ) -> Result<AppendReconciliation> {
        let state =
            LogStreamRepository::delivery_state(&self.pool, artifact_version, delivery_sequence)
                .await?;
        if let (Some(size), Some(digest)) = (state.segment_size, state.segment_sha256.as_deref()) {
            let payload_digest = Sha256::digest(payload)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            if size == payload.len() as i64 && digest == payload_digest {
                return Ok(AppendReconciliation::AlreadyCommitted);
            }
            return self.rebase_append(record, artifact_version).await;
        }
        if state.segment_size.is_some() || state.segment_sha256.is_some() {
            return Ok(AppendReconciliation::Permanent(
                "append:segment_metadata_invalid",
            ));
        }
        if state.backend != LogStreamBackend::ObjectSegments {
            return Ok(AppendReconciliation::Permanent(
                "append:incompatible_stream_backend",
            ));
        }
        if state.sealed {
            return Ok(AppendReconciliation::Permanent("append:stream_sealed"));
        }
        if delivery_sequence != state.next_sequence {
            return self.rebase_append(record, artifact_version).await;
        }
        Ok(AppendReconciliation::Ready)
    }

    async fn rebase_append(
        &self,
        record: &attune_common::models::log_stream::WorkflowLogOutboxRecord,
        artifact_version: i64,
    ) -> Result<AppendReconciliation> {
        match WorkflowLogOutboxRepository::rebase_to_stream_next(
            &self.pool,
            record.id,
            record.claimed_by,
            artifact_version,
        )
        .await?
        {
            DeliveryRebase::Rebased { attempt_count, .. } if attempt_count >= 7 => Ok(
                AppendReconciliation::Permanent("append:unresolved_sequence_conflict"),
            ),
            DeliveryRebase::Rebased { .. } => Ok(AppendReconciliation::Rebased),
            DeliveryRebase::Sealed => Ok(AppendReconciliation::Permanent("append:stream_sealed")),
            DeliveryRebase::Incompatible => Ok(AppendReconciliation::Permanent(
                "append:incompatible_stream_backend",
            )),
        }
    }

    async fn mark_permanent(
        &self,
        record: &attune_common::models::log_stream::WorkflowLogOutboxRecord,
        failure_code: &str,
    ) -> Result<()> {
        let stage = if record.kind == "seal" {
            "seal"
        } else {
            "write"
        };
        let failed = WorkflowLogOutboxRepository::mark_permanently_failed(
            &self.pool,
            record.id,
            record.claimed_by,
            failure_code,
            stage,
        )
        .await?;
        if failed {
            warn!(
                outbox_id = record.id,
                workflow_execution = record.workflow_execution,
                sequence = record.sequence,
                failure_code,
                "Workflow log delivery failed permanently"
            );
        } else {
            warn!(
                outbox_id = record.id,
                failure_code,
                "Workflow log claim expired before permanent failure could be recorded"
            );
        }
        Ok(())
    }
}

fn retry_delay_ms(attempt_count: i32) -> i64 {
    let exponent = u32::try_from(attempt_count.saturating_sub(1).min(6)).unwrap_or(0);
    1_000_i64.saturating_mul(2_i64.pow(exponent))
}

fn delivery_failure_is_retryable(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<attune_common::Error>(),
        Some(
            attune_common::Error::RetryableTransport(_)
                | attune_common::Error::Database(_)
                | attune_common::Error::Timeout(_)
                | attune_common::Error::LogSegmentConflict
        )
    )
}

fn delivery_failure_code(kind: &str, error: &anyhow::Error) -> String {
    let classification = match error.downcast_ref::<attune_common::Error>() {
        Some(attune_common::Error::RetryableTransport(_)) => "retryable_transport",
        Some(attune_common::Error::LogSegmentConflict) => "sequence_conflict",
        Some(attune_common::Error::Database(_)) => "database_unavailable",
        Some(attune_common::Error::Timeout(_)) => "timeout",
        Some(attune_common::Error::Io(_)) => "transport_rejected",
        Some(attune_common::Error::AuthenticationFailed(_)) => "authentication_failed",
        Some(attune_common::Error::PermissionDenied(_)) => "permission_denied",
        Some(attune_common::Error::InvalidState(_)) => "invalid_state",
        Some(_) => "operation_rejected",
        None => "internal_error",
    };
    format!("{kind}:{classification}")
}

#[derive(Clone, Copy)]
struct ResolvedLogStream {
    version_id: i64,
    next_sequence: i64,
}

async fn ensure_log_artifact(
    pool: &PgPool,
    workflow_execution_id: i64,
    action_ref: &str,
    parent_execution_id: i64,
    segment_max_bytes: usize,
    flush_interval_ms: u64,
) -> Result<ResolvedLogStream> {
    let reference = workflow_log_ref(action_ref);
    let mut transaction = pool.begin().await?;
    WorkflowExecutionRepository::find_by_id_for_update(&mut *transaction, workflow_execution_id)
        .await?
        .ok_or_else(|| anyhow!("workflow execution {workflow_execution_id} disappeared"))?;
    let artifact = ArtifactRepository::create_or_get(
        &mut transaction,
        CreateArtifactInput {
            r#ref: reference.clone(),
            scope: OwnerType::Action,
            owner: action_ref.to_string(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Public,
            classification: ArtifactClassification::General,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: WORKFLOW_LOG_RETENTION,
            name: Some(format!("Workflow log: {action_ref}")),
            description: Some(
                "Executor-generated workflow activity log (one version per execution)".into(),
            ),
            content_type: Some(LOG_CONTENT_TYPE.into()),
            data: None,
        },
    )
    .await?;
    let version = match ArtifactVersionRepository::find_by_artifact_and_execution(
        &mut *transaction,
        artifact.id,
        parent_execution_id,
    )
    .await?
    {
        Some(version) => version,
        None => {
            ArtifactVersionRepository::create_workflow_log_pending(
                &mut transaction,
                artifact.id,
                &artifact.r#ref,
                LOG_CONTENT_TYPE.into(),
                parent_execution_id,
                serde_json::json!({
                    "kind": "workflow_log",
                    "execution_id": parent_execution_id,
                    "log_state": "pending",
                }),
            )
            .await?
        }
    };
    if version.file_path.is_none() {
        return Err(anyhow!(
            "workflow log artifact {} version {} has no file_path",
            artifact.id,
            version.id
        ));
    }
    transaction.commit().await?;
    let stream = LogStreamRepository::create(
        pool,
        version.id,
        u64::try_from(segment_max_bytes)?,
        flush_interval_ms,
    )
    .await?;
    Ok(ResolvedLogStream {
        version_id: version.id,
        next_sequence: stream.next_sequence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tokio::sync::{Notify, Semaphore};

    #[derive(Debug)]
    struct RecordingTransport {
        pool: PgPool,
        operations: Mutex<Vec<String>>,
        remaining_failures: AtomicUsize,
        remaining_permanent_failures: AtomicUsize,
        fail_after_seals: AtomicUsize,
        commit_started: Notify,
        commit_gate: Option<Semaphore>,
    }

    impl RecordingTransport {
        fn new(pool: PgPool, failures: usize) -> Self {
            Self {
                pool,
                operations: Mutex::new(Vec::new()),
                remaining_failures: AtomicUsize::new(failures),
                remaining_permanent_failures: AtomicUsize::new(0),
                fail_after_seals: AtomicUsize::new(0),
                commit_started: Notify::new(),
                commit_gate: None,
            }
        }

        fn permanent_failure(pool: PgPool) -> Self {
            let mut transport = Self::new(pool, 0);
            transport.remaining_permanent_failures = AtomicUsize::new(1);
            transport
        }

        fn ambiguous_seal(pool: PgPool) -> Self {
            let mut transport = Self::new(pool, 0);
            transport.fail_after_seals = AtomicUsize::new(1);
            transport
        }

        fn gated(pool: PgPool) -> Self {
            let mut transport = Self::new(pool, 0);
            transport.commit_gate = Some(Semaphore::new(0));
            transport
        }

        fn operations(&self) -> Vec<String> {
            self.operations.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ArtifactFileTransport for RecordingTransport {
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

        async fn commit_log_segment(
            &self,
            artifact_version: i64,
            sequence: i64,
            content: &[u8],
        ) -> attune_common::Result<()> {
            self.commit_started.notify_one();
            if let Some(gate) = &self.commit_gate {
                gate.acquire().await.unwrap().forget();
            }
            if self
                .remaining_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(attune_common::Error::retryable_transport(
                    "injected object-store failure",
                ));
            }
            if self
                .remaining_permanent_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(attune_common::Error::invalid_state(
                    "injected permanent failure",
                ));
            }
            let stream =
                LogStreamRepository::find_by_artifact_version(&self.pool, artifact_version)
                    .await?
                    .expect("workflow log stream");
            let mut transaction = self.pool.begin().await?;
            let locked = LogStreamRepository::lock(&mut transaction, stream.id).await?;
            if let Some(existing) =
                LogStreamRepository::find_segment(&mut transaction, stream.id, sequence).await?
            {
                let content_sha256 = Sha256::digest(content)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                if existing.size_bytes != content.len() as i64 || existing.sha256 != content_sha256
                {
                    return Err(attune_common::Error::LogSegmentConflict);
                }
                transaction.commit().await?;
                return Ok(());
            }
            let content_sha256 = Sha256::digest(content)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            LogStreamRepository::commit_segment(
                &mut transaction,
                &locked,
                sequence,
                content.len() as i64,
                &content_sha256,
                &format!("test/{}/{sequence}", stream.id),
                "test-version",
            )
            .await?;
            transaction.commit().await?;
            self.operations
                .lock()
                .unwrap()
                .push(format!("append:{sequence}"));
            Ok(())
        }

        async fn seal_log_stream(
            &self,
            artifact_version: i64,
            _: bool,
        ) -> attune_common::Result<()> {
            let stream =
                LogStreamRepository::find_by_artifact_version(&self.pool, artifact_version)
                    .await?
                    .expect("workflow log stream");
            let mut transaction = self.pool.begin().await?;
            let locked = LogStreamRepository::lock(&mut transaction, stream.id).await?;
            if !locked.sealed {
                LogStreamRepository::seal(&mut transaction, stream.id, false).await?;
                self.operations.lock().unwrap().push("seal".to_string());
            }
            transaction.commit().await?;
            if self
                .fail_after_seals
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(attune_common::Error::retryable_transport(
                    "seal response was lost",
                ));
            }
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
            "recording"
        }

        fn base_dir(&self) -> &str {
            Path::new("").to_str().unwrap()
        }
    }

    async fn test_workflow() -> (attune_common::test_database::TestDatabase, i64) {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let config = attune_common::config::Config::load_from_file(&config_path).unwrap();
        let database = attune_common::test_database::TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let suffix = Uuid::new_v4().simple().to_string();
        let pack_ref = format!("logtest{}", &suffix[..8]);
        let pack_id: i64 = sqlx::query_scalar(
            "INSERT INTO pack (ref, label, version) VALUES ($1, 'Log test', '1.0.0') RETURNING id",
        )
        .bind(&pack_ref)
        .fetch_one(database.pool())
        .await
        .unwrap();
        let execution_id: i64 = sqlx::query_scalar(
            "INSERT INTO execution (action_ref, status) VALUES ($1, 'running') RETURNING id",
        )
        .bind(format!("{pack_ref}.workflow"))
        .fetch_one(database.pool())
        .await
        .unwrap();
        let workflow_definition: i64 = sqlx::query_scalar(
            "INSERT INTO workflow_definition \
             (ref, pack, pack_ref, label, version, definition) \
             VALUES ($1, $2, $3, 'Log test', '1.0.0', '{}'::jsonb) RETURNING id",
        )
        .bind(format!("{pack_ref}.workflow"))
        .bind(pack_id)
        .bind(&pack_ref)
        .fetch_one(database.pool())
        .await
        .unwrap();
        let workflow_execution: i64 = sqlx::query_scalar(
            "INSERT INTO workflow_execution \
             (execution, workflow_def, task_graph, status) \
             VALUES ($1, $2, '{}'::jsonb, 'running') RETURNING id",
        )
        .bind(execution_id)
        .bind(workflow_definition)
        .fetch_one(database.pool())
        .await
        .unwrap();
        (database, workflow_execution)
    }

    async fn enqueue(pool: &PgPool, workflow_execution: i64, payload: &'static [u8]) -> bool {
        let mut transaction = pool.begin().await.unwrap();
        let inserted = WorkflowLogOutboxRepository::enqueue_append(
            &mut transaction,
            workflow_execution,
            payload,
        )
        .await
        .unwrap();
        transaction.commit().await.unwrap();
        inserted
    }

    async fn enqueue_seal(pool: &PgPool, workflow_execution: i64) -> bool {
        let mut transaction = pool.begin().await.unwrap();
        let inserted =
            WorkflowLogOutboxRepository::enqueue_seal(&mut transaction, workflow_execution)
                .await
                .unwrap();
        transaction.commit().await.unwrap();
        inserted
    }

    async fn workflow_log_identity(pool: &PgPool, workflow_execution: i64) -> (i64, String) {
        sqlx::query_as(
            "SELECT execution.id, execution.action_ref \
             FROM workflow_execution workflow \
             JOIN execution ON execution.id = workflow.execution \
             WHERE workflow.id = $1",
        )
        .bind(workflow_execution)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn make_pending_available(pool: &PgPool, workflow_execution: i64) {
        sqlx::query(
            "UPDATE workflow_log_outbox SET available_at = NOW() \
             WHERE workflow_execution = $1 AND delivered_at IS NULL AND failed_at IS NULL",
        )
        .bind(workflow_execution)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn drain_replicas(first: &WorkflowLogDispatcher, second: &WorkflowLogDispatcher) {
        for _ in 0..20 {
            let (left, right) = tokio::join!(first.dispatch_once(), second.dispatch_once());
            if !left.unwrap() && !right.unwrap() {
                break;
            }
        }
    }

    #[test]
    fn retry_delay_is_bounded() {
        assert_eq!(retry_delay_ms(1), 1_000);
        assert_eq!(retry_delay_ms(7), 64_000);
        assert_eq!(retry_delay_ms(i32::MAX), 64_000);
    }

    #[test]
    fn stored_failure_code_omits_transport_details() {
        let error = anyhow!(attune_common::Error::retryable_transport(
            "upstream response contained a secret"
        ));
        assert_eq!(
            delivery_failure_code("append", &error),
            "append:retryable_transport"
        );
    }

    #[tokio::test]
    async fn replicas_serialize_append_against_append() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        let (first, second) = tokio::join!(
            enqueue(&pool, workflow_execution, b"first"),
            enqueue(&pool, workflow_execution, b"second")
        );
        assert!(first && second);
        enqueue_seal(&pool, workflow_execution).await;

        let transport = Arc::new(RecordingTransport::new(pool.clone(), 0));
        let first = WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500);
        let second = WorkflowLogDispatcher::new(pool, transport.clone(), 1024, 500);
        drain_replicas(&first, &second).await;
        assert_eq!(transport.operations(), vec!["append:0", "append:1", "seal"]);
    }

    #[tokio::test]
    async fn replicas_never_append_after_a_racing_seal() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        let (appended, sealed) = tokio::join!(
            enqueue(&pool, workflow_execution, b"racing append"),
            enqueue_seal(&pool, workflow_execution)
        );
        assert!(sealed);

        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT sequence, kind FROM workflow_log_outbox \
             WHERE workflow_execution = $1 ORDER BY sequence",
        )
        .bind(workflow_execution)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.last().unwrap().1, "seal");
        assert_eq!(rows.len(), usize::from(appended) + 1);

        let transport = Arc::new(RecordingTransport::new(pool.clone(), 0));
        let first = WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500);
        let second = WorkflowLogDispatcher::new(pool, transport.clone(), 1024, 500);
        drain_replicas(&first, &second).await;
        assert_eq!(transport.operations().last().unwrap(), "seal");
    }

    #[tokio::test]
    async fn expired_replica_claim_is_recovered_after_restart() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        enqueue(&pool, workflow_execution, b"recover me").await;
        enqueue_seal(&pool, workflow_execution).await;
        let abandoned_owner = Uuid::new_v4();
        WorkflowLogOutboxRepository::claim_next(&pool, abandoned_owner, Duration::from_millis(1))
            .await
            .unwrap()
            .expect("initial claim");
        tokio::time::sleep(Duration::from_millis(5)).await;

        let transport = Arc::new(RecordingTransport::new(pool.clone(), 0));
        let restarted = WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500);
        let peer = WorkflowLogDispatcher::new(pool, transport.clone(), 1024, 500);
        drain_replicas(&restarted, &peer).await;
        assert_eq!(transport.operations(), vec!["append:0", "seal"]);
    }

    #[tokio::test]
    async fn transient_store_failure_retains_head_and_blocks_seal() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        enqueue(&pool, workflow_execution, b"retry me").await;
        enqueue_seal(&pool, workflow_execution).await;
        let transport = Arc::new(RecordingTransport::new(pool.clone(), 1));
        let first = WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500);
        let second = WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500);

        assert!(first.dispatch_once().await.unwrap());
        assert!(!second.dispatch_once().await.unwrap());
        let pending: (i64, Option<String>, bool) = sqlx::query_as(
            "SELECT attempt_count::bigint, last_error, delivered_at IS NULL \
             FROM workflow_log_outbox WHERE workflow_execution = $1 AND sequence = 0",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending.0, 1);
        assert_eq!(pending.1.as_deref(), Some("append:retryable_transport"));
        assert!(pending.2);
        assert!(transport.operations().is_empty());

        sqlx::query(
            "UPDATE workflow_log_outbox SET available_at = NOW() \
             WHERE workflow_execution = $1 AND delivered_at IS NULL",
        )
        .bind(workflow_execution)
        .execute(&pool)
        .await
        .unwrap();
        drain_replicas(&first, &second).await;
        assert_eq!(transport.operations(), vec!["append:0", "seal"]);
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_log_outbox \
             WHERE workflow_execution = $1 AND delivered_at IS NULL",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0);
    }

    #[tokio::test]
    async fn upgrade_stream_sequence_is_assigned_before_delivery() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        let (parent_execution, action_ref) = workflow_log_identity(&pool, workflow_execution).await;
        let stream = ensure_log_artifact(
            &pool,
            workflow_execution,
            &action_ref,
            parent_execution,
            1024,
            500,
        )
        .await
        .unwrap();
        let transport = Arc::new(RecordingTransport::new(pool.clone(), 0));
        transport
            .commit_log_segment(stream.version_id, 0, b"pre-outbox segment")
            .await
            .unwrap();

        enqueue(&pool, workflow_execution, b"resumed segment").await;
        let dispatcher = WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500);
        assert!(dispatcher.dispatch_once().await.unwrap());

        let assigned: i64 = sqlx::query_scalar(
            "SELECT delivery_sequence FROM workflow_log_outbox \
             WHERE workflow_execution = $1 AND sequence = 0",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(assigned, 1);
        assert_eq!(transport.operations(), vec!["append:0", "append:1"]);
    }

    #[tokio::test]
    async fn configured_size_increase_keeps_existing_stream_limit() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        let (parent_execution, action_ref) = workflow_log_identity(&pool, workflow_execution).await;
        ensure_log_artifact(
            &pool,
            workflow_execution,
            &action_ref,
            parent_execution,
            8,
            500,
        )
        .await
        .unwrap();

        WorkflowLogger::new(workflow_execution, 1024)
            .info(&pool, "a message written after restart")
            .await
            .unwrap();
        let payload_sizes: Vec<i32> = sqlx::query_scalar(
            "SELECT OCTET_LENGTH(payload) FROM workflow_log_outbox \
             WHERE workflow_execution = $1 ORDER BY sequence",
        )
        .bind(workflow_execution)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(payload_sizes.len() > 1);
        assert!(payload_sizes.into_iter().all(|size| size <= 8));
    }

    #[tokio::test]
    async fn permanent_failure_blocks_tail_until_explicit_retry() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        enqueue(&pool, workflow_execution, b"repair me").await;
        enqueue_seal(&pool, workflow_execution).await;
        let transport = Arc::new(RecordingTransport::permanent_failure(pool.clone()));
        let dispatcher = WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500);

        assert!(dispatcher.dispatch_once().await.unwrap());
        assert!(!dispatcher.dispatch_once().await.unwrap());
        let failed: (i64, String) = sqlx::query_as(
            "SELECT id, last_error FROM workflow_log_outbox \
             WHERE workflow_execution = $1 AND failed_at IS NOT NULL",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(failed.1, "append:invalid_state");
        let degraded: String = sqlx::query_scalar(
            "SELECT version.meta->>'log_state' FROM artifact_version version \
             JOIN workflow_execution workflow ON workflow.execution = version.execution \
             WHERE workflow.id = $1",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(degraded, "degraded");
        assert!(WorkflowLogOutboxRepository::retry_failed(&pool, failed.0)
            .await
            .unwrap());

        drain_replicas(&dispatcher, &dispatcher).await;
        assert_eq!(transport.operations(), vec!["append:0", "seal"]);
        let recovered: String = sqlx::query_scalar(
            "SELECT version.meta->>'log_state' FROM artifact_version version \
             JOIN workflow_execution workflow ON workflow.execution = version.execution \
             WHERE workflow.id = $1",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(recovered, "ready");
    }

    #[tokio::test]
    async fn retention_preserves_undelivered_workflow_logs() {
        use attune_common::repositories::retention::{RetentionRepository, RetentionTarget};

        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        let (parent_execution, _) = workflow_log_identity(&pool, workflow_execution).await;
        enqueue(&pool, workflow_execution, b"must survive retention").await;
        WorkflowLogOutboxRepository::claim_next(&pool, Uuid::new_v4(), Duration::from_millis(1))
            .await
            .unwrap()
            .unwrap();
        sqlx::query(
            "UPDATE execution SET status = 'completed', updated = NOW() - INTERVAL '1 day' \
             WHERE id = $1",
        )
        .bind(parent_execution)
        .execute(&pool)
        .await
        .unwrap();

        let delete_error = sqlx::query("DELETE FROM execution WHERE id = $1")
            .bind(parent_execution)
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(matches!(
            delete_error,
            sqlx::Error::Database(ref error)
                if error.constraint() == Some("workflow_log_outbox_workflow_execution_fkey")
        ));
        let retained =
            RetentionRepository::run_target(&pool, RetentionTarget::Executions, 0, 10, false)
                .await
                .unwrap();
        assert_eq!(retained.deleted, 0);
        let payload: Vec<u8> = sqlx::query_scalar(
            "SELECT payload FROM workflow_log_outbox WHERE workflow_execution = $1",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(payload, b"must survive retention");
        tokio::time::sleep(Duration::from_millis(5)).await;

        let transport = Arc::new(RecordingTransport::new(pool.clone(), 0));
        let dispatcher = WorkflowLogDispatcher::new(pool.clone(), transport, 1024, 500);
        assert!(dispatcher.dispatch_once().await.unwrap());
        let deleted =
            RetentionRepository::run_target(&pool, RetentionTarget::Executions, 0, 10, false)
                .await
                .unwrap();
        assert_eq!(deleted.deleted, 1, "retention result: {deleted:?}");
    }

    #[tokio::test]
    async fn append_success_before_lease_expiry_replays_same_sequence() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        enqueue(&pool, workflow_execution, b"ambiguous append").await;
        let owner = Uuid::new_v4();
        let record =
            WorkflowLogOutboxRepository::claim_next(&pool, owner, Duration::from_millis(1))
                .await
                .unwrap()
                .unwrap();
        let (parent_execution, action_ref) = workflow_log_identity(&pool, workflow_execution).await;
        let stream = ensure_log_artifact(
            &pool,
            workflow_execution,
            &action_ref,
            parent_execution,
            1024,
            500,
        )
        .await
        .unwrap();
        let delivery_sequence = WorkflowLogOutboxRepository::assign_delivery_sequence(
            &pool,
            record.id,
            owner,
            stream.next_sequence,
        )
        .await
        .unwrap();
        let transport = Arc::new(RecordingTransport::new(pool.clone(), 0));
        transport
            .commit_log_segment(
                stream.version_id,
                delivery_sequence,
                record.payload.as_deref().unwrap(),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;

        let restarted = WorkflowLogDispatcher::new(pool, transport.clone(), 1024, 500);
        assert!(restarted.dispatch_once().await.unwrap());
        assert_eq!(transport.operations(), vec!["append:0"]);
    }

    #[tokio::test]
    async fn direct_writer_conflict_rebases_dispatcher_append() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        enqueue(&pool, workflow_execution, b"outbox payload").await;

        let transport = Arc::new(RecordingTransport::gated(pool.clone()));
        let dispatcher = Arc::new(WorkflowLogDispatcher::new(
            pool.clone(),
            transport.clone(),
            1024,
            500,
        ));
        let racing_dispatcher = dispatcher.clone();
        let dispatch = tokio::spawn(async move { racing_dispatcher.dispatch_once().await });
        transport.commit_started.notified().await;

        let version_id: i64 = sqlx::query_scalar(
            "SELECT version.id FROM artifact_version version \
             JOIN workflow_execution workflow ON workflow.execution = version.execution \
             WHERE workflow.id = $1",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        let direct = RecordingTransport::new(pool.clone(), 0);
        direct
            .commit_log_segment(version_id, 0, b"direct writer payload")
            .await
            .unwrap();
        transport.commit_gate.as_ref().unwrap().add_permits(2);

        assert!(dispatch.await.unwrap().unwrap());
        let rebased: (Option<i64>, bool) = sqlx::query_as(
            "SELECT delivery_sequence, delivered_at IS NOT NULL \
             FROM workflow_log_outbox WHERE workflow_execution = $1 AND sequence = 0",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rebased, (Some(1), false));
        make_pending_available(&pool, workflow_execution).await;
        assert!(dispatcher.dispatch_once().await.unwrap());
        let row: (Option<i64>, bool) = sqlx::query_as(
            "SELECT delivery_sequence, delivered_at IS NOT NULL \
             FROM workflow_log_outbox WHERE workflow_execution = $1 AND sequence = 0",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row, (Some(1), true));
        assert_eq!(direct.operations(), vec!["append:0"]);
        assert_eq!(transport.operations(), vec!["append:1"]);
    }

    #[tokio::test]
    async fn transient_failures_do_not_exhaust_sequence_rebase_attempts() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        enqueue(&pool, workflow_execution, b"outbox payload").await;
        let transport = Arc::new(RecordingTransport::new(pool.clone(), 6));
        let dispatcher = WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500);

        for _ in 0..6 {
            assert!(dispatcher.dispatch_once().await.unwrap());
            make_pending_available(&pool, workflow_execution).await;
        }
        let version_id: i64 = sqlx::query_scalar(
            "SELECT version.id FROM artifact_version version \
             JOIN workflow_execution workflow ON workflow.execution = version.execution \
             WHERE workflow.id = $1",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        RecordingTransport::new(pool.clone(), 0)
            .commit_log_segment(version_id, 0, b"direct writer payload")
            .await
            .unwrap();

        assert!(dispatcher.dispatch_once().await.unwrap());
        let rebased: (Option<i64>, i32, bool) = sqlx::query_as(
            "SELECT delivery_sequence, attempt_count, failed_at IS NOT NULL \
             FROM workflow_log_outbox WHERE workflow_execution = $1 AND sequence = 0",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rebased, (Some(1), 0, false));

        make_pending_available(&pool, workflow_execution).await;
        assert!(dispatcher.dispatch_once().await.unwrap());
        let delivered: bool = sqlx::query_scalar(
            "SELECT delivered_at IS NOT NULL FROM workflow_log_outbox \
             WHERE workflow_execution = $1 AND sequence = 0",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(delivered);
    }

    #[tokio::test]
    async fn lost_seal_response_is_replayed_safely() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        enqueue(&pool, workflow_execution, b"before seal").await;
        enqueue_seal(&pool, workflow_execution).await;
        let append_transport = Arc::new(RecordingTransport::new(pool.clone(), 0));
        let append_dispatcher =
            WorkflowLogDispatcher::new(pool.clone(), append_transport, 1024, 500);
        assert!(append_dispatcher.dispatch_once().await.unwrap());

        let transport = Arc::new(RecordingTransport::ambiguous_seal(pool.clone()));
        let dispatcher = WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500);
        assert!(dispatcher.dispatch_once().await.unwrap());
        make_pending_available(&pool, workflow_execution).await;
        assert!(dispatcher.dispatch_once().await.unwrap());
        assert_eq!(transport.operations(), vec!["seal"]);
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_log_outbox \
             WHERE workflow_execution = $1 AND delivered_at IS NULL",
        )
        .bind(workflow_execution)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending, 0);
    }

    #[tokio::test]
    async fn shutdown_waits_for_in_flight_delivery() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        enqueue(&pool, workflow_execution, b"finish during shutdown").await;
        let transport = Arc::new(RecordingTransport::gated(pool.clone()));
        let dispatcher = WorkflowLogDispatcher::new(pool, transport.clone(), 1024, 500);
        let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);
        let handle = tokio::spawn(dispatcher.start(shutdown_rx));
        transport.commit_started.notified().await;
        shutdown_tx.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(!handle.is_finished());
        transport.commit_gate.as_ref().unwrap().add_permits(1);
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(transport.operations(), vec!["append:0"]);
    }

    #[tokio::test]
    async fn advisory_lock_namespaces_do_not_collide_with_scheduler_ids() {
        let (database, _) = test_workflow().await;
        let pool = database.pool().clone();
        let mut scheduler_connection = pool.acquire().await.unwrap();
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(42_i64)
            .execute(&mut *scheduler_connection)
            .await
            .unwrap();

        let mut namespaced = pool.begin().await.unwrap();
        let artifact_lock: bool = sqlx::query_scalar(
            "SELECT pg_try_advisory_xact_lock(hashtext('artifact_version'), hashtext($1::text))",
        )
        .bind(42_i64)
        .fetch_one(&mut *namespaced)
        .await
        .unwrap();
        let stream_lock: bool = sqlx::query_scalar(
            "SELECT pg_try_advisory_xact_lock(hashtext('log_stream'), hashtext($1::text))",
        )
        .bind(42_i64)
        .fetch_one(&mut *namespaced)
        .await
        .unwrap();
        assert!(artifact_lock && stream_lock);
        namespaced.rollback().await.unwrap();
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(42_i64)
            .execute(&mut *scheduler_connection)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn backed_off_head_blocks_tail_without_claiming_it() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        for _ in 0..100 {
            enqueue(&pool, workflow_execution, b"queued").await;
        }
        let owner = Uuid::new_v4();
        let head = WorkflowLogOutboxRepository::claim_next(&pool, owner, Duration::from_secs(30))
            .await
            .unwrap()
            .unwrap();
        WorkflowLogOutboxRepository::release_after_failure(
            &pool,
            head.id,
            owner,
            Utc::now() + chrono::Duration::hours(1),
            "backoff",
        )
        .await
        .unwrap();

        assert!(WorkflowLogOutboxRepository::claim_next(
            &pool,
            Uuid::new_v4(),
            Duration::from_secs(30),
        )
        .await
        .unwrap()
        .is_none());
    }

    #[tokio::test]
    async fn last_delivery_racing_enqueue_preserves_one_head() {
        let (database, workflow_execution) = test_workflow().await;
        let pool = database.pool().clone();
        enqueue(&pool, workflow_execution, b"old head").await;
        let owner = Uuid::new_v4();
        let head = WorkflowLogOutboxRepository::claim_next(&pool, owner, Duration::from_secs(30))
            .await
            .unwrap()
            .unwrap();

        let (delivered, enqueued) = tokio::join!(
            WorkflowLogOutboxRepository::mark_delivered(&pool, head.id, owner),
            enqueue(&pool, workflow_execution, b"new head"),
        );
        assert!(delivered.unwrap());
        assert!(enqueued);
        let heads: Vec<(i64, bool)> = sqlx::query_as(
            "SELECT sequence, is_head FROM workflow_log_outbox \
             WHERE workflow_execution = $1 AND delivered_at IS NULL",
        )
        .bind(workflow_execution)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(heads, vec![(1, true)]);
    }
}
