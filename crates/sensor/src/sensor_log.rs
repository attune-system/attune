//! Per-sensor segmented logs.
//!
//! Each sensor instance gets its own stdout and stderr streams with
//! size-based artifact version rotation. Artifact-backed sensor logs are the authoritative
//! record; per-line stdout/stderr mirroring into tracing is intentionally
//! disabled to avoid duplicate ingestion.
//!
//! Sensor logs use file-backed artifact metadata, with one version per rotation
//! window and immutable segments within each version.

use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStdout;
use tracing::{info, warn};

use attune_common::artifact_transport::ArtifactFileTransport;
use attune_common::models::enums::{ArtifactClassification, RetentionPolicyType};
use attune_common::repositories::artifact::{classify_artifact, ArtifactVersionRepository};
use attune_common::repositories::log_stream::LogStreamRepository;

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
    pub max_unflushed_bytes: usize,
    pub max_unflushed_milliseconds: u64,
}

impl Default for SensorLogConfig {
    fn default() -> Self {
        Self {
            max_bytes: 10 * 1024 * 1024, // 10 MB
            max_files: 4,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 4,
            max_unflushed_bytes: 64 * 1024,
            max_unflushed_milliseconds: 500,
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
    current_size: u64,
    config: SensorLogConfig,
    versioning: SensorLogVersioning,
    writer: Option<attune_common::log_stream::SegmentedLogWriter>,
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
    pub fn new_versioned(
        transport: Arc<dyn ArtifactFileTransport>,
        sensor_ref: &str,
        stream: &str,
        config: SensorLogConfig,
        pool: sqlx::PgPool,
        target: SensorLogArtifactTarget,
    ) -> Self {
        Self {
            relative_path: format!("sensors/{}/{}.log", sensor_ref, stream),
            transport,
            current_size: 0,
            config,
            versioning: SensorLogVersioning { pool, target },
            writer: None,
        }
    }

    /// Relative path within the artifacts directory.
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    async fn ensure_writer(&mut self) -> anyhow::Result<()> {
        if self.writer.is_none() {
            self.allocate_new_version_file().await?;
        }
        Ok(())
    }

    /// Write a line to the log file, rotating if needed.
    pub async fn write_line(&mut self, line: &[u8]) -> anyhow::Result<()> {
        self.ensure_writer().await?;

        let newline_len = if line.ends_with(b"\n") { 0 } else { 1 };
        let bytes_to_write = line.len() as u64 + newline_len;

        if self.current_size > 0
            && self.current_size + bytes_to_write > self.config.max_bytes
            && self.config.max_files > 0
        {
            self.rotate().await?;
        }

        let mut bytes = Vec::with_capacity(line.len() + newline_len as usize);
        bytes.extend_from_slice(line);
        if newline_len == 1 {
            bytes.push(b'\n');
        }
        self.writer
            .as_ref()
            .expect("writer allocated")
            .write_all(&bytes)
            .await?;

        self.current_size += bytes_to_write;
        Ok(())
    }

    async fn rotate(&mut self) -> anyhow::Result<()> {
        self.seal_active().await?;
        self.allocate_new_version_file().await?;
        Ok(())
    }

    /// Flush and close the underlying file.
    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.seal_active().await
    }

    async fn allocate_new_version_file(&mut self) -> anyhow::Result<()> {
        let versioning = &self.versioning;

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

        LogStreamRepository::create(
            &versioning.pool,
            version.id,
            self.config.max_unflushed_bytes as u64,
            self.config.max_unflushed_milliseconds,
        )
        .await?;
        self.writer = Some(attune_common::log_stream::SegmentedLogWriter::new(
            self.transport.clone(),
            version.id,
            self.config.max_unflushed_bytes,
            self.config.max_unflushed_milliseconds,
        )?);
        self.relative_path = file_path;
        self.current_size = 0;
        Ok(())
    }

    async fn seal_active(&mut self) -> anyhow::Result<()> {
        let Some(writer) = self.writer.take() else {
            return Ok(());
        };
        writer.seal(false).await?;
        Ok(())
    }
}

/// Spawn a task that reads a sensor's stdout and writes to a rotating log file.
pub fn spawn_stdout_log_task(
    stdout: ChildStdout,
    sensor_ref: String,
    transport: Arc<dyn ArtifactFileTransport>,
    log_config: SensorLogConfig,
    pool: sqlx::PgPool,
    artifact_target: SensorLogArtifactTarget,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut writer = RotatingLogWriter::new_versioned(
            transport,
            &sensor_ref,
            "stdout",
            log_config,
            pool,
            artifact_target,
        );
        let mut reader = BufReader::new(stdout).lines();

        while let Ok(Some(line)) = reader.next_line().await {
            if let Err(e) = writer.write_line(line.as_bytes()).await {
                warn!("Failed to write sensor {} stdout log: {}", sensor_ref, e);
            }
        }

        if let Err(error) = writer.close().await {
            warn!("Failed to seal sensor {} stdout log: {}", sensor_ref, error);
        }
        info!("Sensor {} stdout stream closed", sensor_ref);
    })
}

/// Spawn a task that reads a sensor's stderr and writes to a rotating log file.
pub fn spawn_stderr_log_task(
    stderr: tokio::process::ChildStderr,
    sensor_ref: String,
    transport: Arc<dyn ArtifactFileTransport>,
    log_config: SensorLogConfig,
    pool: sqlx::PgPool,
    artifact_target: SensorLogArtifactTarget,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut writer = RotatingLogWriter::new_versioned(
            transport,
            &sensor_ref,
            "stderr",
            log_config,
            pool,
            artifact_target,
        );
        let mut reader = BufReader::new(stderr).lines();

        while let Ok(Some(line)) = reader.next_line().await {
            if let Err(e) = writer.write_line(line.as_bytes()).await {
                warn!("Failed to write sensor {} stderr log: {}", sensor_ref, e);
            }
        }

        if let Err(error) = writer.close().await {
            warn!("Failed to seal sensor {} stderr log: {}", sensor_ref, error);
        }
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

    #[test]
    fn sensor_log_defaults_keep_four_versions() {
        let config = SensorLogConfig::default();
        assert_eq!(config.max_files, 4);
        assert_eq!(config.retention_policy, RetentionPolicyType::Versions);
        assert_eq!(config.retention_limit, 4);
        assert_eq!(config.max_unflushed_bytes, 64 * 1024);
        assert_eq!(config.max_unflushed_milliseconds, 500);
    }

    #[test]
    fn sensor_log_retention_overrides_are_applied_independently() {
        let config = SensorLogConfig::default()
            .with_retention_overrides(Some(RetentionPolicyType::Days), Some(2));
        assert_eq!(config.max_files, 4);
        assert_eq!(config.retention_policy, RetentionPolicyType::Days);
        assert_eq!(config.retention_limit, 2);
    }
}
