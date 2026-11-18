//! Private controller transport credential custody, never mounted into a pack
//! or the launch cache. Existing retained state wins; no legacy/fallback reader.
use attune_common::workload_session::WorkloadSession;
use chrono::Utc;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

const BODY_LIMIT: u64 = 65536;
pub struct SessionStore {
    root: PathBuf,
    _owner: File,
}
impl SessionStore {
    pub fn open(root: PathBuf) -> io::Result<Self> {
        if !root.is_absolute() {
            return Err(invalid());
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)?;
        let metadata = fs::symlink_metadata(&root)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(invalid());
        }
        let owner = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(root.join("owner.lock"))?;
        validate_file(&owner)?;
        if unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            root,
            _owner: owner,
        })
    }
    /// None is only an actually missing retained file before initial bootstrap.
    /// Malformed, expired, linked or permissive retained state is an error.
    pub fn retained(&self) -> io::Result<Option<WorkloadSession>> {
        match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.root.join("session.json"))
        {
            Ok(file) => read_session(file).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub fn initial(path: &Path) -> io::Result<WorkloadSession> {
        read_session(
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)?,
        )
    }
    pub fn persist(&self, session: &WorkloadSession) -> io::Result<()> {
        validate_session(session)?;
        // A preexisting path must also be an owned single-link regular file.
        // Refuse to overwrite a symlink, special file or unexpected owner.
        match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.root.join("session.json"))
        {
            Ok(file) => validate_file(&file)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let bytes = serde_json::to_vec(session).map_err(|_| invalid())?;
        if bytes.len() as u64 > BODY_LIMIT {
            return Err(invalid());
        }
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        validate_file(temporary.as_file())?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(self.root.join("session.json"))
            .map_err(|error| error.error)?;
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
}
fn invalid() -> io::Error {
    io::Error::other("invalid private controller session state; explicit CP bootstrap required")
}
fn validate_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() > BODY_LIMIT
    {
        return Err(invalid());
    }
    Ok(())
}
fn validate_session(session: &WorkloadSession) -> io::Result<()> {
    let now = Utc::now();
    if session.expires_at <= now || session.expires_at > now + chrono::Duration::seconds(300) {
        return Err(invalid());
    }
    Ok(())
}
fn read_session(file: File) -> io::Result<WorkloadSession> {
    validate_file(&file)?;
    let mut bytes = Vec::new();
    file.take(BODY_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > BODY_LIMIT {
        return Err(invalid());
    }
    let session = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    validate_session(&session)?;
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::workload_launch::LaunchToken;
    use std::os::unix::fs::PermissionsExt;
    fn session(token: &str) -> WorkloadSession {
        WorkloadSession {
            token: LaunchToken::new(token.into()).unwrap(),
            expires_at: Utc::now() + chrono::Duration::seconds(120),
        }
    }
    #[test]
    fn ownership_and_atomic_retained_session_survive_restart() {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = SessionStore::open(root.path().into()).unwrap();
        assert!(SessionStore::open(root.path().into()).is_err());
        assert!(store.retained().unwrap().is_none());
        store.persist(&session("cp-first")).unwrap();
        store.persist(&session("cp-renewed")).unwrap();
        assert_eq!(
            fs::metadata(root.path().join("session.json"))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
        drop(store);
        let restarted = SessionStore::open(root.path().into()).unwrap();
        assert_eq!(
            restarted.retained().unwrap().unwrap().token.as_str(),
            "cp-renewed"
        );
    }
    #[test]
    fn malformed_expired_permissive_and_linked_state_never_falls_back() {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = SessionStore::open(root.path().into()).unwrap();
        let path = root.path().join("session.json");
        store.persist(&session("cp-secret")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.retained().is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&path, root.path().join("linked")).unwrap();
        assert!(store.retained().is_err());
        assert!(store.persist(&session("new")).is_err());
        fs::remove_file(root.path().join("linked")).unwrap();
        let mut expired = session("expired");
        expired.expires_at = Utc::now() - chrono::Duration::seconds(1);
        fs::write(&path, serde_json::to_vec(&expired).unwrap()).unwrap();
        assert!(store.retained().is_err());
        fs::write(&path, b"malformed").unwrap();
        assert!(store.retained().is_err());
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("missing", &path).unwrap();
        assert!(store.retained().is_err());
        assert!(store.persist(&session("new")).is_err());
        fs::remove_file(&path).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        // O_NONBLOCK permits rejecting a special file without waiting for a
        // writer. Neither malformed state nor a FIFO can trigger bootstrap.
        assert!(store.retained().is_err());
        assert!(store.persist(&session("new")).is_err());
    }
}
