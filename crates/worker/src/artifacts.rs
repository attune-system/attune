//! Artifacts Module
//!
//! Handles storage and retrieval of execution artifacts (logs, outputs, results).

use attune_common::{
    artifact_transport::{
        ensure_checked_parent_dirs, reject_hard_linked_regular_file, resolve_checked_path,
        ValidatedRelativePath,
    },
    error::{Error, Result},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

/// Artifact type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactType {
    /// Execution logs (stdout/stderr)
    Log,
    /// Execution result data
    Result,
    /// Custom file output
    File,
    /// Trace/debug information
    Trace,
}

/// Artifact metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Artifact ID
    pub id: String,
    /// Execution ID
    pub execution_id: i64,
    /// Artifact type
    pub artifact_type: ArtifactType,
    /// File path
    pub path: PathBuf,
    /// Content type (MIME type)
    pub content_type: String,
    /// Size in bytes
    pub size: u64,
    /// Creation timestamp
    pub created: chrono::DateTime<chrono::Utc>,
}

/// Artifact manager for storing execution artifacts
pub struct ArtifactManager {
    /// Base directory for artifact storage
    base_dir: PathBuf,
}

impl ArtifactManager {
    /// Create a new artifact manager
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Initialize the artifact storage directory
    pub async fn initialize(&self) -> Result<()> {
        attune_common::utils::create_shared_dir_all(&self.base_dir)
            .await
            .map_err(|e| Error::Internal(format!("Failed to create artifact directory: {}", e)))?;

        info!("Artifact storage initialized at: {:?}", self.base_dir);
        Ok(())
    }

    /// Get the directory path for an execution
    pub fn get_execution_dir(&self, execution_id: i64) -> PathBuf {
        self.base_dir.join(format!("execution_{}", execution_id))
    }

    async fn checked_execution_file(&self, execution_id: i64, filename: &str) -> Result<PathBuf> {
        let relative = ValidatedRelativePath::new(&format!("execution_{execution_id}/{filename}"))?;
        ensure_checked_parent_dirs(&self.base_dir, &relative).await
    }

    async fn create_checked_file(&self, path: &std::path::Path) -> Result<fs::File> {
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to open artifact file: {e}")))?;
        let metadata = file
            .metadata()
            .await
            .map_err(|e| Error::Internal(format!("Failed to inspect artifact file: {e}")))?;
        reject_hard_linked_regular_file(path, &metadata)?;
        file.set_len(0)
            .await
            .map_err(|e| Error::Internal(format!("Failed to truncate artifact file: {e}")))?;
        Ok(file)
    }

