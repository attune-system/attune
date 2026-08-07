use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

/// A non-empty, unambiguous relative path below the artifact storage root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedRelativePath(String);

impl ValidatedRelativePath {
    pub fn new(value: &str) -> Result<Self> {
        if value.is_empty() {
            return Err(invalid_path("path must not be empty"));
        }
        if value.contains('\0') {
            return Err(invalid_path("NUL bytes are not allowed"));
        }
        if value.contains('\\') {
            return Err(invalid_path("backslashes are not allowed"));
        }
        // Colons can denote Windows drive prefixes or alternate data streams.
        if value.contains(':') {
            return Err(invalid_path("colons are not allowed"));
        }

        let path = Path::new(value); // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- This constructor parses the input so it can reject absolute, prefixed, parent, current-directory, and ambiguous components.
        if path.is_absolute() {
            return Err(invalid_path("absolute paths are not allowed"));
        }

        for component in path.components() {
            match component {
                Component::Normal(_) => {}
                Component::Prefix(_) | Component::RootDir => {
                    return Err(invalid_path("rooted or prefixed paths are not allowed"));
                }
                Component::ParentDir => {
                    return Err(invalid_path("parent path components are not allowed"));
                }
                Component::CurDir => {
                    return Err(invalid_path("current-directory components are not allowed"));
                }
            }
        }

        // Path::components normalizes repeated and trailing separators. Reject
        // them so API and filesystem transports always interpret the same path.
        if value.split('/').any(str::is_empty) {
            return Err(invalid_path("empty path components are not allowed"));
        }
        if value.split('/').any(|segment| segment == ".") {
            return Err(invalid_path("current-directory components are not allowed"));
        }

        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

fn invalid_path(reason: &str) -> Error {
    Error::Validation(format!("Invalid artifact relative path: {reason}"))
}

fn symlink_error(path: &Path) -> Error {
    Error::PermissionDenied(format!(
        "Artifact path contains a symbolic link: '{}'",
        path.display()
    ))
}

/// Reject a regular file that has another hard link on Unix.
///
/// Artifact authorization is path-based, so allowing two paths to address the
/// same inode would let writes or deletes cross an authorization boundary.
pub fn reject_hard_linked_regular_file(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.is_file() && metadata.nlink() > 1 {
            return Err(Error::PermissionDenied(format!(
                "Artifact path is a hard-linked regular file: '{}'",
                path.display()
            )));
        }
    }

    #[cfg(not(unix))]
    let _ = (path, metadata);

    Ok(())
}

/// Resolve a validated artifact path and reject every existing symlink component.
///
/// This prevents stable symlink escapes. A hostile process with write access to
/// the artifact directory could still replace a checked component before the
/// caller opens it; fully eliminating that race requires descriptor-relative OS
/// APIs not currently exposed by this transport abstraction.
pub async fn resolve_checked_path(
    base: &Path,
    relative: &ValidatedRelativePath,
) -> Result<PathBuf> {
    let canonical_base = tokio::fs::canonicalize(base).await.map_err(|e| {
        Error::Io(format!(
            "Failed to resolve artifact root '{}': {e}",
            base.display()
        ))
    })?;
    let mut current = canonical_base.clone();

    for component in relative.as_path().components() {
        current.push(component.as_os_str());
        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(symlink_error(&current));
                }
                reject_hard_linked_regular_file(&current, &metadata)?;
                let canonical = tokio::fs::canonicalize(&current).await.map_err(|e| {
                    Error::Io(format!("Failed to resolve '{}': {e}", current.display()))
                })?;
                if !canonical.starts_with(&canonical_base) {
                    return Err(Error::PermissionDenied(format!(
                        "Artifact path escapes storage root: '{}'",
                        current.display()
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => {
                return Err(Error::Io(format!(
                    "Failed to inspect artifact path '{}': {e}",
                    current.display()
                )));
            }
        }
    }

    Ok(canonical_base.join(relative.as_path()))
}

/// Create a validated path's parents one component at a time, rejecting symlinks.
pub async fn ensure_checked_parent_dirs(
    base: &Path,
    relative: &ValidatedRelativePath,
) -> Result<PathBuf> {
    crate::utils::create_shared_dir_all(base)
        .await
        .map_err(|e| {
            Error::Io(format!(
                "Failed to create artifact root '{}': {e}",
                base.display()
            ))
        })?;
    let canonical_base = tokio::fs::canonicalize(base).await.map_err(|e| {
        Error::Io(format!(
            "Failed to resolve artifact root '{}': {e}",
            base.display()
        ))
    })?;
    let mut current = canonical_base.clone();

    if let Some(parent) = relative.as_path().parent() {
        for component in parent.components() {
            current.push(component.as_os_str());
            match tokio::fs::symlink_metadata(&current).await {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() {
                        return Err(symlink_error(&current));
                    }
                    if !metadata.is_dir() {
                        return Err(Error::Io(format!(
                            "Artifact parent '{}' is not a directory",
                            current.display()
                        )));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    match tokio::fs::create_dir(&current).await {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(e) => {
                            return Err(Error::Io(format!(
                                "Failed to create artifact directory '{}': {e}",
                                current.display()
                            )));
                        }
                    }
                    let metadata = tokio::fs::symlink_metadata(&current).await.map_err(|e| {
                        Error::Io(format!(
                            "Failed to inspect created artifact directory '{}': {e}",
                            current.display()
                        ))
                    })?;
                    if metadata.file_type().is_symlink() || !metadata.is_dir() {
                        return Err(symlink_error(&current));
                    }
                }
                Err(e) => {
                    return Err(Error::Io(format!(
                        "Failed to inspect artifact directory '{}': {e}",
                        current.display()
                    )));
                }
            }

            let canonical = tokio::fs::canonicalize(&current).await.map_err(|e| {
                Error::Io(format!("Failed to resolve '{}': {e}", current.display()))
            })?;
            if !canonical.starts_with(&canonical_base) {
                return Err(Error::PermissionDenied(format!(
                    "Artifact path escapes storage root: '{}'",
                    current.display()
                )));
            }
        }
    }

    let full_path = canonical_base.join(relative.as_path());
    if let Ok(metadata) = tokio::fs::symlink_metadata(&full_path).await {
        if metadata.file_type().is_symlink() {
            return Err(symlink_error(&full_path));
        }
        reject_hard_linked_regular_file(&full_path, &metadata)?;
    }
    Ok(full_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_component_aware_relative_paths() {
        assert!(ValidatedRelativePath::new("core/logs/v1.txt").is_ok());
        for invalid in [
            "",
            "/etc/passwd",
            "../escape",
            "a/../escape",
            "./file",
            "a/./file",
            "a//b",
            "a/",
            "C:/escape",
            "a\\..\\escape",
            "nul\0byte",
        ] {
            assert!(ValidatedRelativePath::new(invalid).is_err(), "{invalid}");
        }
        assert!(ValidatedRelativePath::new("not..a-parent/file").is_ok());
    }
}
