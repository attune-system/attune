//! Durable workflow activity-log outbox and dispatcher.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use attune_common::artifact_transport::ArtifactFileTransport;
use attune_common::models::{
    ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
};
use attune_common::repositories::artifact::{
    ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
};
use attune_common::repositories::log_stream::LogStreamRepository;
use attune_common::repositories::workflow::WorkflowExecutionRepository;
use attune_common::repositories::workflow_log_outbox::WorkflowLogOutboxRepository;
use chrono::{SecondsFormat, Utc};
use sqlx::{PgConnection, PgPool};
use tracing::warn;
use uuid::Uuid;

const LOG_CONTENT_TYPE: &str = "text/plain";
const WORKFLOW_LOG_RETENTION: i32 = 50;
const CLAIM_LEASE: Duration = Duration::from_secs(120);
const IDLE_DELAY: Duration = Duration::from_millis(100);

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
        for chunk in line.as_bytes().chunks(self.segment_max_bytes) {
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

    pub async fn start(self) -> Result<()> {
        loop {
            match self.dispatch_once().await {
                Ok(true) => {}
                Ok(false) => tokio::time::sleep(IDLE_DELAY).await,
                Err(dispatch_error) => {
                    warn!(error = %dispatch_error, "Workflow log dispatcher cycle failed");
                    tokio::time::sleep(Duration::from_secs(1)).await;
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
            match record.kind.as_str() {
                "append" => {
                    let payload = record
                        .payload
                        .as_deref()
                        .ok_or_else(|| anyhow!("append outbox record has no payload"))?;
                    self.transport
                        .commit_log_segment(stream.version_id, record.sequence, payload)
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
            Result::<()>::Ok(())
        }
        .await;

        match result {
            Ok(()) => {
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
            Err(delivery_error) => {
                let retry_at = Utc::now()
                    + chrono::Duration::milliseconds(retry_delay_ms(record.attempt_count));
                let failure = if record.kind == "seal" {
                    "seal delivery failed"
                } else {
                    "append delivery failed"
                };
                WorkflowLogOutboxRepository::release_after_failure(
                    &self.pool,
                    record.id,
                    record.claimed_by,
                    retry_at,
                    failure,
                )
                .await?;
                warn!(
                    outbox_id = record.id,
                    workflow_execution = record.workflow_execution,
                    sequence = record.sequence,
                    error = %delivery_error,
                    "Workflow log delivery failed and was scheduled for retry"
                );
            }
        }
        Ok(true)
    }
}

fn retry_delay_ms(attempt_count: i32) -> i64 {
    let exponent = u32::try_from(attempt_count.saturating_sub(1).min(6)).unwrap_or(0);
    1_000_i64.saturating_mul(2_i64.pow(exponent))
}

#[derive(Clone, Copy)]
struct ResolvedLogStream {
    version_id: i64,
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
    LogStreamRepository::create(
        pool,
        version.id,
        u64::try_from(segment_max_bytes)?,
        flush_interval_ms,
    )
    .await?;
    Ok(ResolvedLogStream {
        version_id: version.id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Debug)]
    struct RecordingTransport {
        pool: PgPool,
        operations: Mutex<Vec<String>>,
        remaining_failures: AtomicUsize,
    }

    impl RecordingTransport {
        fn new(pool: PgPool, failures: usize) -> Self {
            Self {
                pool,
                operations: Mutex::new(Vec::new()),
                remaining_failures: AtomicUsize::new(failures),
            }
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
            let stream =
                LogStreamRepository::find_by_artifact_version(&self.pool, artifact_version)
                    .await?
                    .expect("workflow log stream");
            let mut transaction = self.pool.begin().await?;
            let locked = LogStreamRepository::lock(&mut transaction, stream.id).await?;
            if let Some(existing) =
                LogStreamRepository::find_segment(&mut transaction, stream.id, sequence).await?
            {
                if existing.size_bytes != content.len() as i64 {
                    return Err(attune_common::Error::invalid_state(
                        "conflicting test segment retry",
                    ));
                }
                transaction.commit().await?;
                return Ok(());
            }
            LogStreamRepository::commit_segment(
                &mut transaction,
                &locked,
                sequence,
                content.len() as i64,
                &format!("{:064x}", sequence),
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
            .unwrap();
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

    #[tokio::test]
    #[ignore = "integration test - requires database"]
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
    #[ignore = "integration test - requires database"]
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
    #[ignore = "integration test - requires database"]
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
    #[ignore = "integration test - requires database"]
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
        assert_eq!(pending.1.as_deref(), Some("append delivery failed"));
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
}
