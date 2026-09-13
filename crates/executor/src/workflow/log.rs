//! Workflow activity log
//!
//! Each workflow execution's executor writes a per-execution **version** to a
//! single `FileText` artifact owned by the workflow action. The
//! artifact ref is `{action_ref}.workflow.log`, retention is per-action (50
//! versions by default), and each version is associated with the
//! `parent_execution_id` it was written for.
//!
//! As the executor orchestrates the workflow it commits timestamped lines
//! describing notable activity (workflow start, task dispatch, task
//! completion, transitions, cancellation, completion) so users can browse a
//! single readable log alongside the structured `workflow_execution` and
//! child execution records.
//!
//! Privacy: the log intentionally records *what happened*, not *what was
//! passed in or returned*. No task input or result content is written here.
//!
//! Storage: artifact + version are created lazily on first write. Each line is
//! stored as one or more ordered, immutable segments.
//!
//! Reliability: logging never changes the workflow's business result. Write
//! and seal failures are logged and recorded as a sanitized degraded state in
//! the artifact version metadata. A successful seal records a ready state.

use std::sync::Arc;

use anyhow::Result;
use chrono::{SecondsFormat, Utc};
use sqlx::PgPool;
use tokio::sync::OnceCell;
use tracing::warn;

use attune_common::artifact_transport::ArtifactFileTransport;
use attune_common::log_stream::{
    log_segment_commit_attempt, log_segment_retry_delay_ms, SegmentedLogConfig,
};
use attune_common::repositories::log_stream::LogStreamRepository;

use attune_common::models::{
    ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
};
use attune_common::repositories::{
    artifact::{
        ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput, LogFailureStage,
    },
    Create, FindByRef,
};

/// Log level tag included on each line.
#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

const LOG_CONTENT_TYPE: &str = "text/plain";
const WORKFLOW_LOG_RETENTION: i32 = 50;

/// Build the artifact ref for a workflow log keyed by the workflow's action ref.
pub fn workflow_log_ref(action_ref: &str) -> String {
    format!("{action_ref}.workflow.log")
}

/// Immutable segmented logger for a single workflow execution.
///
/// Construct one per workflow advancement / dispatch entry point. The first
/// `log()` call ensures the backing artifact + per-execution version row
/// exist and resolves the stream; subsequent calls reuse the cached IDs.
#[derive(Clone)]
pub struct WorkflowLogger {
    pool: PgPool,
    transport: Arc<dyn ArtifactFileTransport>,
    action_ref: String,
    parent_execution_id: i64,
    config: SegmentedLogConfig,
    stream: Arc<OnceCell<ResolvedLogStream>>,
}

#[derive(Clone, Copy)]
struct ResolvedLogStream {
    version_id: i64,
    stream_id: i64,
    segment_max_bytes: usize,
}

impl WorkflowLogger {
    pub fn new_with_transport(
        pool: PgPool,
        transport: Arc<dyn ArtifactFileTransport>,
        action_ref: impl Into<String>,
        parent_execution_id: i64,
        config: SegmentedLogConfig,
    ) -> Self {
        Self {
            pool,
            transport,
            action_ref: action_ref.into(),
            parent_execution_id,
            config,
            stream: Arc::new(OnceCell::new()),
        }
    }

    async fn resolve_stream(&self) -> Result<ResolvedLogStream> {
        let state = self
            .stream
            .get_or_try_init(|| async {
                ensure_log_artifact(
                    &self.pool,
                    &self.action_ref,
                    self.parent_execution_id,
                    self.config.max_segment_bytes,
                    self.config.flush_interval_ms,
                )
                .await
            })
            .await?;
        Ok(*state)
    }

    /// Append a single timestamped log line. Best-effort; failures are
    /// reported via `tracing::warn!` and do not propagate.
    pub async fn log(&self, level: LogLevel, message: impl AsRef<str>) {
        let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let line = format!("{ts} [{}] {}\n", level.as_str(), message.as_ref());

        let stream = match self.resolve_stream().await {
            Ok(state) => state,
            Err(e) => {
                warn!(
                    "workflow log: failed to resolve log path for execution {} (action {}): {}",
                    self.parent_execution_id, self.action_ref, e
                );
                self.mark_existing_degraded(LogFailureStage::Write).await;
                return;
            }
        };

        if let Err(e) = self.commit(&stream, line.as_bytes()).await {
            warn!(
                "workflow log: failed to commit segment for artifact version {}: {}",
                stream.version_id, e
            );
            self.mark_degraded(stream.version_id, LogFailureStage::Write)
                .await;
        }
    }