    async fn reject_hard_links_below(&self, root: &std::path::Path) -> Result<()> {
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            let mut entries = fs::read_dir(&dir).await.map_err(|e| {
                Error::Internal(format!("Failed to inspect artifact directory: {e}"))
            })?;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| Error::Internal(format!("Failed to inspect artifact entry: {e}")))?
            {
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).await.map_err(|e| {
                    Error::Internal(format!("Failed to inspect artifact path: {e}"))
                })?;
                if metadata.file_type().is_symlink() {
                    return Err(Error::PermissionDenied(format!(
                        "Artifact tree contains a symbolic link: '{}'",
                        path.display()
                    )));
                }
                reject_hard_linked_regular_file(&path, &metadata)?;
                if metadata.is_dir() {
                    pending.push(path);
                }
            }
        }
        Ok(())
    }

    /// Store execution logs
    pub async fn store_logs(
        &self,
        execution_id: i64,
        stdout: &str,
        stderr: &str,
    ) -> Result<Vec<Artifact>> {
        let mut artifacts = Vec::new();

        // Store stdout
        if !stdout.is_empty() {
            let stdout_path = self
                .checked_execution_file(execution_id, "stdout.log")
                .await?;
            let mut file = self.create_checked_file(&stdout_path).await?;
            file.write_all(stdout.as_bytes())
                .await
                .map_err(|e| Error::Internal(format!("Failed to write stdout: {}", e)))?;
            file.sync_all()
                .await
                .map_err(|e| Error::Internal(format!("Failed to sync stdout file: {}", e)))?;

            let metadata = fs::metadata(&stdout_path)
                .await
                .map_err(|e| Error::Internal(format!("Failed to get stdout metadata: {}", e)))?;
            artifacts.push(Artifact {
                id: format!("{}_stdout", execution_id),
                execution_id,
                artifact_type: ArtifactType::Log,
                path: stdout_path,
                content_type: "text/plain".to_string(),
                size: metadata.len(),
                created: chrono::Utc::now(),
            });

            debug!(
                "Stored stdout log for execution {} ({} bytes)",
                execution_id,
                metadata.len()
            );
        }

        // Store stderr
        if !stderr.is_empty() {
            let stderr_path = self
                .checked_execution_file(execution_id, "stderr.log")
                .await?;
            let mut file = self.create_checked_file(&stderr_path).await?;
            file.write_all(stderr.as_bytes())
                .await
                .map_err(|e| Error::Internal(format!("Failed to write stderr: {}", e)))?;
            file.sync_all()
                .await
                .map_err(|e| Error::Internal(format!("Failed to sync stderr file: {}", e)))?;

            let metadata = fs::metadata(&stderr_path)
                .await
                .map_err(|e| Error::Internal(format!("Failed to get stderr metadata: {}", e)))?;
            artifacts.push(Artifact {
                id: format!("{}_stderr", execution_id),
                execution_id,
                artifact_type: ArtifactType::Log,
                path: stderr_path,
                content_type: "text/plain".to_string(),
                size: metadata.len(),
                created: chrono::Utc::now(),
            });

            debug!(
                "Stored stderr log for execution {} ({} bytes)",
                execution_id,
                metadata.len()
            );
        }

        Ok(artifacts)
    }

    /// Store execution result
    pub async fn store_result(
        &self,
        execution_id: i64,
        result: &serde_json::Value,
    ) -> Result<Artifact> {
        let result_path = self
            .checked_execution_file(execution_id, "result.json")
            .await?;
        let result_json = serde_json::to_string_pretty(result)?;

        let mut file = self.create_checked_file(&result_path).await?;
        file.write_all(result_json.as_bytes())
            .await
            .map_err(|e| Error::Internal(format!("Failed to write result: {}", e)))?;
        file.sync_all()
            .await
            .map_err(|e| Error::Internal(format!("Failed to sync result file: {}", e)))?;

        let metadata = fs::metadata(&result_path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to get result metadata: {}", e)))?;

        debug!(
            "Stored result for execution {} ({} bytes)",
            execution_id,
            metadata.len()
        );

        Ok(Artifact {
            id: format!("{}_result", execution_id),
            execution_id,
            artifact_type: ArtifactType::Result,
            path: result_path,
            content_type: "application/json".to_string(),
            size: metadata.len(),
            created: chrono::Utc::now(),
        })
    }

    /// Read an artifact
    pub async fn read_artifact(&self, artifact: &Artifact) -> Result<Vec<u8>> {
        let relative = artifact.path.strip_prefix(&self.base_dir).map_err(|_| {
            Error::PermissionDenied("Artifact path is outside the configured root".to_string())
        })?;
        let relative =
            ValidatedRelativePath::new(relative.to_str().ok_or_else(|| {
                Error::Validation("Artifact path is not valid UTF-8".to_string())
            })?)?;
        let path = resolve_checked_path(&self.base_dir, &relative).await?;
        let file = fs::File::open(&path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read artifact: {e}")))?;
        reject_hard_linked_regular_file(
            &path,
            &file
                .metadata()
                .await
                .map_err(|e| Error::Internal(format!("Failed to inspect artifact: {e}")))?,
        )?;
        let mut file = file;
        let mut content = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut file, &mut content)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read artifact: {e}")))?;
        Ok(content)
    }

    /// Delete artifacts for an execution
    pub async fn delete_execution_artifacts(&self, execution_id: i64) -> Result<()> {
        let relative = ValidatedRelativePath::new(&format!("execution_{execution_id}"))?;
        let exec_dir = resolve_checked_path(&self.base_dir, &relative).await?;

        if exec_dir.exists() {
            self.reject_hard_links_below(&exec_dir).await?;
            fs::remove_dir_all(&exec_dir).await.map_err(|e| {
                Error::Internal(format!("Failed to delete execution artifacts: {}", e))
            })?;

            info!("Deleted artifacts for execution {}", execution_id);
        } else {
            warn!(
                "No artifacts found for execution {} (directory does not exist)",
                execution_id
            );
        }

        Ok(())
    }

    /// Clean up old artifacts (retention policy)
    pub async fn cleanup_old_artifacts(&self, retention_days: u64) -> Result<usize> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days as i64);
        let mut deleted_count = 0;

        let mut entries = fs::read_dir(&self.base_dir)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read artifact directory: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| Error::Internal(format!("Failed to read directory entry: {}", e)))?
        {
            let path = entry.path();
            if let Ok(entry_metadata) = fs::symlink_metadata(&path).await {
                if entry_metadata.is_dir() && !entry_metadata.file_type().is_symlink() {
                    if let Ok(metadata) = fs::metadata(&path).await {
                        if let Ok(modified) = metadata.modified() {
                            let modified_time: chrono::DateTime<chrono::Utc> = modified.into();
                            if modified_time < cutoff {
                                if let Err(e) = self.reject_hard_links_below(&path).await {
                                    warn!(
                                        "Refusing to delete unsafe old artifact directory {:?}: {}",
                                        path, e
                                    );
                                    continue;
                                }
                                if let Err(e) = fs::remove_dir_all(&path).await {
                                    warn!(
                                        "Failed to delete old artifact directory {:?}: {}",
                                        path, e
                                    );
                                } else {
                                    deleted_count += 1;
                                    debug!("Deleted old artifact directory: {:?}", path);
                                }
                            }
                        }
                    }
                }
            }
        }

        info!(
            "Cleaned up {} old artifact directories (retention: {} days)",
            deleted_count, retention_days
        );

        Ok(deleted_count)
    }
}

