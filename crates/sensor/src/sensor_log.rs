//! Per-sensor rotating log files.
//!
//! Each sensor instance gets its own stdout and stderr log files with
//! size-based rotation. Artifact-backed sensor logs are the authoritative
//! record; per-line stdout/stderr mirroring into tracing is intentionally
//! disabled to avoid duplicate ingestion.
//!
//! Sensor logs normally use file-backed artifact versions, with one version per
//! active/rotated segment. A legacy raw-file layout under
//! `{artifacts_dir}/sensors/{sensor_ref}/` is still supported as a fallback when
//! artifact registration is unavailable.

use std::collections::HashSet;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStdout;
use tracing::{info, warn};

use attune_common::artifact_transport::ArtifactFileTransport;
use attune_common::models::enums::{ArtifactClassification, RetentionPolicyType};
use attune_common::repositories::artifact::{
    classify_artifact, ArtifactRepository, ArtifactVersionRepository,
};

/// Configuration for sensor log rotation.
#[derive(Debug, Clone)]
pub struct SensorLogConfig {
    /// Maximum bytes per log file before rotation.
    pub max_bytes: u64,
    /// Number of legacy raw rotated files to keep when artifact versioning is unavailable.
    pub max_files: u32,
    /// Retention policy for registered sensor log artifact versions.
    pub retention_policy: RetentionPolicyType,
    /// Retention limit for registered sensor log artifact versions.
    pub retention_limit: i32,
}

impl Default for SensorLogConfig {
    fn default() -> Self {
        Self {
            max_bytes: 10 * 1024 * 1024, // 10 MB
            max_files: 4,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 4,
        }
    }
}

impl SensorLogConfig {
    pub fn with_retention_overrides(
        &self,
        retention_policy: Option<RetentionPolicyType>,
        retention_limit: Option<i32>,
    ) -> Self {
        Self {
            retention_policy: retention_policy.unwrap_or(self.retention_policy),
            retention_limit: retention_limit.unwrap_or(self.retention_limit),
            ..self.clone()
        }
    }
}

/// A rotating log file writer for a single sensor stream (stdout or stderr).
///
/// Writes through the artifact file transport and rotates when the current
/// file exceeds `max_bytes`.
pub struct RotatingLogWriter {
    /// Relative path within artifacts_dir (e.g., `sensors/core.timer/stdout.log`).
    relative_path: String,
    /// File transport used to persist log bytes.
    transport: Arc<dyn ArtifactFileTransport>,
    /// Current file size in bytes.
    current_size: u64,
    /// Whether `current_size` has been initialized from the transport.
    size_initialized: bool,
    /// Rotation config.
    config: SensorLogConfig,
    /// Optional artifact version target. When present, each rotated segment is
    /// stored as a file-backed artifact version instead of a legacy `.N` file.
    versioning: Option<SensorLogVersioning>,
    active_version_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct SensorLogArtifactTarget {
    artifact_id: i64,
    artifact_ref: String,
    sensor_ref: String,
    stream: String,
}

#[derive(Debug, Clone)]
pub struct SensorLogArtifacts {
    pub stdout: SensorLogArtifactTarget,
    pub stderr: SensorLogArtifactTarget,
}

#[derive(Debug, Clone)]
struct SensorLogVersioning {
    pool: sqlx::PgPool,
    target: SensorLogArtifactTarget,
}

impl RotatingLogWriter {
    pub fn new(
        transport: Arc<dyn ArtifactFileTransport>,
        sensor_ref: &str,
        stream: &str,
        config: SensorLogConfig,
    ) -> Self {
        let relative_path = format!("sensors/{}/{}.log", sensor_ref, stream);
        Self {
            relative_path,
            transport,
            current_size: 0,
            size_initialized: false,
            config,
            versioning: None,
            active_version_id: None,
        }
    }

    pub fn new_versioned(
        transport: Arc<dyn ArtifactFileTransport>,
        sensor_ref: &str,
        stream: &str,
        config: SensorLogConfig,
        pool: sqlx::PgPool,
        target: SensorLogArtifactTarget,
    ) -> Self {
        let mut writer = Self::new(transport, sensor_ref, stream, config);
        writer.versioning = Some(SensorLogVersioning { pool, target });
        writer
    }