    pub async fn info(&self, msg: impl AsRef<str>) {
        self.log(LogLevel::Info, msg).await;
    }

    pub async fn warn(&self, msg: impl AsRef<str>) {
        self.log(LogLevel::Warn, msg).await;
    }

    pub async fn error(&self, msg: impl AsRef<str>) {
        self.log(LogLevel::Error, msg).await;
    }

    async fn commit(&self, stream: &ResolvedLogStream, bytes: &[u8]) -> Result<()> {
        for chunk in bytes.chunks(stream.segment_max_bytes) {
            let mut connection = self.pool.acquire().await?;
            let mut current =
                LogStreamRepository::find_by_id(&mut connection, stream.stream_id).await?;
            drop(connection);
            let mut sequence = current.next_sequence;
            let mut attempt = 1_u32;
            loop {
                let result = log_segment_commit_attempt(
                    self.config,
                    self.transport
                        .commit_log_segment(current.artifact_version, sequence, chunk),
                )
                .await;
                match result {
                    Ok(()) => break,
                    Err(error)
                        if attempt < self.config.retry_max_attempts
                            && (error.is_retryable_transport()
                                || error.expected_log_sequence().is_some()) =>
                    {
                        if let Some(expected_sequence) = error.expected_log_sequence() {
                            current = LogStreamRepository::find_by_id_in_pool(
                                &self.pool,
                                stream.stream_id,
                            )
                            .await?;
                            if current.sealed {
                                return Err(anyhow::anyhow!("workflow log stream is sealed"));
                            }
                            if current.next_sequence < expected_sequence {
                                return Err(anyhow::anyhow!(
                                    "workflow log sequence moved backwards while resolving a conflict"
                                ));
                            }
                            sequence = current.next_sequence;
                        }
                        let delay_ms = log_segment_retry_delay_ms(self.config, attempt);
                        warn!(
                            artifact_version = current.artifact_version,
                            sequence,
                            attempt,
                            next_attempt = attempt + 1,
                            delay_ms,
                            error = %error,
                            "Retrying workflow log segment commit"
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        attempt += 1;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }

    /// Seal the log without changing the workflow's business result.
    pub async fn seal(&self) {
        let stream = match self.resolve_stream().await {
            Ok(state) => state,
            Err(error) => {
                warn!(
                    "workflow log: failed to resolve stream while sealing execution {} (action {}): {}",
                    self.parent_execution_id, self.action_ref, error
                );
                self.mark_existing_degraded(LogFailureStage::Seal).await;
                return;
            }
        };

        if let Err(error) = self.seal_stream(stream.version_id).await {
            warn!(
                "workflow log: failed to seal artifact version {}: {}",
                stream.version_id, error
            );
            self.mark_degraded(stream.version_id, LogFailureStage::Seal)
                .await;
            return;
        }

        if let Err(error) =
            ArtifactVersionRepository::mark_log_ready(&self.pool, stream.version_id).await
        {
            warn!(
                "workflow log: failed to record ready state for artifact version {}: {}",
                stream.version_id, error
            );
        }
    }

    async fn seal_stream(&self, version_id: i64) -> Result<()> {
        self.transport
            .seal_log_stream(version_id, false)
            .await
            .map_err(Into::into)
    }

    async fn mark_degraded(&self, version_id: i64, stage: LogFailureStage) {
        if let Err(error) =
            ArtifactVersionRepository::mark_log_degraded(&self.pool, version_id, stage).await
        {
            warn!(
                "workflow log: failed to record degraded state for artifact version {}: {}",
                version_id, error
            );
        }
    }

    async fn mark_existing_degraded(&self, stage: LogFailureStage) {
        let result = async {
            let Some(artifact) =
                ArtifactRepository::find_by_ref(&self.pool, &workflow_log_ref(&self.action_ref))
                    .await?
            else {
                return Ok(None);
            };
            ArtifactVersionRepository::find_by_artifact_and_execution(
                &self.pool,
                artifact.id,
                self.parent_execution_id,
            )
            .await
        }
        .await;

        match result {
            Ok(Some(version)) => self.mark_degraded(version.id, stage).await,
            Ok(None) => {}
            Err(error) => warn!(
                "workflow log: failed to find artifact version for degraded state on execution {}: {}",
                self.parent_execution_id, error
            ),
        }
    }
}

/// Ensure the workflow log artifact, per-execution version, and stream exist.
async fn ensure_log_artifact(
    pool: &PgPool,
    action_ref: &str,
    parent_execution_id: i64,
    segment_max_bytes: usize,
    flush_interval_ms: u64,
) -> Result<ResolvedLogStream> {
    let r#ref = workflow_log_ref(action_ref);

    // Find or create the artifact row, scoped to the action.
    let artifact = match ArtifactRepository::find_by_ref(pool, &r#ref).await? {
        Some(a) => a,
        None => {
            ArtifactRepository::create(
                pool,
                CreateArtifactInput {
                    r#ref: r#ref.clone(),
                    scope: OwnerType::Action,
                    owner: action_ref.to_string(),
                    r#type: ArtifactType::FileText,
                    visibility: ArtifactVisibility::Public,
                    classification: ArtifactClassification::General,
                    retention_policy: RetentionPolicyType::Versions,
                    retention_limit: WORKFLOW_LOG_RETENTION,
                    name: Some(format!("Workflow log: {action_ref}")),
                    description: Some(
                        "Executor-generated workflow activity log (one version per execution)"
                            .into(),
                    ),
                    content_type: Some(LOG_CONTENT_TYPE.into()),
                    data: None,
                },
            )
            .await?
        }
    };

    // Find the version this execution already owns, else allocate a new one
    // tagged with parent_execution_id.
    let version = match ArtifactVersionRepository::find_by_artifact_and_execution(
        pool,
        artifact.id,
        parent_execution_id,
    )
    .await?
    {
        Some(v) => v,
        None => {
            ArtifactVersionRepository::create_file_backed(
                pool,
                artifact.id,
                &artifact.r#ref,
                LOG_CONTENT_TYPE.into(),
                Some(parent_execution_id),
                Some(serde_json::json!({
                    "kind": "workflow_log",
                    "execution_id": parent_execution_id,
                    "log_state": "pending",
                })),
                Some("executor".into()),
            )
            .await?
        }
    };

    version.file_path.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "workflow log artifact {} version {} has no file_path",
            artifact.id,
            version.id
        )
    })?;
    let stream = LogStreamRepository::create(
        pool,
        version.id,
        u64::try_from(segment_max_bytes)?,
        flush_interval_ms,
    )
    .await?;
    Ok(ResolvedLogStream {
        version_id: version.id,
        stream_id: stream.id,
        segment_max_bytes: usize::try_from(stream.max_unflushed_bytes)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{path::Path, sync::Mutex};
    use tokio::sync::Barrier;

    fn log_config(segment_bytes: usize, flush_interval_ms: u64) -> SegmentedLogConfig {
        SegmentedLogConfig {
            initial_segment_bytes: segment_bytes,
            max_segment_bytes: segment_bytes,
            flush_interval_ms,
            retry_max_attempts: 3,
            retry_attempt_timeout_ms: 100,
            retry_initial_backoff_ms: 1,
            retry_max_backoff_ms: 2,
            finalization_timeout_ms: 100,
        }
    }

    #[derive(Debug)]
    struct RecordingTransport {
        pool: PgPool,
        segments: Mutex<Vec<(i64, i64, Vec<u8>)>>,
        seals: Mutex<Vec<(i64, bool)>>,
        fail_commits: bool,
        fail_seals: bool,
        commit_errors: Mutex<VecDeque<attune_common::Error>>,
        attempts: Mutex<Vec<(i64, i64, Vec<u8>)>>,
        commit_barrier: Option<Arc<Barrier>>,
        barrier_entries: AtomicUsize,
    }

    impl RecordingTransport {
        fn with_commit_errors(
            pool: PgPool,
            errors: impl IntoIterator<Item = attune_common::Error>,
        ) -> Self {
            Self {
                pool,
                segments: Mutex::new(Vec::new()),
                seals: Mutex::new(Vec::new()),
                fail_commits: false,
                fail_seals: false,
                commit_errors: Mutex::new(errors.into_iter().collect()),
                attempts: Mutex::new(Vec::new()),
                commit_barrier: None,
                barrier_entries: AtomicUsize::new(0),
            }
        }

        fn failing(pool: PgPool, fail_commits: bool, fail_seals: bool) -> Self {
            Self {
                pool,
                segments: Mutex::new(Vec::new()),
                seals: Mutex::new(Vec::new()),
                fail_commits,
                fail_seals,
                commit_errors: Mutex::new(VecDeque::new()),
                attempts: Mutex::new(Vec::new()),
                commit_barrier: None,
                barrier_entries: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl attune_common::artifact_transport::ArtifactFileTransport for RecordingTransport {
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
            self.attempts
                .lock()
                .unwrap()
                .push((artifact_version, sequence, content.to_vec()));
            if let Some(error) = self.commit_errors.lock().unwrap().pop_front() {
                return Err(error);
            }
            if self.fail_commits {
                return Err(attune_common::Error::invalid_state(
                    "injected workflow log write failure with sensitive detail",
                ));
            }
            let stream =
                LogStreamRepository::find_by_artifact_version(&self.pool, artifact_version)
                    .await?
                    .expect("log stream");
            if let Some(barrier) = &self.commit_barrier {
                if self.barrier_entries.fetch_add(1, Ordering::SeqCst) < 2 {
                    barrier.wait().await;
                }
            }
            let mut transaction = self.pool.begin().await?;
            let locked = LogStreamRepository::lock(&mut transaction, stream.id).await?;
            if locked.sealed {
                return Err(attune_common::Error::invalid_state("log stream is sealed"));
            }
            if sequence != locked.next_sequence {
                return Err(attune_common::Error::log_sequence_conflict(
                    locked.next_sequence,
                ));
            }
            LogStreamRepository::commit_segment(
                &mut transaction,
                &locked,
                sequence,
                content.len() as i64,
                &"0".repeat(64),
                &format!("test/{sequence}"),
                "e:test-version",
            )
            .await?;
            transaction.commit().await?;
            self.segments
                .lock()
                .unwrap()
                .push((artifact_version, sequence, content.to_vec()));
            Ok(())
        }

        async fn seal_log_stream(
            &self,
            artifact_version: i64,
            truncated: bool,
        ) -> attune_common::Result<()> {
            if self.fail_seals {
                return Err(attune_common::Error::invalid_state(
                    "injected workflow log seal failure with sensitive detail",
                ));
            }
            self.seals
                .lock()
                .unwrap()
                .push((artifact_version, truncated));
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
            "api-test"
        }

        fn base_dir(&self) -> &str {
            Path::new("").to_str().unwrap()
        }
    }

    #[tokio::test]
    #[ignore = "integration test - requires database"]
    async fn workflow_logs_recover_transient_commits_and_preserve_order() {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let config = attune_common::config::Config::load_from_file(&config_path).unwrap();
        let database = attune_common::test_database::TestDatabase::create(&config.database)
            .await
            .unwrap();
        let transport = Arc::new(RecordingTransport::with_commit_errors(
            database.pool().clone(),
            [
                attune_common::Error::retryable_transport("temporary one"),
                attune_common::Error::retryable_transport("temporary two"),
            ],
        ));
        let logger = WorkflowLogger::new_with_transport(
            database.pool().clone(),
            transport.clone(),
            "core.test_workflow",
            42,
            log_config(4, 1234),
        );

        let resolved = logger.resolve_stream().await.unwrap();
        let version_id = resolved.version_id;
        let stream_id = resolved.stream_id;
        let stream = LogStreamRepository::find_by_artifact_version(database.pool(), version_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stream.max_unflushed_bytes, 4);
        assert_eq!(stream.max_unflushed_milliseconds, 1234);

        logger.commit(&resolved, b"abcdef").await.unwrap();
        let reconstructed = WorkflowLogger::new_with_transport(
            database.pool().clone(),
            transport.clone(),
            "core.test_workflow",
            42,
            log_config(8, 4321),
        );
        let reconstructed_stream = reconstructed.resolve_stream().await.unwrap();
        assert_eq!(reconstructed_stream.version_id, version_id);
        assert_eq!(reconstructed_stream.stream_id, stream_id);
        let persisted_stream =
            LogStreamRepository::find_by_artifact_version(database.pool(), version_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(persisted_stream.max_unflushed_bytes, 4);
        assert_eq!(persisted_stream.max_unflushed_milliseconds, 1234);
        reconstructed
            .commit(&reconstructed_stream, b"ghijkl")
            .await
            .unwrap();
        reconstructed.seal().await;

        let first_version_id = {
            let segments = transport.segments.lock().unwrap();
            assert_eq!(segments.len(), 4);
            assert_eq!(segments[0].1, 0);
            assert_eq!(segments[1].1, 1);
            assert_eq!(segments[2].1, 2);
            assert_eq!(segments[3].1, 3);
            assert_eq!(segments[0].2, b"abcd");
            assert_eq!(segments[1].2, b"ef");
            assert_eq!(segments[2].2, b"ghij");
            assert_eq!(segments[3].2, b"kl");
            segments[0].0
        };
        assert_eq!(transport.attempts.lock().unwrap().len(), 6);
        assert_eq!(
            &transport.attempts.lock().unwrap()[..3],
            &[
                (version_id, 0, b"abcd".to_vec()),
                (version_id, 0, b"abcd".to_vec()),
                (version_id, 0, b"abcd".to_vec()),
            ]
        );
        assert_eq!(
            *transport.seals.lock().unwrap(),
            vec![(first_version_id, false)]
        );
        let version = ArtifactVersionRepository::find_by_id(database.pool(), version_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(version.meta.unwrap()["log_state"], "ready");
    }

    #[tokio::test]
    #[ignore = "integration test - requires database"]
    async fn concurrent_workflow_logs_recover_sequence_collision_without_losing_lines() {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let config = attune_common::config::Config::load_from_file(&config_path).unwrap();
        let database = attune_common::test_database::TestDatabase::create(&config.database)
            .await
            .unwrap();
        let transport = Arc::new(RecordingTransport {
            pool: database.pool().clone(),
            segments: Mutex::new(Vec::new()),
            seals: Mutex::new(Vec::new()),
            fail_commits: false,
            fail_seals: false,
            commit_errors: Mutex::new(VecDeque::new()),
            attempts: Mutex::new(Vec::new()),
            commit_barrier: Some(Arc::new(Barrier::new(2))),
            barrier_entries: AtomicUsize::new(0),
        });
        let first = WorkflowLogger::new_with_transport(
            database.pool().clone(),
            transport.clone(),
            "core.concurrent_workflow",
            45,
            log_config(64, 500),
        );
        let second = WorkflowLogger::new_with_transport(
            database.pool().clone(),
            transport.clone(),
            "core.concurrent_workflow",
            45,
            log_config(64, 500),
        );
        let first_stream = first.resolve_stream().await.unwrap();
        let second_stream = second.resolve_stream().await.unwrap();

        let (first_result, second_result) = tokio::join!(
            first.commit(&first_stream, b"first line\n"),
            second.commit(&second_stream, b"second line\n")
        );
        first_result.unwrap();
        second_result.unwrap();

        let mut segments = transport.segments.lock().unwrap().clone();
        segments.sort_by_key(|segment| segment.1);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].1, 0);
        assert_eq!(segments[1].1, 1);
        let contents = segments
            .iter()
            .map(|segment| segment.2.as_slice())
            .collect::<Vec<_>>();
        assert!(contents.contains(&b"first line\n".as_slice()));
        assert!(contents.contains(&b"second line\n".as_slice()));
        assert_eq!(transport.attempts.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    #[ignore = "integration test - requires database"]
    async fn workflow_log_write_failure_is_non_fatal_and_durably_degraded() {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let config = attune_common::config::Config::load_from_file(&config_path).unwrap();
        let database = attune_common::test_database::TestDatabase::create(&config.database)
            .await
            .unwrap();
        let transport = Arc::new(RecordingTransport::failing(
            database.pool().clone(),
            true,
            false,
        ));
        let logger = WorkflowLogger::new_with_transport(
            database.pool().clone(),
            transport.clone(),
            "core.write_failure_workflow",
            43,
            log_config(64 * 1024, 500),
        );

        logger.info("business completion remains successful").await;

        let version_id = logger.resolve_stream().await.unwrap().version_id;
        let version = ArtifactVersionRepository::find_by_id(database.pool(), version_id)
            .await
            .unwrap()
            .unwrap();
        let meta = version.meta.unwrap();
        assert_eq!(meta["log_state"], "degraded");
        assert_eq!(meta["log_failure"], "write");
        assert!(!meta.to_string().contains("sensitive detail"));
        assert_eq!(transport.attempts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    #[ignore = "integration test - requires database"]
    async fn workflow_log_seal_failure_is_non_fatal_and_durably_degraded() {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let config = attune_common::config::Config::load_from_file(&config_path).unwrap();
        let database = attune_common::test_database::TestDatabase::create(&config.database)
            .await
            .unwrap();
        let transport = Arc::new(RecordingTransport::failing(
            database.pool().clone(),
            false,
            true,
        ));
        let logger = WorkflowLogger::new_with_transport(
            database.pool().clone(),
            transport,
            "core.seal_failure_workflow",
            44,
            log_config(64 * 1024, 500),
        );

        logger.seal().await;

        let version_id = logger.resolve_stream().await.unwrap().version_id;
        let version = ArtifactVersionRepository::find_by_id(database.pool(), version_id)
            .await
            .unwrap()
            .unwrap();
        let meta = version.meta.unwrap();
        assert_eq!(meta["log_state"], "degraded");
        assert_eq!(meta["log_failure"], "seal");
        assert!(!meta.to_string().contains("sensitive detail"));
    }
}
