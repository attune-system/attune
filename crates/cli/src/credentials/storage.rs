//! Owner-private atomic files and cross-process locks. Locks use a separate inode
//! because replacing config.yaml must not release the lock on its old inode.
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub(crate) fn reject_link(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if !m.is_file() => bail!("Credential configuration must be a regular file"),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn lock(path: &Path) -> Result<Lock> {
    let parent = path.parent().context("Missing config directory")?;
    if !parent.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent)?;
    }
    reject_link(path)?;
    let mut lock_name = path.as_os_str().to_owned();
    lock_name.push(".lock");
    let lock_path = PathBuf::from(lock_name);
    reject_link(&lock_path)?;
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(&lock_path)
        .context("Cannot open config lock")?;
    private(&lock_path)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => break,
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                if std::time::Instant::now() >= deadline {
                    bail!("Timed out waiting for credential configuration lock; retry");
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => return Err(error).context("Cannot lock configuration"),
        }
    }
    Ok(Lock(file))
}

pub(crate) fn refresh_path(path: &Path, name: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let digest: String = Sha256::digest(name.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    path.with_extension(format!("refresh-{digest}"))
}

pub(crate) fn write(path: &Path, content: &[u8]) -> Result<()> {
    reject_link(path)?;
    let parent = path.parent().context("Missing config directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    private(temp.path())?;
    temp.write_all(content)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|_| anyhow::anyhow!("Failed to replace credential configuration"))?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub(crate) fn private(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    {
        use std::{os::windows::ffi::OsStrExt, ptr};
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{
                Authorization::{
                    ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW,
                    SE_FILE_OBJECT,
                },
                GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION,
                PROTECTED_DACL_SECURITY_INFORMATION,
            },
        };
        // Protected DACL grants full access only to the object's owner.
        let sddl: Vec<u16> = "D:P(A;;FA;;;OW)".encode_utf16().chain(Some(0)).collect();
        let mut name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            let mut sd = ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut sd,
                ptr::null_mut(),
            ) == 0
            {
                bail!("Cannot create owner-only credential permissions");
            }
            let mut present = 0;
            let mut defaulted = 0;
            let mut acl = ptr::null_mut();
            let ok = GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted);
            let status = if ok != 0 && present != 0 {
                SetNamedSecurityInfoW(
                    name.as_mut_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    acl,
                    ptr::null_mut(),
                )
            } else {
                1
            };
            LocalFree(sd);
            if status != 0 {
                bail!("Cannot enforce owner-only credential permissions");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refresh_and_config_locks_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let refresh = refresh_path(&path, "default");
        assert_ne!(refresh, path);
        let _refresh_lock = lock(&refresh).unwrap();
        let _config_lock = lock(&path).unwrap();
        write(&path, b"private fixture").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"private fixture");
    }
    #[cfg(unix)]
    #[test]
    fn symlink_config_is_rejected_without_changing_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let path = dir.path().join("config.yaml");
        fs::write(&target, b"unchanged").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(lock(&path).is_err());
        assert!(write(&path, b"replacement").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"unchanged");
    }
}