    /// Relative path within the artifacts directory.
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    async fn ensure_current_size(&mut self) -> anyhow::Result<()> {
        if !self.size_initialized {
            if self.versioning.is_some() && self.active_version_id.is_none() {
                self.allocate_new_version_file().await?;
            }
            self.current_size = self
                .transport
                .file_size(&self.relative_path)
                .await?
                .unwrap_or(0);
            self.size_initialized = true;
        }
        Ok(())
    }

    /// Write a line to the log file, rotating if needed.
    pub async fn write_line(&mut self, line: &[u8]) -> anyhow::Result<()> {
        self.ensure_current_size().await?;

        let newline_len = if line.ends_with(b"\n") { 0 } else { 1 };
        let bytes_to_write = line.len() as u64 + newline_len;

        if self.current_size > 0
            && self.current_size + bytes_to_write > self.config.max_bytes
            && self.config.max_files > 0
        {
            self.rotate().await?;
        }

        if line.ends_with(b"\n") {
            self.transport
                .append_file(&self.relative_path, line)
                .await?;
        } else {
            let mut bytes = Vec::with_capacity(line.len() + 1);
            bytes.extend_from_slice(line);
            bytes.push(b'\n');
            self.transport
                .append_file(&self.relative_path, &bytes)
                .await?;
        }

        self.current_size += bytes_to_write;
        Ok(())
    }

    async fn rotate(&mut self) -> anyhow::Result<()> {
        if self.versioning.is_some() {
            self.finalize_active_version_size().await?;
            self.allocate_new_version_file().await?;
            return Ok(());
        }

        // Shift existing rotated files: .N -> .N+1, deleting the oldest.
        for i in (1..=self.config.max_files).rev() {
            let from = format!("{}.{}", self.relative_path, i);
            if i == self.config.max_files {
                if let Err(e) = self.transport.delete_file(&from).await {
                    warn!("Failed to delete old sensor log '{}': {}", from, e);
                }
                continue;
            }

            if self.transport.file_exists(&from).await.unwrap_or(false) {
                let to = format!("{}.{}", self.relative_path, i + 1);
                if let Err(e) = self.transport.rename_file(&from, &to).await {
                    warn!("Failed to rotate sensor log '{}' to '{}': {}", from, to, e);
                }
            }
        }

        // Current -> .1
        if self
            .transport
            .file_exists(&self.relative_path)
            .await
            .unwrap_or(false)
        {
            let first_rotated = format!("{}.1", self.relative_path);
            if let Err(e) = self
                .transport
                .rename_file(&self.relative_path, &first_rotated)
                .await
            {
                warn!(
                    "Failed to rotate sensor log '{}' to '{}': {}",
                    self.relative_path, first_rotated, e
                );
            }
        }

        self.current_size = 0;
        Ok(())
    }

    /// Flush and close the underlying file.
    pub async fn close(&mut self) {
        if let Err(e) = self.finalize_active_version_size().await {
            warn!(
                "Failed to finalize sensor log artifact version '{}': {}",
                self.relative_path, e
            );
        }
    }

    async fn allocate_new_version_file(&mut self) -> anyhow::Result<()> {
        let Some(versioning) = self.versioning.as_ref() else {
            return Ok(());
        };

        let before_versions = ArtifactVersionRepository::find_file_versions_by_artifact(
            &versioning.pool,
            versioning.target.artifact_id,
        )
        .await?;

        let version = ArtifactVersionRepository::create_file_backed(
            &versioning.pool,
            versioning.target.artifact_id,
            &versioning.target.artifact_ref,
            "text/plain".to_string(),
            None,
            Some(serde_json::json!({
                "sensor_ref": versioning.target.sensor_ref,
                "stream": versioning.target.stream,
            })),
            Some("sensor".to_string()),
        )
        .await?;
        info!(
            artifact_id = versioning.target.artifact_id,
            artifact_ref = %versioning.target.artifact_ref,
            version_id = version.id,
            sensor_ref = %versioning.target.sensor_ref,
            stream = %versioning.target.stream,
            "Allocated sensor runtime log artifact version"
        );

        let file_path = version
            .file_path
            .ok_or_else(|| anyhow::anyhow!("Allocated sensor log version has no file_path"))?;

        self.transport.ensure_parent_dirs(&file_path).await?;

        let after_versions = ArtifactVersionRepository::find_file_versions_by_artifact(
            &versioning.pool,
            versioning.target.artifact_id,
        )
        .await?;
        let retained_paths: HashSet<String> = after_versions
            .iter()
            .filter_map(|version| version.file_path.clone())
            .collect();

        for stale_path in before_versions
            .into_iter()
            .filter_map(|version| version.file_path)
            .filter(|path| !retained_paths.contains(path))
        {
            if let Err(e) = self.transport.delete_file(&stale_path).await {
                warn!(
                    "Failed to delete stale retained sensor log file '{}': {}",
                    stale_path, e
                );
            }
        }

        self.relative_path = file_path;
        self.active_version_id = Some(version.id);
        self.current_size = 0;
        self.size_initialized = true;
        Ok(())
    }

