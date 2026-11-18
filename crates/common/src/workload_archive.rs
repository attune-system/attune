//! Canonical v1 workload ZIP validation, shared by CP publication and controller
//! input staging. Pure validation performs no filesystem access or extraction.
//! Links retain their bytes; they may resolve only within the selected fixed
//! container root. Image-external interpreter links require e.g. venv --copies.
use crate::{workload_launch::LaunchInputKind, Error, Result};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{Read, Seek, SeekFrom},
};
use zip::ZipArchive;

#[derive(Clone, Copy, Debug)]
pub struct ArchivePolicy {
    pub root: LaunchInputKind,
    pub max_compressed_bytes: u64,
    pub max_expanded_bytes: u64,
    /// Includes implicit parent directories, not only ZIP central entries.
    pub max_files: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveEntryKind {
    Directory,
    File { executable: bool },
    Symlink { target: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Canonical relative POSIX path, without a trailing slash.
    pub name: String,
    pub size_bytes: u64,
    pub kind: ArchiveEntryKind,
}
#[derive(Clone, Debug)]
pub struct ValidatedArchive {
    /// Entries remain in ZIP order; implicit directories are counted separately.
    pub entries: Vec<ArchiveEntry>,
    pub expanded_bytes: u64,
    pub files: u64,
}
fn invalid() -> Error {
    Error::validation("Invalid or escaping canonical workload archive")
}
fn quota() -> Error {
    Error::validation("Workload archive exceeds admitted bounds")
}

/// Reads all decompressed bytes through bounded readers, verifying ZIP CRCs.
/// Compressed length is checked before parsing metadata. Successful validation
/// rewinds the original reader; digest/pin authentication is the caller's job.
pub fn validate_zip<R: Read + Seek>(
    reader: &mut R,
    policy: &ArchivePolicy,
) -> Result<ValidatedArchive> {
    if policy.max_compressed_bytes == 0 || policy.max_expanded_bytes == 0 || policy.max_files == 0 {
        return Err(quota());
    }
    let length = reader.seek(SeekFrom::End(0)).map_err(|_| invalid())?;
    if length == 0 || length > policy.max_compressed_bytes {
        return Err(quota());
    }
    reader.seek(SeekFrom::Start(0)).map_err(|_| invalid())?;
    let entries = {
        let mut archive = ZipArchive::new(&mut *reader).map_err(|_| invalid())?;
        if archive.len() as u64 > policy.max_files {
            return Err(quota());
        }
        // ZipArchive indexes by name and can otherwise hide duplicate central
        // entries. Require one consistent local/central representation before
        // accepting its metadata or expanding payloads.
        archive
            .validate_header_consistency()
            .map_err(|_| invalid())?;
        let mut entries = Vec::with_capacity(archive.len());
        let mut expanded = 0u64;
        for index in 0..archive.len() {
            let mut file = archive.by_index(index).map_err(|_| invalid())?;
            if file.encrypted() {
                return Err(invalid());
            }
            let name = file.name().map_err(|_| invalid())?.into_owned();
            let directory = file.is_dir();
            let name = if directory {
                name.strip_suffix('/').ok_or_else(invalid)?.to_string()
            } else {
                name
            };
            validate_name(&name)?;
            let size = file.size();
            expanded = expanded.checked_add(size).ok_or_else(quota)?;
            if expanded > policy.max_expanded_bytes {
                return Err(quota());
            }
            let mode = file.unix_mode().unwrap_or(0);
            let kind = mode & 0o170000;
            let entry_kind = if directory {
                if (kind != 0 && kind != 0o040000) || size != 0 {
                    return Err(invalid());
                }
                ArchiveEntryKind::Directory
            } else if kind == 0o120000 {
                if size > 4096 {
                    return Err(quota());
                }
                let mut target = String::new();
                file.by_ref()
                    .take(size.saturating_add(1))
                    .read_to_string(&mut target)
                    .map_err(|_| invalid())?;
                if target.len() as u64 != size {
                    return Err(invalid());
                }
                ArchiveEntryKind::Symlink { target }
            } else {
                if kind != 0 && kind != 0o100000 {
                    return Err(invalid());
                }
                // Read to EOF, so CRC errors cannot hide behind metadata-only
                // checks. One extra byte detects dishonest expanded lengths.
                let count = std::io::copy(
                    &mut file.by_ref().take(size.saturating_add(1)),
                    &mut std::io::sink(),
                )
                .map_err(|_| invalid())?;
                if count != size {
                    return Err(invalid());
                }
                ArchiveEntryKind::File {
                    executable: mode & 0o111 != 0,
                }
            };
            entries.push(ArchiveEntry {
                name,
                size_bytes: size,
                kind: entry_kind,
            });
        }
        entries
    };
    let (expanded_bytes, files) = validate_entries(&entries, policy)?;
    reader.seek(SeekFrom::Start(0)).map_err(|_| invalid())?;
    Ok(ValidatedArchive {
        entries,
        expanded_bytes,
        files,
    })
}

/// Validates a settled nofollow filesystem inventory with exactly the same
/// logical path/link graph rules as a ZIP input. Callers must separately reject
/// hardlinks/special files and prove that all writers have stopped.
pub fn validate_tree(entries: &[ArchiveEntry], policy: &ArchivePolicy) -> Result<()> {
    validate_entries(entries, policy).map(|_| ())
}
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 4096 || name.contains(['\\', '\0']) {
        return Err(invalid());
    }
    let components = name.split('/').collect::<Vec<_>>();
    if components.len() > 128
        || components
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == ".." || part.len() > 255)
    {
        return Err(invalid());
    }
    Ok(())
}
#[derive(Clone, Copy)]
enum Node<'a> {
    Directory,
    File,
    Link(&'a str),
}
fn validate_entries(entries: &[ArchiveEntry], policy: &ArchivePolicy) -> Result<(u64, u64)> {
    if policy.max_expanded_bytes == 0 || policy.max_files == 0 {
        return Err(quota());
    }
    let mut names = BTreeSet::new();
    let mut nodes = BTreeMap::new();
    let mut expanded = 0u64;
    for entry in entries {
        validate_name(&entry.name)?;
        if !names.insert(entry.name.as_str()) {
            return Err(invalid());
        }
        expanded = expanded.checked_add(entry.size_bytes).ok_or_else(quota)?;
        if expanded > policy.max_expanded_bytes {
            return Err(quota());
        }
        let node = match &entry.kind {
            ArchiveEntryKind::Directory => {
                if entry.size_bytes != 0 {
                    return Err(invalid());
                }
                Node::Directory
            }
            ArchiveEntryKind::File { .. } => Node::File,
            ArchiveEntryKind::Symlink { target } => {
                if target.len() as u64 != entry.size_bytes
                    || target.is_empty()
                    || target.len() > 4096
                    || target.contains(['\\', '\0'])
                {
                    return Err(invalid());
                }
                Node::Link(target)
            }
        };
        if let Some(existing) = nodes.insert(entry.name.as_str(), node) {
            if !matches!((existing, node), (Node::Directory, Node::Directory)) {
                return Err(invalid());
            }
        }
        let mut current = entry.name.as_str();
        while let Some((parent, _)) = current.rsplit_once('/') {
            if nodes
                .get(parent)
                .is_some_and(|node| !matches!(node, Node::Directory))
            {
                return Err(invalid());
            }
            nodes.entry(parent).or_insert(Node::Directory);
            current = parent;
        }
        if nodes.len() as u64 > policy.max_files {
            return Err(quota());
        }
    }
    for entry in entries {
        if let ArchiveEntryKind::Symlink { target } = &entry.kind {
            resolve_link(&entry.name, target, policy.root.container_root(), &nodes)?;
        }
    }
    Ok((expanded, nodes.len() as u64))
}
fn target_components<'a>(target: &'a str, root: &str) -> Result<(bool, Vec<&'a str>)> {
    let absolute = target.starts_with('/');
    let relative = if absolute {
        let suffix = target.strip_prefix(root).ok_or_else(invalid)?;
        if suffix.is_empty() {
            ""
        } else {
            suffix.strip_prefix('/').ok_or_else(invalid)?
        }
    } else {
        target
    };
    let mut components = Vec::new();
    for part in relative.split('/') {
        if part.len() > 255 {
            return Err(invalid());
        }
        if !part.is_empty() && part != "." {
            components.push(part);
        }
    }
    if components.len() > 128 {
        return Err(invalid());
    }
    Ok((absolute, components))
}
fn resolve_link(
    name: &str,
    target: &str,
    root: &str,
    nodes: &BTreeMap<&str, Node<'_>>,
) -> Result<()> {
    let (absolute, components) = target_components(target, root)?;
    let mut resolved = if absolute {
        Vec::new()
    } else {
        name.split('/')
            .take(name.split('/').count() - 1)
            .collect::<Vec<_>>()
    };
    let mut pending = VecDeque::from(components);
    let mut followed = 0;
    while let Some(part) = pending.pop_front() {
        if part == ".." {
            if resolved.pop().is_none() {
                return Err(invalid());
            }
            continue;
        }
        resolved.push(part);
        if let Some(Node::Link(target)) = nodes.get(resolved.join("/").as_str()) {
            followed += 1;
            if followed > 40 {
                return Err(invalid());
            }
            resolved.pop();
            let (absolute, components) = target_components(target, root)?;
            if absolute {
                resolved.clear();
            }
            for component in components.into_iter().rev() {
                pending.push_front(component);
            }
        } else if !pending.is_empty()
            && nodes
                .get(resolved.join("/").as_str())
                .is_some_and(|node| matches!(node, Node::File))
        {
            return Err(invalid());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::{write::SimpleFileOptions, ZipWriter};
    fn policy() -> ArchivePolicy {
        ArchivePolicy {
            root: LaunchInputKind::Runtime,
            max_compressed_bytes: 65536,
            max_expanded_bytes: 1024,
            max_files: 32,
        }
    }
    fn file(name: &str) -> ArchiveEntry {
        ArchiveEntry {
            name: name.into(),
            size_bytes: 1,
            kind: ArchiveEntryKind::File { executable: false },
        }
    }
    fn link(name: &str, target: &str) -> ArchiveEntry {
        ArchiveEntry {
            name: name.into(),
            size_bytes: target.len() as u64,
            kind: ArchiveEntryKind::Symlink {
                target: target.into(),
            },
        }
    }
    #[test]
    fn validates_internal_relative_and_absolute_links_without_rewriting() {
        let entries = vec![
            file("lib/tool"),
            link("bin/tool", "../lib/tool"),
            link("absolute", "/opt/attune/runtime/lib/tool"),
        ];
        validate_tree(&entries, &policy()).unwrap();
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buffer);
            zip.start_file("lib/tool", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"x").unwrap();
            zip.add_symlink(
                "absolute",
                "/opt/attune/runtime/lib/tool",
                SimpleFileOptions::default(),
            )
            .unwrap();
            zip.finish().unwrap();
        }
        let validated = validate_zip(&mut buffer, &policy()).unwrap();
        assert_eq!(validated.entries[1], entries[2]);
        assert_eq!(buffer.position(), 0);
    }
    #[test]
    fn rejects_escape_cycle_link_parent_and_composed_escape() {
        for entries in [
            vec![file("../out")],
            vec![file("/out")],
            vec![file("a//b")],
            vec![link("a", "../out")],
            vec![link("a", "/usr/bin/python")],
            vec![link("a", "b"), link("b", "a")],
            vec![link("a", "safe"), file("a/child")],
            vec![link("dir/jump", ".."), link("dir/deep", "jump/../outside")],
            vec![link("a", "/opt/attune/runtime/../pack/secret")],
        ] {
            assert!(
                validate_tree(&entries, &policy()).is_err(),
                "accepted {entries:?}"
            );
        }
    }
    #[test]
    fn implicit_directories_count_against_quota() {
        let mut policy = policy();
        policy.max_files = 2;
        assert!(validate_tree(&[file("a/b/c")], &policy).is_err());
        policy.max_files = 3;
        validate_tree(&[file("a/b/c")], &policy).unwrap();
        policy.max_expanded_bytes = 1;
        assert!(validate_tree(&[file("a"), file("b")], &policy).is_err());
    }
    #[test]
    fn crc_and_compressed_bounds_are_enforced_before_acceptance() {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buffer);
            zip.start_file(
                "tool",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
            zip.write_all(b"unique-owned-payload").unwrap();
            zip.finish().unwrap();
        }
        let mut policy = policy();
        policy.max_compressed_bytes = 1;
        assert!(validate_zip(&mut buffer, &policy).is_err());
        policy.max_compressed_bytes = 65536;
        let position = buffer
            .get_ref()
            .windows(20)
            .position(|chunk| chunk == b"unique-owned-payload")
            .unwrap();
        buffer.get_mut()[position] ^= 1;
        assert!(validate_zip(&mut buffer, &policy).is_err());
    }
    #[test]
    fn rejects_duplicate_and_mismatched_local_central_names() {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buffer);
            for name in ["a", "b"] {
                zip.start_file(name, SimpleFileOptions::default()).unwrap();
                zip.write_all(b"x").unwrap();
            }
            zip.finish().unwrap();
        }
        let original = buffer.into_inner();
        for central_too in [false, true] {
            let mut bytes = original.clone();
            for index in 0..bytes.len().saturating_sub(46) {
                let name_offset = if bytes[index..].starts_with(b"PK\x03\x04") {
                    Some(30)
                } else if central_too && bytes[index..].starts_with(b"PK\x01\x02") {
                    Some(46)
                } else {
                    None
                };
                if let Some(offset) = name_offset {
                    if bytes.get(index + offset) == Some(&b'b') {
                        bytes[index + offset] = b'a';
                    }
                }
            }
            assert!(validate_zip(&mut Cursor::new(bytes), &policy()).is_err());
        }
    }
}
