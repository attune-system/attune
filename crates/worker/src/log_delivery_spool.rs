//! Worker-owned encrypted custody for a bounded, immutable log delivery handoff.
//!
//! A saved entry is acknowledged only after its contents and directory entry are
//! synchronized. Recovery requires the same worker identity and encryption key.
//! This is process/container restart durability, not node/volume durability.
use attune_common::{crypto, Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

const MAGIC: &[u8] = b"ATTUNE_LOG_SPOOL_V1\n";
const NONCE_BYTES: usize = 12;

/// Exact on-wire bytes, already framed/encrypted according to the output policy.
/// No bearer credentials, runtime API tokens, or encryption keys belong here.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SpoolSegment {
    pub sequence: i64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SpoolDelivery {
    pub execution: i64,
    pub version: i64,
    pub stream: i64,
    pub worker: i64,
    pub attempt: Uuid,
    pub source_instance: Uuid,
    pub custody: Uuid,
    pub finalize_by: chrono::DateTime<chrono::Utc>,
    pub truncated: bool,
    pub authenticated_manifest: Option<String>,
    pub segments: Vec<SpoolSegment>,
}

impl SpoolDelivery {
    pub fn validate(&self, max_payload_bytes: u64) -> Result<()> {
        if [self.execution, self.version, self.stream, self.worker]
            .iter()
            .any(|id| *id <= 0)
            || self.attempt.is_nil()
            || self.source_instance.is_nil()
            || self.custody.is_nil()
            || self.segments.is_empty()
        {
            return Err(Error::validation("Invalid log spool binding"));
        }
        let mut total = 0_u64;
        for (sequence, segment) in self.segments.iter().enumerate() {
            if i64::try_from(sequence).ok() != Some(segment.sequence) || segment.bytes.is_empty() {
                return Err(Error::validation(
                    "Log spool segments must be contiguous and nonempty",
                ));
            }
            total = total
                .checked_add(segment.bytes.len() as u64)
                .filter(|total| *total <= max_payload_bytes)
                .ok_or_else(|| Error::validation("Log spool payload exceeds its byte budget"))?;
        }
        Ok(())
    }
}

struct OwnedSpool {
    root: PathBuf,
    worker: i64,
    key: [u8; 32],
    max_bytes: u64,
    max_entry_bytes: u64,
    max_payload_bytes: u64,
    // File locking fences overlapping worker incarnations. Blocking operations
    // retain this Arc even if their async caller is cancelled.
    _lease: File,
    serialization: Mutex<()>,
}

#[derive(Clone)]
pub struct LogDeliverySpool(Arc<OwnedSpool>);

fn io(error: std::io::Error) -> Error {
    Error::Io(error.to_string())
}
fn open_private(path: &Path, create_new: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options.open(path).map_err(io)
}
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(io)
}

