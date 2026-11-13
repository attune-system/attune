//! Native entrypoints may be a file, or a directory of static Linux variants.
//! Directory dispatch never invokes a shell or performs a PATH lookup.
use std::path::{Component, Path, PathBuf};

pub const LINUX_VARIANTS: [&str; 2] = ["linux-amd64", "linux-arm64"];

pub fn validate_directory(path: &Path) -> Result<(), String> {
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("native directory path must not traverse parents".into());
    }
    // A link in the directory's ancestry could escape its recorded release tree.
    for ancestor in path.ancestors().filter(|path| !path.as_os_str().is_empty()) {
        if std::fs::symlink_metadata(ancestor)
            .map_err(|_| "native entrypoint ancestry is unavailable")?
            .file_type()
            .is_symlink()
        {
            return Err("native directory ancestry must not contain symlinks".into());
        }
    }
    let mut count = 0;
    for entry in std::fs::read_dir(path).map_err(|_| "native directory is unavailable")? {
        let entry = entry.map_err(|_| "native directory entry is unavailable")?;
        let name = entry.file_name();
        if !LINUX_VARIANTS.iter().any(|known| name == *known) {
            return Err("native directory contains an unknown variant".into());
        }
        let metadata =
            std::fs::symlink_metadata(entry.path()).map_err(|_| "native variant is unavailable")?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("native variant must be a regular file, never a symlink".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err("native variant must be executable".into());
            }
        }
        count += 1;
    }
    if count == 0 {
        return Err("native directory has no variants".into());
    }
    Ok(())
}

pub fn resolve(path: &Path, os: &str, arch: &str) -> Result<PathBuf, String> {
    if !path.is_dir() {
        return Ok(path.to_owned()); // Preserve existing ordinary file entrypoints.
    }
    validate_directory(path)?;
    let variant = match (os, arch) {
        ("linux", "x86_64") => "linux-amd64",
        ("linux", "aarch64") => "linux-arm64",
        _ => return Err("unsupported native directory platform".into()),
    };
    let selected = path.join(variant);
    if !selected.is_file() {
        return Err("native directory has no matching platform variant".into());
    }
    Ok(selected)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn variant(root: &Path, name: &str) {
        let path = root.join(name);
        std::fs::write(&path, b"owned native fixture").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn target_selection_is_exact_and_regular_files_are_unchanged() {
        let root = tempfile::tempdir().unwrap();
        variant(root.path(), "linux-amd64");
        assert_eq!(
            resolve(root.path(), "linux", "x86_64").unwrap(),
            root.path().join("linux-amd64")
        );
        assert!(resolve(root.path(), "linux", "aarch64").is_err());
        assert!(resolve(root.path(), "windows", "x86_64").is_err());
        variant(root.path(), "linux-arm64");
        assert_eq!(
            resolve(root.path(), "linux", "aarch64").unwrap(),
            root.path().join("linux-arm64")
        );
        let file = root.path().join("linux-amd64");
        assert_eq!(resolve(&file, "linux", "x86_64").unwrap(), file);
    }

    #[test]
    fn directory_rejects_unknown_entries_links_and_parent_traversal() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("action");
        std::fs::create_dir(&directory).unwrap();
        assert!(validate_directory(&directory).is_err());
        variant(&directory, "linux-amd64");
        variant(&directory, "unexpected");
        assert!(validate_directory(&directory).is_err());
        std::fs::remove_file(directory.join("unexpected")).unwrap();
        symlink(directory.join("linux-amd64"), directory.join("linux-arm64")).unwrap();
        assert!(validate_directory(&directory).is_err());
        std::fs::remove_file(directory.join("linux-arm64")).unwrap();
        symlink(&directory, root.path().join("alias")).unwrap();
        assert!(validate_directory(&root.path().join("alias")).is_err());
        assert!(validate_directory(&directory.join("..").join("action")).is_err());
        std::fs::set_permissions(
            directory.join("linux-amd64"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(validate_directory(&directory).is_err());
    }
}