impl Default for ArtifactManager {
    fn default() -> Self {
        Self::new(PathBuf::from("/tmp/attune/artifacts"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_artifact_manager_store_logs() {
        let temp_dir = TempDir::new().unwrap();
        let manager = ArtifactManager::new(temp_dir.path().to_path_buf());
        manager.initialize().await.unwrap();

        let artifacts = manager
            .store_logs(1, "stdout output", "stderr output")
            .await
            .unwrap();

        assert_eq!(artifacts.len(), 2);
    }

    #[tokio::test]
    async fn test_artifact_manager_store_result() {
        let temp_dir = TempDir::new().unwrap();
        let manager = ArtifactManager::new(temp_dir.path().to_path_buf());
        manager.initialize().await.unwrap();

        let result = serde_json::json!({"status": "success", "value": 42});
        let artifact = manager.store_result(1, &result).await.unwrap();

        assert_eq!(artifact.execution_id, 1);
        assert_eq!(artifact.content_type, "application/json");
    }

    #[tokio::test]
    async fn test_artifact_manager_delete() {
        let temp_dir = TempDir::new().unwrap();
        let manager = ArtifactManager::new(temp_dir.path().to_path_buf());
        manager.initialize().await.unwrap();

        manager.store_logs(1, "test", "test").await.unwrap();
        assert!(manager.get_execution_dir(1).exists());

        manager.delete_execution_artifacts(1).await.unwrap();
        assert!(!manager.get_execution_dir(1).exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_artifact_manager_rejects_execution_directory_symlink() {
        use std::os::unix::fs::symlink;

        let temp_dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let manager = ArtifactManager::new(temp_dir.path().to_path_buf());
        manager.initialize().await.unwrap();
        symlink(outside.path(), manager.get_execution_dir(1)).unwrap();

        assert!(manager
            .store_result(1, &serde_json::json!({"secret": true}))
            .await
            .is_err());
        assert!(!outside.path().join("result.json").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_artifact_manager_rejects_hard_links_for_write_read_and_delete() {
        let temp_dir = TempDir::new().unwrap();
        let manager = ArtifactManager::new(temp_dir.path().to_path_buf());
        manager.initialize().await.unwrap();
        manager
            .store_result(1, &serde_json::json!({"safe": true}))
            .await
            .unwrap();

        let result_path = manager.get_execution_dir(1).join("result.json");
        std::fs::hard_link(&result_path, temp_dir.path().join("alias.json")).unwrap();
        let artifact = Artifact {
            id: "1_result".to_string(),
            execution_id: 1,
            artifact_type: ArtifactType::Result,
            path: result_path.clone(),
            content_type: "application/json".to_string(),
            size: 0,
            created: chrono::Utc::now(),
        };

        assert!(manager.read_artifact(&artifact).await.is_err());
        assert!(manager
            .store_result(1, &serde_json::json!({"bad": true}))
            .await
            .is_err());
        assert!(manager.delete_execution_artifacts(1).await.is_err());
        assert!(result_path.exists());
    }
}