impl LogDeliverySpool {
    /// Open one worker's private persistent root. The caller supplies a directory
    /// dedicated to this spool, never a pack-controlled or shared scratch root.
    #[cfg(unix)]
    pub async fn open(
        root: PathBuf,
        worker: i64,
        encryption_key: &str,
        max_bytes: u64,
        max_entry_bytes: u64,
        max_payload_bytes: u64,
    ) -> Result<Self> {
        if worker <= 0
            || encryption_key.len() < 32
            || max_payload_bytes == 0
            || max_entry_bytes <= (MAGIC.len() + NONCE_BYTES + 16) as u64
            || max_entry_bytes == u64::MAX
            || max_entry_bytes > max_bytes
        {
            return Err(Error::validation("Invalid log spool configuration"));
        }
        let mut digest = Sha256::new();
        digest.update(b"attune.worker.log_spool.v1\0");
        digest.update(encryption_key.as_bytes());
        let key: [u8; 32] = digest.finalize().into();
        tokio::task::spawn_blocking(move || {
            fs::create_dir_all(&root).map_err(io)?;
            let metadata = fs::symlink_metadata(&root).map_err(io)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(Error::invalid_state(
                    "Log spool root must be a private directory",
                ));
            }
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(io)?;
            let lease = open_private(&root.join("owner.lock"), false)?;
            lease
                .try_lock()
                .map_err(|_| Error::invalid_state("Log spool already has an active owner"))?;
            // Only temporary names created by this spool are eligible for crash
            // recovery cleanup. Unknown files/directories are never removed.
            for entry in fs::read_dir(&root).map_err(io)? {
                let entry = entry.map_err(io)?;
                let name = entry.file_name();
                if let Some(uuid) = name.to_str().and_then(|s| s.strip_prefix("pending-")) {
                    if Uuid::parse_str(uuid).is_ok() && entry.file_type().map_err(io)?.is_file() {
                        fs::remove_file(entry.path()).map_err(io)?;
                    }
                }
            }
            sync_directory(&root)?;
            Ok(Self(Arc::new(OwnedSpool {
                root,
                worker,
                key,
                max_bytes,
                max_entry_bytes,
                max_payload_bytes,
                _lease: lease,
                serialization: Mutex::new(()),
            })))
        })
        .await
        .map_err(|_| Error::internal("Log spool initialization task failed"))?
    }

    pub async fn save(&self, entry: SpoolDelivery) -> Result<()> {
        let owned = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = owned
                .serialization
                .lock()
                .map_err(|_| Error::invalid_state("Log spool owner failed"))?;
            entry.validate(owned.max_payload_bytes)?;
            if entry.worker != owned.worker {
                return Err(Error::validation("Log spool worker mismatch"));
            }
            let path = owned.path(entry.version);
            if path.try_exists().map_err(io)? {
                return if owned.read(entry.version)? == entry {
                    Ok(())
                } else {
                    Err(Error::invalid_state("Log spool entry is immutable"))
                };
            }
            let plaintext = serde_json::to_vec(&entry)
                .map_err(|_| Error::internal("Log spool serialization failed"))?;
            let encrypted =
                crypto::encrypt_authenticated(&plaintext, &owned.key, &owned.aad(entry.version))?;
            let bytes = (MAGIC.len() + NONCE_BYTES + encrypted.ciphertext.len()) as u64;
            if bytes > owned.max_entry_bytes
                || owned
                    .usage()?
                    .checked_add(bytes)
                    .is_none_or(|total| total > owned.max_bytes)
            {
                return Err(Error::invalid_state("Log spool capacity exhausted"));
            }
            let temporary = owned.root.join(format!("pending-{}", Uuid::new_v4()));
            let result = (|| {
                let mut file = open_private(&temporary, true)?;
                file.write_all(MAGIC).map_err(io)?;
                file.write_all(&encrypted.nonce).map_err(io)?;
                file.write_all(&encrypted.ciphertext).map_err(io)?;
                file.sync_all().map_err(io)?;
                // Hard-link publication is atomic and never replaces a foreign
                // path or an entry from a conflicting handoff.
                fs::hard_link(&temporary, &path).map_err(io)?;
                fs::remove_file(&temporary).map_err(io)?;
                sync_directory(&owned.root)
            })();
            if temporary.try_exists().map_err(io)? {
                fs::remove_file(&temporary).map_err(io)?;
                sync_directory(&owned.root)?;
            }
            result
        })
        .await
        .map_err(|_| Error::internal("Log spool write task failed"))?
    }

    pub async fn load(&self, version: i64) -> Result<SpoolDelivery> {
        let owned = self.0.clone();
        tokio::task::spawn_blocking(move || owned.read(version))
            .await
            .map_err(|_| Error::internal("Log spool read task failed"))?
    }

    pub async fn versions(&self) -> Result<Vec<i64>> {
        let owned = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut versions = Vec::new();
            for entry in fs::read_dir(&owned.root).map_err(io)? {
                let entry = entry.map_err(io)?;
                if let Some(version) = spool_version(&entry.file_name().to_string_lossy()) {
                    if !entry.file_type().map_err(io)?.is_file() {
                        return Err(Error::invalid_state("Invalid log spool entry type"));
                    }
                    versions.push(version);
                }
            }
            versions.sort_unstable();
            Ok(versions)
        })
        .await
        .map_err(|_| Error::internal("Log spool scan task failed"))?
    }

    /// Call only after acknowledged Ready publication, or after recording the
    /// exact intent's incomplete/revoked outcome. Age alone is not deletion proof.
    pub async fn remove(&self, version: i64, custody: Uuid) -> Result<()> {
        let owned = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = owned
                .serialization
                .lock()
                .map_err(|_| Error::invalid_state("Log spool owner failed"))?;
            if owned.read(version)?.custody != custody {
                return Err(Error::invalid_state("Log spool custody mismatch"));
            }
            fs::remove_file(owned.path(version)).map_err(io)?;
            sync_directory(&owned.root)
        })
        .await
        .map_err(|_| Error::internal("Log spool removal task failed"))?
    }
}

