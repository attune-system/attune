//! Fixed local file effects invoked only through the trusted Pod adapter. No
//! listener, arbitrary executable selector, host path or platform credential.
use attune_common::workload_launch::{LaunchInputKind, PinnedLaunchInput, Sha256Digest};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::Path,
};
const STATE_VOLUME: &str = "/opt/attune/transfer";
const STATE: &str = "/opt/attune/transfer/private";
fn invalid() -> io::Error {
    io::Error::other("invalid fixed workload transfer")
}
fn prepare_private_state(volume: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(volume)?;
    if !meta.is_dir()
        || !(meta.uid() == unsafe { libc::geteuid() } || (meta.uid() == 0 && meta.gid() == 1000))
    {
        return Err(invalid());
    }
    // Kubernetes emptyDir roots are 0777 (and fsGroup may add setgid). The
    // volume is mounted only in the trusted helper. Put locks, receipts and
    // archive bodies in an owned private child, never in that volume root.
    let state = volume.join("private");
    match fs::DirBuilder::new().mode(0o700).create(&state) {
        Ok(()) => File::open(volume)?.sync_all()?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let meta = fs::symlink_metadata(state)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o777 != 0o700 {
        return Err(invalid());
    }
    Ok(())
}
fn lock() -> io::Result<File> {
    prepare_private_state(Path::new(STATE_VOLUME))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(format!("{STATE}/lock"))?;
    if file.metadata()?.nlink() != 1 || unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0
    {
        return Err(invalid());
    }
    Ok(file)
}
pub fn seed(arguments: &[String]) -> io::Result<()> {
    if arguments.len() != 5 {
        return Err(invalid());
    }
    let kind = match arguments[0].as_str() {
        "pack" => LaunchInputKind::Pack,
        "runtime" => LaunchInputKind::Runtime,
        "runner" => LaunchInputKind::Runner,
        _ => return Err(invalid()),
    };
    let hash = Sha256Digest::parse(&arguments[1]).map_err(|_| invalid())?;
    let size = arguments[2].parse::<u64>().map_err(|_| invalid())?;
    let expanded = arguments[3].parse::<u64>().map_err(|_| invalid())?;
    let files = arguments[4].parse::<u64>().map_err(|_| invalid())?;
    if size == 0
        || size > 1024 * 1024 * 1024
        || expanded == 0
        || expanded > 4 * 1024 * 1024 * 1024
        || files == 0
        || files > 100_000
    {
        return Err(invalid());
    }
    let _lock = lock()?;
    let mut body = tempfile::tempfile_in(STATE)?;
    let actual = io::copy(&mut io::stdin().take(size + 1), &mut body)?;
    if actual != size {
        return Err(invalid());
    }
    body.seek(SeekFrom::Start(0))?;
    let mut sha = sha2::Sha256::new();
    use sha2::Digest;
    let mut buffer = [0; 65536];
    loop {
        let count = body.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        sha.update(&buffer[..count]);
    }
    let computed = sha
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if computed != hash.as_str() {
        return Err(invalid());
    }
    body.seek(SeekFrom::Start(0))?;
    let receipt = format!("{}\n{size}\n{expanded}\n{files}\n", hash.as_str());
    let name = format!("{STATE}/{}", arguments[0]);
    match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&name)
    {
        Ok(mut file) => {
            if !file.metadata()?.is_file() || file.metadata()?.nlink() != 1 {
                return Err(invalid());
            }
            let mut prior = String::new();
            Read::by_ref(&mut file)
                .take(1024)
                .read_to_string(&mut prior)?;
            return if prior == receipt {
                Ok(())
            } else {
                Err(invalid())
            };
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let pin = PinnedLaunchInput {
        pin_id: 1,
        kind,
        object_id: 1,
        object_version: "fixed-helper-transfer".into(),
        size_bytes: size,
        sha256: hash,
        max_expanded_bytes: expanded,
        max_files: files,
    };
    crate::cache::seed_fixed_root(&pin, &mut body, Path::new(kind.container_root()))
        .map_err(|_| invalid())?;
    let mut pending = tempfile::NamedTempFile::new_in(STATE)?;
    pending.write_all(receipt.as_bytes())?;
    pending.as_file().sync_all()?;
    pending.persist_noclobber(name).map_err(|e| e.error)?;
    File::open(STATE)?.sync_all()?;
    Ok(())
}
pub fn install() -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let destination = Path::new("/opt/attune/bootstrap/runner");
    let mut source = File::open(executable)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o555)
        .custom_flags(libc::O_NOFOLLOW)
        .open(destination)?;
    io::copy(&mut source, &mut output)?;
    output.sync_all()?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o555))?;
    File::open(destination.parent().ok_or_else(invalid)?)?.sync_all()?;
    Ok(())
}
pub fn archive(arguments: &[String]) -> io::Result<()> {
    if arguments.len() != 3 {
        return Err(invalid());
    }
    let limits = crate::cache::CacheLimits {
        compressed_bytes: arguments[0].parse().map_err(|_| invalid())?,
        expanded_bytes: arguments[1].parse().map_err(|_| invalid())?,
        files: arguments[2].parse().map_err(|_| invalid())?,
    };
    if limits.compressed_bytes == 0
        || limits.compressed_bytes > 1024 * 1024 * 1024
        || limits.expanded_bytes == 0
        || limits.expanded_bytes > 4 * 1024 * 1024 * 1024
        || limits.files == 0
        || limits.files > 100_000
    {
        return Err(invalid());
    }
    let _lock = lock()?;
    let path = format!("{STATE}/runtime-publication.zip");
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => {
            if !file.metadata()?.is_file()
                || file.metadata()?.nlink() != 1
                || file.metadata()?.len() > limits.compressed_bytes
            {
                return Err(invalid());
            }
            file
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let pending = tempfile::NamedTempFile::new_in(STATE)?;
            let file = crate::cache::archive_fixed_root(
                Path::new("/opt/attune/runtime"),
                pending.as_file().try_clone()?,
                limits,
            )
            .map_err(|_| invalid())?;
            pending.persist_noclobber(&path).map_err(|e| e.error)?;
            File::open(STATE)?.sync_all()?;
            file
        }
        Err(e) => return Err(e),
    };
    file.seek(SeekFrom::Start(0))?;
    attune_common::workload_archive::validate_zip(
        &mut file,
        &attune_common::workload_archive::ArchivePolicy {
            root: LaunchInputKind::Runtime,
            max_compressed_bytes: limits.compressed_bytes,
            max_expanded_bytes: limits.expanded_bytes,
            max_files: limits.files,
        },
    )
    .map_err(|_| invalid())?;
    io::copy(&mut file, &mut io::stdout())?;
    Ok(())
}
#[cfg(target_os = "linux")]
pub fn terminate() -> io::Result<()> {
    // Only the fixed Kubernetes bootstrap PID1 may be signaled. Do not inspect
    // /proc/1/exe: Runner deliberately disables dumpability, which correctly
    // denies that ptrace-gated lookup to this unprivileged sibling utility.
    let cmd = fs::read("/proc/1/cmdline")?;
    let mut args = cmd.split(|b| *b == 0);
    if args.next() != Some(b"/opt/attune/bootstrap/runner".as_slice())
        || args.next() != Some(b"gated".as_slice())
        || args.next() != Some(b"".as_slice())
        || args.next().is_some()
    {
        return Err(invalid());
    }
    // PID1 handles TERM before opening its gate. It terminates/waits its child;
    // namespace-init exit then kills escaped descendants as well. No caller PID,
    // arbitrary signal, kill(-1) or host/container selector is accepted.
    if unsafe { libc::kill(1, libc::SIGTERM) } < 0 {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kubernetes_volume_uses_an_owned_private_child_and_reopens_it() {
        let volume = tempfile::tempdir().unwrap();
        fs::set_permissions(volume.path(), fs::Permissions::from_mode(0o2777)).unwrap();
        prepare_private_state(volume.path()).unwrap();
        let state = volume.path().join("private");
        let before = fs::symlink_metadata(&state).unwrap();
        assert_eq!(before.uid(), unsafe { libc::geteuid() });
        assert_eq!(before.mode() & 0o777, 0o700);
        fs::write(state.join("receipt"), b"owned receipt").unwrap();
        prepare_private_state(volume.path()).unwrap();
        assert_eq!(fs::read(state.join("receipt")).unwrap(), b"owned receipt");
        assert_eq!(fs::symlink_metadata(state).unwrap().ino(), before.ino());
    }

    #[test]
    fn unsafe_existing_state_is_rejected_without_repair() {
        let volume = tempfile::tempdir().unwrap();
        let state = volume.path().join("private");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(prepare_private_state(volume.path()).is_err());
        assert_eq!(fs::symlink_metadata(state).unwrap().mode() & 0o777, 0o777);
    }

    #[test]
    fn symlink_state_or_volume_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let volume = root.path().join("volume");
        let other = root.path().join("other");
        fs::create_dir(&volume).unwrap();
        fs::create_dir(&other).unwrap();
        std::os::unix::fs::symlink(&other, volume.join("private")).unwrap();
        assert!(prepare_private_state(&volume).is_err());
        let linked_volume = root.path().join("linked-volume");
        std::os::unix::fs::symlink(&volume, &linked_volume).unwrap();
        assert!(prepare_private_state(&linked_volume).is_err());
        assert!(fs::read_dir(other).unwrap().next().is_none());
    }
}
