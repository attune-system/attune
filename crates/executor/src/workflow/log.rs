//! Workflow activity log
//!
//! Each workflow execution's executor appends a per-execution **version** to a
//! single file-backed `FileText` artifact owned by the workflow action. The
//! artifact ref is `{action_ref}.workflow.log`, retention is per-action (50
//! versions by default), and each version is associated with the
//! `parent_execution_id` it was written for.
//!
//! As the executor orchestrates the workflow it appends timestamped lines
//! describing notable activity (workflow start, task dispatch, task
//! completion, transitions, cancellation, completion) so users can browse a
//! single readable log alongside the structured `workflow_execution` and
//! child execution records.
//!
//! Privacy: the log intentionally records *what happened*, not *what was
//! passed in or returned*. No task input or result content is written here.
//!
//! Storage: artifact + version are created lazily on first write. Version
//! files live at `{artifacts_dir}/{action_ref-as-dirs}/workflow/log/v{N}.txt`
//! and are opened with `O_APPEND` per write.
//!
//! Reliability: log writes are fire-and-forget; failures are logged at
//! `warn!` and do not propagate.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{SecondsFormat, Utc};
use sqlx::PgPool;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::OnceCell;
use tracing::warn;

use attune_common::models::{
    ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
};
use attune_common::repositories::{
    artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
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

/// Append-only logger for a single workflow execution.
///
/// Construct one per workflow advancement / dispatch entry point. The first
/// `log()` call ensures the backing artifact + per-execution version row
/// exist and resolves the on-disk path; subsequent calls reuse the cached
/// path.
#[derive(Clone)]
pub struct WorkflowLogger {
    pool: PgPool,
    artifacts_dir: PathBuf,
    action_ref: String,
    parent_execution_id: i64,
    relative_path: Arc<OnceCell<PathBuf>>,
}

impl WorkflowLogger {
    pub fn new(
        pool: PgPool,
        artifacts_dir: impl Into<PathBuf>,
        action_ref: impl Into<String>,
        parent_execution_id: i64,
    ) -> Self {
        Self {
            pool,
            artifacts_dir: artifacts_dir.into(),
            action_ref: action_ref.into(),
            parent_execution_id,
            relative_path: Arc::new(OnceCell::new()),
        }
    }

    /// Ensure the artifact + per-execution version row + parent dir all
    /// exist, returning the absolute path to the log file.
    async fn resolve_path(&self) -> Result<PathBuf> {
        let rel = self
            .relative_path
            .get_or_try_init(|| async {
                ensure_log_artifact(&self.pool, &self.action_ref, self.parent_execution_id).await
            })
            .await?;

        let full = self.artifacts_dir.join(rel);
        if let Some(parent) = full.parent() {
            let _ = attune_common::utils::create_shared_dir_all(parent).await;
        }
        Ok(full)
    }

    /// Append a single timestamped log line. Best-effort; failures are
    /// reported via `tracing::warn!` and do not propagate.
    pub async fn log(&self, level: LogLevel, message: impl AsRef<str>) {
        let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let line = format!("{ts} [{}] {}\n", level.as_str(), message.as_ref());

        let path = match self.resolve_path().await {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    "workflow log: failed to resolve log path for execution {} (action {}): {}",
                    self.parent_execution_id, self.action_ref, e
                );
                return;
            }
        };

        if let Err(e) = append_line(&path, &line).await {
            warn!(
                "workflow log: failed to append to {}: {}",
                path.display(),
                e
            );
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
}

async fn append_line(path: &std::path::Path, line: &str) -> std::io::Result<()> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    f.write_all(line.as_bytes()).await?;
    f.flush().await?;
    Ok(())
}

/// Ensure the workflow log artifact + per-execution file-backed version
/// exist, returning the relative file path for the version this execution
/// should append to.
async fn ensure_log_artifact(
    pool: &PgPool,
    action_ref: &str,
    parent_execution_id: i64,
) -> Result<PathBuf> {
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
                })),
                Some("executor".into()),
            )
            .await?
        }
    };

    let file_path = version.file_path.ok_or_else(|| {
        anyhow::anyhow!(
            "workflow log artifact {} version {} has no file_path",
            artifact.id,
            version.id
        )
    })?;
    Ok(PathBuf::from(file_path))
}