fn spool_version(name: &str) -> Option<i64> {
    let text = name.strip_suffix(".spool")?;
    let version: i64 = text.parse().ok()?;
    (version > 0 && version.to_string() == text).then_some(version)
}
impl OwnedSpool {
    fn path(&self, version: i64) -> PathBuf {
        self.root.join(format!("{version}.spool"))
    }
    fn aad(&self, version: i64) -> Vec<u8> {
        [
            MAGIC,
            self.worker.to_be_bytes().as_slice(),
            version.to_be_bytes().as_slice(),
        ]
        .concat()
    }
    fn usage(&self) -> Result<u64> {
        let mut total = 0_u64;
        for entry in fs::read_dir(&self.root).map_err(io)? {
            let entry = entry.map_err(io)?;
            if spool_version(&entry.file_name().to_string_lossy()).is_some() {
                let metadata = entry.metadata().map_err(io)?;
                if !entry.file_type().map_err(io)?.is_file() {
                    return Err(Error::invalid_state("Invalid log spool entry type"));
                }
                total = total
                    .checked_add(metadata.len())
                    .ok_or_else(|| Error::invalid_state("Log spool usage overflow"))?;
            }
        }
        Ok(total)
    }
    fn read(&self, version: i64) -> Result<SpoolDelivery> {
        if version <= 0 {
            return Err(Error::validation("Invalid log spool version"));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let file = options.open(self.path(version)).map_err(io)?;
        let metadata = file.metadata().map_err(io)?;
        if !metadata.is_file() || metadata.len() > self.max_entry_bytes {
            return Err(Error::invalid_state("Invalid log spool entry size/type"));
        }
        let mut bytes = Vec::new();
        file.take(self.max_entry_bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() as u64 > self.max_entry_bytes
            || !bytes.starts_with(MAGIC)
            || bytes.len() < MAGIC.len() + NONCE_BYTES + 16
        {
            return Err(Error::invalid_state("Invalid log spool envelope"));
        }
        let nonce = bytes[MAGIC.len()..MAGIC.len() + NONCE_BYTES]
            .try_into()
            .map_err(|_| Error::invalid_state("Invalid log spool nonce"))?;
        let plaintext = crypto::decrypt_authenticated(
            &bytes[MAGIC.len() + NONCE_BYTES..],
            &nonce,
            &self.key,
            &self.aad(version),
        )?;
        let entry: SpoolDelivery = serde_json::from_slice(&plaintext)
            .map_err(|_| Error::invalid_state("Invalid log spool payload"))?;
        entry.validate(self.max_payload_bytes)?;
        if entry.worker != self.worker || entry.version != version {
            return Err(Error::invalid_state("Log spool identity mismatch"));
        }
        Ok(entry)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    const KEY: &str = "spool-test-key-with-at-least-thirty-two-bytes";
    fn delivery(version: i64) -> SpoolDelivery {
        SpoolDelivery {
            execution: 1,
            version,
            stream: 3,
            worker: 4,
            attempt: Uuid::new_v4(),
            source_instance: Uuid::new_v4(),
            custody: Uuid::new_v4(),
            finalize_by: chrono::Utc::now() + chrono::Duration::seconds(60),
            truncated: false,
            authenticated_manifest: Some("authenticated-proof-only".into()),
            segments: vec![SpoolSegment {
                sequence: 0,
                bytes: b"private log bytes".to_vec(),
            }],
        }
    }
    async fn spool(root: &Path) -> LogDeliverySpool {
        LogDeliverySpool::open(root.to_owned(), 4, KEY, 8192, 4096, 1024)
            .await
            .unwrap()
    }
    #[tokio::test]
    async fn encrypted_entry_survives_reopen_and_requires_exact_custody_for_removal() {
        let root = tempfile::tempdir().unwrap();
        let entry = delivery(2);
        let owner = spool(root.path()).await;
        owner.save(entry.clone()).await.unwrap();
        let disk = fs::read(root.path().join("2.spool")).unwrap();
        assert!(!disk.windows(7).any(|w| w == b"private"));
        assert_eq!(owner.versions().await.unwrap(), vec![2]);
        drop(owner);
        let owner = spool(root.path()).await;
        assert!(owner.load(2).await.unwrap() == entry);
        assert!(owner.remove(2, Uuid::new_v4()).await.is_err());
        owner.remove(2, entry.custody).await.unwrap();
        assert!(owner.versions().await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn quota_and_immutable_replay_are_enforced() {
        let root = tempfile::tempdir().unwrap();
        let entry = delivery(2);
        let budget =
            (serde_json::to_vec(&entry).unwrap().len() + MAGIC.len() + NONCE_BYTES + 16) as u64;
        let owner = LogDeliverySpool::open(root.path().to_owned(), 4, KEY, budget, budget, 1024)
            .await
            .unwrap();
        owner.save(entry.clone()).await.unwrap();
        owner.save(entry.clone()).await.unwrap();
        let mut changed = entry;
        changed.truncated = true;
        assert!(owner.save(changed).await.is_err());
        assert!(owner.save(delivery(5)).await.is_err());
        assert_eq!(owner.versions().await.unwrap(), vec![2]);
    }
    #[tokio::test]
    async fn concurrent_owner_and_wrong_key_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let owner = spool(root.path()).await;
        owner.save(delivery(2)).await.unwrap();
        assert!(
            LogDeliverySpool::open(root.path().to_owned(), 4, KEY, 8192, 4096, 1024)
                .await
                .is_err()
        );
        drop(owner);
        let other = LogDeliverySpool::open(
            root.path().to_owned(),
            4,
            "different-key-with-thirty-two-characters",
            8192,
            4096,
            1024,
        )
        .await
        .unwrap();
        assert!(other.load(2).await.is_err());
    }
    #[tokio::test]
    async fn tampering_and_cross_version_replay_fail_authentication() {
        let root = tempfile::tempdir().unwrap();
        let owner = spool(root.path()).await;
        owner.save(delivery(2)).await.unwrap();
        fs::copy(root.path().join("2.spool"), root.path().join("5.spool")).unwrap();
        assert!(owner.load(5).await.is_err());
        let mut bytes = fs::read(root.path().join("2.spool")).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(root.path().join("2.spool"), bytes).unwrap();
        assert!(owner.load(2).await.is_err());
    }
    #[tokio::test]
    async fn owned_temporary_cleanup_preserves_neighbors_and_rejects_symlinks() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let temp = root.path().join(format!("pending-{}", Uuid::new_v4()));
        fs::write(&temp, b"partial").unwrap();
        fs::write(root.path().join("neighbor"), b"keep").unwrap();
        let owner = spool(root.path()).await;
        assert!(!temp.exists());
        assert!(root.path().join("neighbor").exists());
        let outside = tempfile::NamedTempFile::new().unwrap();
        symlink(outside.path(), root.path().join("2.spool")).unwrap();
        assert!(owner.load(2).await.is_err());
        assert!(owner.save(delivery(2)).await.is_err());
    }
}