    async fn finalize_active_version_size(&self) -> anyhow::Result<()> {
        let (Some(versioning), Some(version_id)) =
            (self.versioning.as_ref(), self.active_version_id)
        else {
            return Ok(());
        };

        let Some(size_bytes) = self.transport.file_size(&self.relative_path).await? else {
            return Ok(());
        };
        let size_bytes = size_bytes as i64;

        ArtifactVersionRepository::update_size_bytes(&versioning.pool, version_id, size_bytes)
            .await?;
        ArtifactRepository::update_size_bytes(
            &versioning.pool,
            versioning.target.artifact_id,
            size_bytes,
        )
        .await?;
        info!(
            artifact_id = versioning.target.artifact_id,
            artifact_ref = %versioning.target.artifact_ref,
            version_id,
            sensor_ref = %versioning.target.sensor_ref,
            stream = %versioning.target.stream,
            size_bytes,
            "Finalized sensor runtime log artifact version"
        );
        Ok(())
    }
}

/// Spawn a task that reads a sensor's stdout and writes to a rotating log file.
pub fn spawn_stdout_log_task(
    stdout: ChildStdout,
    sensor_ref: String,
    transport: Arc<dyn ArtifactFileTransport>,
    log_config: SensorLogConfig,
    pool: Option<sqlx::PgPool>,
    artifact_target: Option<SensorLogArtifactTarget>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut writer = match (pool, artifact_target) {
            (Some(pool), Some(target)) => RotatingLogWriter::new_versioned(
                transport,
                &sensor_ref,
                "stdout",
                log_config,
                pool,
                target,
            ),
            _ => RotatingLogWriter::new(transport, &sensor_ref, "stdout", log_config),
        };
        let mut reader = BufReader::new(stdout).lines();

        while let Ok(Some(line)) = reader.next_line().await {
            if let Err(e) = writer.write_line(line.as_bytes()).await {
                warn!("Failed to write sensor {} stdout log: {}", sensor_ref, e);
            }
        }

        writer.close().await;
        info!("Sensor {} stdout stream closed", sensor_ref);
    })
}

/// Spawn a task that reads a sensor's stderr and writes to a rotating log file.
pub fn spawn_stderr_log_task(
    stderr: tokio::process::ChildStderr,
    sensor_ref: String,
    transport: Arc<dyn ArtifactFileTransport>,
    log_config: SensorLogConfig,
    pool: Option<sqlx::PgPool>,
    artifact_target: Option<SensorLogArtifactTarget>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut writer = match (pool, artifact_target) {
            (Some(pool), Some(target)) => RotatingLogWriter::new_versioned(
                transport,
                &sensor_ref,
                "stderr",
                log_config,
                pool,
                target,
            ),
            _ => RotatingLogWriter::new(transport, &sensor_ref, "stderr", log_config),
        };
        let mut reader = BufReader::new(stderr).lines();

        while let Ok(Some(line)) = reader.next_line().await {
            if let Err(e) = writer.write_line(line.as_bytes()).await {
                warn!("Failed to write sensor {} stderr log: {}", sensor_ref, e);
            }
        }

        writer.close().await;
        info!("Sensor {} stderr stream closed", sensor_ref);
    })
}

