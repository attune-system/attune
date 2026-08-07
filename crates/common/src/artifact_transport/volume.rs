//! Volume-based artifact file transport.
//!
//! Reads and writes files directly on a shared filesystem. This is the
//! fast path used when the worker/sensor and API share a mounted volume.

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::{
    ensure_checked_parent_dirs, reject_hard_linked_regular_file, resolve_checked_path,
    ArtifactFileTransport, BoxAsyncReader, BoxAsyncWriter, ValidatedRelativePath,
};
use crate::error::{Error, Result};

/// Direct filesystem transport backed by a shared volume directory.
#[derive(Debug, Clone)]
pub struct VolumeTransport {
    base_dir: PathBuf,
}

impl VolumeTransport {
    pub fn new(base_dir: &str) -> Self {
        Self {
            base_dir: PathBuf::from(base_dir),
        }
    }

    async fn resolve(&self, file_path: &str) -> Result<PathBuf> {
        let relative = ValidatedRelativePath::new(file_path)?;
        resolve_checked_path(&self.base_dir, &relative).await
    }

    /// Ensure parent directories exist with group-writable permissions.
    async fn ensure_parent(&self, file_path: &str) -> Result<PathBuf> {
        let relative = ValidatedRelativePath::new(file_path)?;
        let path = ensure_checked_parent_dirs(&self.base_dir, &relative).await?;
        if let Some(parent) = path.parent() {
            self.normalize_shared_dir_permissions(parent).await;
        }
        Ok(path)
    }

    #[cfg(unix)]
    async fn normalize_shared_dir_permissions(&self, parent: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let Ok(relative) = parent.strip_prefix(&self.base_dir) else {
            return;
        };

        let mut current = self.base_dir.clone();
        let dirs = std::iter::once(current.clone()).chain(relative.components().map(|component| {
            current.push(component.as_os_str());
            current.clone()
        }));

        for dir in dirs {
            if let Err(e) = fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o2775)).await
            {
                tracing::warn!(
                    "Failed to set shared artifact directory permissions on '{}': {}",
                    dir.display(),
                    e
                );
            }
        }
    }

    #[cfg(not(unix))]
    async fn normalize_shared_dir_permissions(&self, _parent: &Path) {}

    #[cfg(unix)]
    async fn normalize_shared_file_permissions(&self, path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        if let Err(e) = fs::set_permissions(path, std::fs::Permissions::from_mode(0o664)).await {
            tracing::warn!(
                "Failed to set shared artifact file permissions on '{}': {}",
                path.display(),
                e
            );
        }
    }

    #[cfg(not(unix))]
    async fn normalize_shared_file_permissions(&self, _path: &Path) {}
}

#[async_trait]
impl ArtifactFileTransport for VolumeTransport {
    async fn write_file(
        &self,
        file_path: &str,
        content: &[u8],
        _content_type: Option<&str>,
    ) -> Result<()> {
        let path = self.ensure_parent(file_path).await?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&path)
            .await
            .map_err(|e| Error::Io(format!("Failed to open {}: {e}", path.display())))?;
        let metadata = file
            .metadata()
            .await
            .map_err(|e| Error::Io(format!("Failed to inspect {}: {e}", path.display())))?;
        reject_hard_linked_regular_file(&path, &metadata)?;
        file.set_len(0)
            .await
            .map_err(|e| Error::Io(format!("Failed to truncate {}: {e}", path.display())))?;
        file.write_all(content)
            .await
            .map_err(|e| Error::Io(format!("Failed to write {}: {e}", path.display())))?;
        self.normalize_shared_file_permissions(&path).await;
        Ok(())
    }

    async fn read_file(&self, file_path: &str) -> Result<Vec<u8>> {
        let path = self.resolve(file_path).await?;
        let mut file = fs::File::open(&path) // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- resolve validates the relative path and rejects symlink and hard-link escapes before this open.
            .await
            .map_err(|e| Error::Io(format!("Failed to read {}: {e}", path.display())))?;
        reject_hard_linked_regular_file(
            &path,
            &file
                .metadata()
                .await
                .map_err(|e| Error::Io(format!("Failed to inspect {}: {e}", path.display())))?,
        )?;
        let mut content = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut file, &mut content)
            .await
            .map_err(|e| Error::Io(format!("Failed to read {}: {e}", path.display())))?;
        Ok(content)
    }

    async fn append_file(&self, file_path: &str, content: &[u8]) -> Result<()> {
        let path = self.ensure_parent(file_path).await?;

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| Error::Io(format!("Failed to open for append {}: {e}", path.display())))?;
        reject_hard_linked_regular_file(
            &path,
            &file
                .metadata()
                .await
                .map_err(|e| Error::Io(format!("Failed to inspect {}: {e}", path.display())))?,
        )?;
        file.write_all(content)
            .await
            .map_err(|e| Error::Io(format!("Failed to append to {}: {e}", path.display())))?;
        file.flush()
            .await
            .map_err(|e| Error::Io(format!("Failed to flush append to {}: {e}", path.display())))?;
        self.normalize_shared_file_permissions(&path).await;
        Ok(())
    }

    async fn file_exists(&self, file_path: &str) -> Result<bool> {
        let path = self.resolve(file_path).await?;
        match fs::symlink_metadata(&path).await {
            Ok(metadata) => {
                reject_hard_linked_regular_file(&path, &metadata)?;
                Ok(!metadata.file_type().is_symlink())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(Error::Io(format!(
                "Failed to inspect {}: {e}",
                path.display()
            ))),
        }
    }

    async fn file_size(&self, file_path: &str) -> Result<Option<u64>> {
        let path = self.resolve(file_path).await?;
        match fs::metadata(&path).await {
            Ok(meta) => {
                reject_hard_linked_regular_file(&path, &meta)?;
                Ok(Some(meta.len()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::Io(format!("Failed to stat {}: {e}", path.display()))),
        }
    }

    async fn delete_file(&self, file_path: &str) -> Result<()> {
        let path = self.resolve(file_path).await?;
        match fs::symlink_metadata(&path).await {
            Ok(metadata) => reject_hard_linked_regular_file(&path, &metadata)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(Error::Io(format!(
                    "Failed to inspect {}: {e}",
                    path.display()
                )))
            }
        }
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Io(format!(
                "Failed to delete {}: {e}",
                path.display()
            ))),
        }
    }

    async fn rename_file(&self, from: &str, to: &str) -> Result<()> {
        let src = self.resolve(from).await?;
        let dst = self.ensure_parent(to).await?;
        fs::rename(&src, &dst).await.map_err(|e| {
            Error::Io(format!(
                "Failed to rename {} to {}: {e}",
                src.display(),
                dst.display()
            ))
        })
    }

    async fn create_writer(&self, file_path: &str) -> Result<BoxAsyncWriter> {
        let path = self.ensure_parent(file_path).await?;
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&path)
            .await
            .map_err(|e| {
                Error::Io(format!(
                    "Failed to create writer for {}: {e}",
                    path.display()
                ))
            })?;
        reject_hard_linked_regular_file(
            &path,
            &file
                .metadata()
                .await
                .map_err(|e| Error::Io(format!("Failed to inspect {}: {e}", path.display())))?,
        )?;
        file.set_len(0)
            .await
            .map_err(|e| Error::Io(format!("Failed to truncate {}: {e}", path.display())))?;
        self.normalize_shared_file_permissions(&path).await;
        Ok(Box::pin(file))
    }

    async fn open_reader(&self, file_path: &str, offset: u64) -> Result<BoxAsyncReader> {
        let path = self.resolve(file_path).await?;
        let mut file = fs::File::open(&path) // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- resolve validates the relative path and rejects symlink and hard-link escapes before this open.
            .await
            .map_err(|e| Error::Io(format!("Failed to open reader for {}: {e}", path.display())))?;
        reject_hard_linked_regular_file(
            &path,
            &file
                .metadata()
                .await
                .map_err(|e| Error::Io(format!("Failed to inspect {}: {e}", path.display())))?,
        )?;
        if offset > 0 {
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|e| Error::Io(format!("Failed to seek in {}: {e}", path.display())))?;
        }
        Ok(Box::pin(file))
    }

    fn transport_mode(&self) -> &'static str {
        "volume"
    }

    fn base_dir(&self) -> &str {
        self.base_dir.to_str().unwrap_or("/opt/attune/artifacts")
    }

    async fn ensure_parent_dirs(&self, file_path: &str) -> Result<()> {
        self.ensure_parent(file_path).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_write_read_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let transport = VolumeTransport::new(tmp.path().to_str().unwrap());

        transport
            .write_file("test/hello.txt", b"Hello, world!", None)
            .await
            .unwrap();
        let content = transport.read_file("test/hello.txt").await.unwrap();
        assert_eq!(content, b"Hello, world!");
    }

    #[tokio::test]
    async fn test_file_exists() {
        let tmp = TempDir::new().unwrap();
        let transport = VolumeTransport::new(tmp.path().to_str().unwrap());

        assert!(!transport.file_exists("nope.txt").await.unwrap());
        transport.write_file("yes.txt", b"ok", None).await.unwrap();
        assert!(transport.file_exists("yes.txt").await.unwrap());
    }

    #[tokio::test]
    async fn test_append_file() {
        let tmp = TempDir::new().unwrap();
        let transport = VolumeTransport::new(tmp.path().to_str().unwrap());

        transport.append_file("log.txt", b"line1\n").await.unwrap();
        transport.append_file("log.txt", b"line2\n").await.unwrap();
        let content = transport.read_file("log.txt").await.unwrap();
        assert_eq!(content, b"line1\nline2\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_shared_permissions_are_api_readable() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let transport = VolumeTransport::new(tmp.path().to_str().unwrap());

        transport
            .append_file("sensor/core/timer_sensor/stdout/v1.txt", b"line\n")
            .await
            .unwrap();

        for dir in [
            tmp.path().join("sensor"),
            tmp.path().join("sensor/core"),
            tmp.path().join("sensor/core/timer_sensor"),
            tmp.path().join("sensor/core/timer_sensor/stdout"),
        ] {
            let mode = fs::metadata(&dir).await.unwrap().permissions().mode() & 0o7777;
            assert_eq!(mode, 0o2775, "unexpected mode for {}", dir.display());
        }

        let file_mode = fs::metadata(tmp.path().join("sensor/core/timer_sensor/stdout/v1.txt"))
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o664);
    }

    #[tokio::test]
    async fn test_file_size() {
        let tmp = TempDir::new().unwrap();
        let transport = VolumeTransport::new(tmp.path().to_str().unwrap());

        assert_eq!(transport.file_size("nope").await.unwrap(), None);
        transport
            .write_file("f.bin", &[0u8; 42], None)
            .await
            .unwrap();
        assert_eq!(transport.file_size("f.bin").await.unwrap(), Some(42));
    }

    #[tokio::test]
    async fn test_delete_file() {
        let tmp = TempDir::new().unwrap();
        let transport = VolumeTransport::new(tmp.path().to_str().unwrap());

        transport.write_file("rm.txt", b"bye", None).await.unwrap();
        transport.delete_file("rm.txt").await.unwrap();
        assert!(!transport.file_exists("rm.txt").await.unwrap());
        // Deleting again is OK
        transport.delete_file("rm.txt").await.unwrap();
    }

    #[tokio::test]
    async fn test_rename_file() {
        let tmp = TempDir::new().unwrap();
        let transport = VolumeTransport::new(tmp.path().to_str().unwrap());

        transport.write_file("a.txt", b"data", None).await.unwrap();
        transport.rename_file("a.txt", "sub/b.txt").await.unwrap();
        assert!(!transport.file_exists("a.txt").await.unwrap());
        let content = transport.read_file("sub/b.txt").await.unwrap();
        assert_eq!(content, b"data");
    }

    #[tokio::test]
    async fn test_all_operations_reject_unsafe_paths() {
        let tmp = TempDir::new().unwrap();
        let transport = VolumeTransport::new(tmp.path().to_str().unwrap());

        assert!(transport
            .write_file("../escape", b"bad", None)
            .await
            .is_err());
        assert!(transport.read_file("/etc/passwd").await.is_err());
        assert!(transport.append_file("a\\b", b"bad").await.is_err());
        assert!(transport.file_exists("a/../b").await.is_err());
        assert!(transport.file_size("C:/escape").await.is_err());
        assert!(transport.delete_file("../escape").await.is_err());
        assert!(transport.rename_file("safe", "../escape").await.is_err());
        assert!(transport.create_writer("../escape").await.is_err());
        assert!(transport.open_reader("../escape", 0).await.is_err());
        assert!(transport.ensure_parent_dirs("../escape").await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_existing_symlink_component_is_rejected() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), tmp.path().join("link")).unwrap();
        let transport = VolumeTransport::new(tmp.path().to_str().unwrap());

        assert!(transport
            .write_file("link/escape.txt", b"bad", None)
            .await
            .is_err());
        assert!(!outside.path().join("escape.txt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_all_file_operations_reject_existing_hard_links() {
        let tmp = TempDir::new().unwrap();
        let transport = VolumeTransport::new(tmp.path().to_str().unwrap());
        let original = tmp.path().join("shared.txt");
        fs::write(&original, b"original").await.unwrap();
        std::fs::hard_link(&original, tmp.path().join("alias.txt")).unwrap();

        assert!(transport
            .write_file("shared.txt", b"bad", None)
            .await
            .is_err());
        assert!(transport.read_file("shared.txt").await.is_err());
        assert!(transport.append_file("shared.txt", b"bad").await.is_err());
        assert!(transport.file_exists("shared.txt").await.is_err());
        assert!(transport.file_size("shared.txt").await.is_err());
        assert!(transport.delete_file("shared.txt").await.is_err());
        assert!(transport
            .rename_file("shared.txt", "renamed.txt")
            .await
            .is_err());
        assert!(transport.create_writer("shared.txt").await.is_err());
        assert!(transport.open_reader("shared.txt", 0).await.is_err());
        assert_eq!(fs::read(&original).await.unwrap(), b"original");
        assert!(tmp.path().join("alias.txt").exists());
    }
}