/// Register sensor log artifacts in the database so they are discoverable
/// via the standard artifact API.
pub async fn register_sensor_log_artifacts(
    pool: &sqlx::PgPool,
    sensor_ref: &str,
    _transport: &dyn ArtifactFileTransport,
    log_config: &SensorLogConfig,
) -> anyhow::Result<SensorLogArtifacts> {
    use attune_common::models::enums::{ArtifactType, ArtifactVisibility, OwnerType};
    use attune_common::repositories::artifact::{
        ArtifactRepository, CreateArtifactInput, UpdateArtifactInput,
    };
    use attune_common::repositories::{Create, FindByRef, Update};

    let mut stdout = None;
    let mut stderr = None;

    for stream in &["stdout", "stderr"] {
        let artifact_ref = format!("sensor.{}.{}", sensor_ref, stream);
        let artifact = if let Some(existing) =
            attune_common::repositories::artifact::ArtifactRepository::find_by_ref(
                pool,
                &artifact_ref,
            )
            .await?
        {
            if existing.retention_policy != log_config.retention_policy
                || existing.retention_limit != log_config.retention_limit
                || existing.classification
                    != classify_artifact(&artifact_ref, ArtifactType::FileText)
                || existing.visibility != ArtifactVisibility::Private
            {
                ArtifactRepository::update(
                    pool,
                    existing.id,
                    UpdateArtifactInput {
                        visibility: Some(ArtifactVisibility::Private),
                        classification: Some(classify_artifact(
                            &artifact_ref,
                            ArtifactType::FileText,
                        )),
                        retention_policy: Some(log_config.retention_policy),
                        retention_limit: Some(log_config.retention_limit),
                        ..Default::default()
                    },
                )
                .await?;
            }
            existing
        } else {
            ArtifactRepository::create(
                pool,
                CreateArtifactInput {
                    r#ref: artifact_ref.clone(),
                    scope: OwnerType::Sensor,
                    owner: sensor_ref.to_string(),
                    r#type: ArtifactType::FileText,
                    visibility: ArtifactVisibility::Private,
                    classification: ArtifactClassification::RuntimeLog,
                    retention_policy: log_config.retention_policy,
                    retention_limit: log_config.retention_limit,
                    name: Some(format!("{} sensor {} log", sensor_ref, stream)),
                    description: Some(format!(
                        "Rotating {} log for sensor '{}'",
                        stream, sensor_ref
                    )),
                    content_type: Some("text/plain".to_string()),
                    data: None,
                },
            )
            .await?
        };

        let target = SensorLogArtifactTarget {
            artifact_id: artifact.id,
            artifact_ref,
            sensor_ref: sensor_ref.to_string(),
            stream: stream.to_string(),
        };
        match *stream {
            "stdout" => stdout = Some(target),
            "stderr" => stderr = Some(target),
            _ => {}
        }
    }

    Ok(SensorLogArtifacts {
        stdout: stdout.ok_or_else(|| anyhow::anyhow!("stdout log artifact target missing"))?,
        stderr: stderr.ok_or_else(|| anyhow::anyhow!("stderr log artifact target missing"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio::process::Command;
    use tracing::{field::Field, field::Visit, Event, Subscriber};
    use tracing_subscriber::layer::{Context, Layer};
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::registry::LookupSpan;

    #[test]
    fn sensor_log_defaults_keep_four_versions() {
        let config = SensorLogConfig::default();
        assert_eq!(config.max_files, 4);
        assert_eq!(config.retention_policy, RetentionPolicyType::Versions);
        assert_eq!(config.retention_limit, 4);
    }

    #[test]
    fn sensor_log_retention_overrides_are_applied_independently() {
        let config = SensorLogConfig::default()
            .with_retention_overrides(Some(RetentionPolicyType::Days), Some(2));
        assert_eq!(config.max_files, 4);
        assert_eq!(config.retention_policy, RetentionPolicyType::Days);
        assert_eq!(config.retention_limit, 2);
    }

    fn volume_transport(tmp: &TempDir) -> Arc<dyn ArtifactFileTransport> {
        Arc::new(attune_common::artifact_transport::VolumeTransport::new(
            tmp.path().to_str().unwrap(),
        ))
    }

    #[derive(Clone, Default)]
    struct CapturedEvents(Arc<Mutex<Vec<String>>>);

    impl CapturedEvents {
        fn contains(&self, needle: &str) -> bool {
            self.0
                .lock()
                .unwrap()
                .iter()
                .any(|event| event.contains(needle))
        }
    }

    #[derive(Clone, Default)]
    struct CaptureLayer {
        events: CapturedEvents,
    }

    impl<S> Layer<S> for CaptureLayer
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            let mut visitor = MessageVisitor::default();
            event.record(&mut visitor);
            self.events
                .0
                .lock()
                .unwrap()
                .push(visitor.message.unwrap_or_default());
        }
    }

    #[derive(Default)]
    struct MessageVisitor {
        message: Option<String>,
    }

    impl Visit for MessageVisitor {
        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "message" {
                self.message = Some(value.to_string());
            }
        }

        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            if field.name() == "message" {
                self.message = Some(format!("{value:?}"));
            }
        }
    }

    #[tokio::test]
    async fn test_rotating_log_writer_basic_write() {
        let tmp = TempDir::new().unwrap();
        let config = SensorLogConfig {
            max_bytes: 1024,
            max_files: 3,
            ..Default::default()
        };
        let mut writer =
            RotatingLogWriter::new(volume_transport(&tmp), "test.sensor", "stdout", config);

        writer.write_line(b"hello world").await.unwrap();
        writer.close().await;

        let content = tokio::fs::read_to_string(tmp.path().join("sensors/test.sensor/stdout.log"))
            .await
            .unwrap();
        assert!(content.contains("hello world"));
    }

    #[tokio::test]
    async fn test_rotating_log_writer_rotation() {
        let tmp = TempDir::new().unwrap();
        let config = SensorLogConfig {
            max_bytes: 50, // Very small to trigger rotation
            max_files: 3,
            ..Default::default()
        };
        let mut writer =
            RotatingLogWriter::new(volume_transport(&tmp), "test.sensor", "stdout", config);

        // Write enough to trigger rotation
        for i in 0..10 {
            writer
                .write_line(format!("line number {}", i).as_bytes())
                .await
                .unwrap();
        }
        writer.close().await;

        // Check that rotated files exist
        let base = tmp.path().join("sensors/test.sensor/stdout.log");
        assert!(base.exists());
        let rotated_1 = PathBuf::from(format!("{}.1", base.display()));
        assert!(rotated_1.exists());
    }

    #[tokio::test]
    async fn test_rotating_log_writer_max_files_enforced() {
        let tmp = TempDir::new().unwrap();
        let config = SensorLogConfig {
            max_bytes: 30,
            max_files: 2,
            ..Default::default()
        };
        let mut writer =
            RotatingLogWriter::new(volume_transport(&tmp), "test.sensor", "stderr", config);

        // Write enough lines to rotate multiple times
        for i in 0..20 {
            writer
                .write_line(format!("error line {}", i).as_bytes())
                .await
                .unwrap();
        }
        writer.close().await;

        let base = tmp.path().join("sensors/test.sensor/stderr.log");
        assert!(base.exists());

        // .1 and .2 should exist, .3 should not (max_files=2)
        let rotated_1 = PathBuf::from(format!("{}.1", base.display()));
        let rotated_2 = PathBuf::from(format!("{}.2", base.display()));
        let rotated_3 = PathBuf::from(format!("{}.3", base.display()));
        assert!(rotated_1.exists());
        assert!(rotated_2.exists());
        assert!(!rotated_3.exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sensor_log_stream_lines_are_not_reemitted_to_tracing() {
        let tmp = TempDir::new().unwrap();
        let events = CapturedEvents::default();
        let subscriber = tracing_subscriber::registry().with(CaptureLayer {
            events: events.clone(),
        });
        let _guard = tracing::subscriber::set_default(subscriber);

        let mut child = Command::new("sh")
            .arg("-c")
            .arg("printf 'stdout line\\n'; printf 'stderr line\\n' >&2")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let transport = volume_transport(&tmp);

        let stdout_handle = spawn_stdout_log_task(
            stdout,
            "test.sensor".to_string(),
            transport.clone(),
            SensorLogConfig::default(),
            None,
            None,
        );
        let stderr_handle = spawn_stderr_log_task(
            stderr,
            "test.sensor".to_string(),
            transport,
            SensorLogConfig::default(),
            None,
            None,
        );

        child.wait().await.unwrap();
        stdout_handle.await.unwrap();
        stderr_handle.await.unwrap();

        let stdout_content =
            tokio::fs::read_to_string(tmp.path().join("sensors/test.sensor/stdout.log"))
                .await
                .unwrap();
        let stderr_content =
            tokio::fs::read_to_string(tmp.path().join("sensors/test.sensor/stderr.log"))
                .await
                .unwrap();

        assert!(stdout_content.contains("stdout line"));
        assert!(stderr_content.contains("stderr line"));
        assert!(!events.contains("Sensor test.sensor stdout: stdout line"));
        assert!(!events.contains("Sensor test.sensor stderr: stderr line"));
        assert!(events.contains("Sensor test.sensor stdout stream closed"));
        assert!(events.contains("Sensor test.sensor stderr stream closed"));
    }
}
