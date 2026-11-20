//! Offline process entrypoint support. No profiles, service config or children.
use crate::{
    execution_authority::consumer_contract::ExpectedMembership,
    repositories::{recorded_rollout::canonical, rollout_admin::RolloutAdminRepository as Repo},
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{ConnectOptions, Connection};
#[cfg(unix)]
use std::os::unix::{
    fs::{MetadataExt, OpenOptionsExt},
    io::AsRawFd,
};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

#[cfg(unix)]
use std::{fs::OpenOptions, io::Read};

type SafeResult<T> = Result<T, &'static str>;
const HELP:&str="attune-rollout-admin: one-shot offline deployment control\nCommands: validate --manifest FILE\nstatus --credentials FILE\nstage --credentials FILE --manifest FILE --receipt FILE\nactivate|revoke --credentials FILE --receipt FILE --confirm-generation UUID\nverify-provisioning --credentials FILE\nAll files must be private regular owner-0600 files; output directory owner-0700. No service/profile/environment credentials are read.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    database_url: String,
    installation_schema: String,
    installation_id: Uuid,
    database_oid: i64,
    schema_oid: i64,
    capability_role: String,
    runtime_roles: Vec<String>,
}
fn identifier(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
#[cfg(unix)]
fn private_metadata(m: &std::fs::Metadata, uid: u32, max: u64) -> bool {
    m.is_file() && m.mode() & 0o7777 == 0o600 && m.uid() == uid && m.nlink() == 1 && m.len() <= max
}
pub(crate) fn read_private_with(
    path: &Path,
    max: u64,
    opened: impl FnOnce(),
) -> SafeResult<Vec<u8>> {
    #[cfg(not(unix))]
    {
        let _ = (path, max, opened);
        Err("unsupported_platform")
    }
    #[cfg(unix)]
    {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| "private_file_denied")?;
        let before = file.metadata().map_err(|_| "private_file_denied")?;
        if !private_metadata(&before, unsafe { libc::geteuid() }, max) {
            return Err("private_file_denied");
        }
        opened();
        let mut bytes = Vec::new();
        (&file)
            .take(max + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "private_file_denied")?;
        let after = file.metadata().map_err(|_| "private_file_denied")?;
        let named = std::fs::symlink_metadata(path).map_err(|_| "private_file_denied")?;
        if bytes.len() as u64 > max
            || !named.is_file()
            || named.ino() != before.ino()
            || named.dev() != before.dev()
            || after.mode() & 0o7777 != 0o600
            || after.uid() != before.uid()
            || after.nlink() != 1
            || named.mode() & 0o7777 != 0o600
            || named.uid() != before.uid()
            || named.nlink() != 1
            || after.len() != before.len()
            || after.mtime() != before.mtime()
            || after.mtime_nsec() != before.mtime_nsec()
            || after.ctime() != before.ctime()
            || after.ctime_nsec() != before.ctime_nsec()
        {
            return Err("private_file_denied");
        }
        Ok(bytes)
    }
}
fn read_json(path: &Path, max: u64) -> SafeResult<Value> {
    serde_json::from_slice(&read_private_with(path, max, || {})?)
        .map_err(|_| "invalid_private_json")
}
fn manifest(path: &Path) -> SafeResult<ExpectedMembership> {
    let body = read_private_with(path, 2 * 1024 * 1024, || {})?;
    let value = serde_json::from_slice(&body).map_err(|_| "invalid_manifest")?;
    canonical(value).map_err(|_| "invalid_manifest")
}

struct ReceiptOutput {
    dir: File,
    temp: tempfile::NamedTempFile,
    path: PathBuf,
    #[cfg(unix)]
    parent: PathBuf,
}
impl ReceiptOutput {
    fn reserve(path: &Path) -> SafeResult<Self> {
        #[cfg(not(unix))]
        {
            let _ = path;
            Err("unsupported_platform")
        }
        #[cfg(unix)]
        {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let dir = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(parent)
                .map_err(|_| "receipt_output_denied")?;
            let m = dir.metadata().map_err(|_| "receipt_output_denied")?;
            if m.uid() != unsafe { libc::geteuid() }
                || m.mode() & 0o7777 != 0o700
                || path.file_name().is_none()
                || std::fs::symlink_metadata(path).is_ok()
            {
                return Err("receipt_output_denied");
            }
            // Anchor publication to the already-open trusted directory even if a
            // pathname is renamed. Linux procfs is not a credential transport.
            let anchored = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
            let temp =
                tempfile::NamedTempFile::new_in(&anchored).map_err(|_| "receipt_output_denied")?;
            let path = anchored.join(path.file_name().ok_or("receipt_output_denied")?);
            Ok(Self {
                dir,
                temp,
                path,
                parent: parent.to_owned(),
            })
        }
    }
    fn publish(mut self, body: &Value) -> SafeResult<()> {
        #[cfg(unix)]
        {
            let held = self.dir.metadata().map_err(|_| "receipt_output_failed")?;
            let named =
                std::fs::symlink_metadata(&self.parent).map_err(|_| "receipt_output_failed")?;
            if !named.is_dir()
                || named.ino() != held.ino()
                || named.dev() != held.dev()
                || held.mode() & 0o7777 != 0o700
            {
                return Err("receipt_output_failed");
            }
        }
        let bytes = serde_jcs::to_vec(body).map_err(|_| "receipt_output_failed")?;
        if bytes.len() > 16384 {
            return Err("receipt_output_failed");
        }
        self.temp
            .write_all(&bytes)
            .map_err(|_| "receipt_output_failed")?;
        self.temp
            .as_file()
            .sync_all()
            .map_err(|_| "receipt_output_failed")?;
        self.temp
            .persist_noclobber(&self.path)
            .map_err(|_| "receipt_output_failed")?;
        self.dir.sync_all().map_err(|_| "receipt_output_failed")?;
        Ok(())
    }
}

struct Args {
    command: String,
    credentials: Option<PathBuf>,
    manifest: Option<PathBuf>,
    receipt: Option<PathBuf>,
    confirm: Option<Uuid>,
    receipt_row: Option<i64>,
}
fn args(values: Vec<String>) -> SafeResult<Args> {
    let mut it = values.into_iter();
    let command = it.next().ok_or("invalid_arguments")?;
    let mut a = Args {
        command,
        credentials: None,
        manifest: None,
        receipt: None,
        confirm: None,
        receipt_row: None,
    };
    while let Some(key) = it.next() {
        let value = it.next().ok_or("invalid_arguments")?;
        match key.as_str() {
            "--credentials" if a.credentials.is_none() => a.credentials = Some(value.into()),
            "--manifest" if a.manifest.is_none() => a.manifest = Some(value.into()),
            "--receipt" if a.receipt.is_none() => a.receipt = Some(value.into()),
            "--receipt-row" if a.receipt_row.is_none() => {
                let id = value.parse::<i64>().map_err(|_| "invalid_arguments")?;
                if id <= 0 {
                    return Err("invalid_arguments");
                }
                a.receipt_row = Some(id);
            }
            "--confirm-generation" if a.confirm.is_none() => {
                a.confirm = Some(value.parse().map_err(|_| "invalid_arguments")?)
            }
            _ => return Err("invalid_arguments"),
        }
    }
    let valid = match a.command.as_str() {
        "validate" => {
            a.manifest.is_some()
                && a.credentials.is_none()
                && a.receipt.is_none()
                && a.confirm.is_none()
        }
        "status" | "verify-provisioning" => {
            a.credentials.is_some()
                && a.manifest.is_none()
                && a.receipt.is_none()
                && a.confirm.is_none()
        }
        "stage" => {
            a.credentials.is_some()
                && a.manifest.is_some()
                && a.receipt.is_some()
                && a.confirm.is_none()
        }
        "activate" => {
            a.credentials.is_some()
                && a.manifest.is_none()
                && a.receipt.is_some()
                && a.confirm.is_some()
                && a.receipt_row.is_none()
        }
        "revoke" => {
            a.credentials.is_some()
                && a.manifest.is_none()
                && (a.receipt.is_some() ^ a.receipt_row.is_some())
                && a.confirm.is_some()
        }
        _ => false,
    };
    if !valid || (a.command != "revoke" && a.receipt_row.is_some()) {
        return Err("invalid_arguments");
    }
    Ok(a)
}
async fn database_command(a: Args) -> SafeResult<Value> {
    let credentials: Credentials = serde_json::from_slice(&read_private_with(
        a.credentials.as_deref().ok_or("invalid_arguments")?,
        16384,
        || {},
    )?)
    .map_err(|_| "invalid_credentials_file")?;
    if !identifier(&credentials.installation_schema)
        || credentials.installation_id.is_nil()
        || credentials.database_oid <= 0
        || credentials.schema_oid <= 0
        || !identifier(&credentials.capability_role)
        || credentials.runtime_roles.is_empty()
        || credentials.runtime_roles.iter().any(|r| !identifier(r))
    {
        return Err("invalid_credentials_file");
    }
    let url = url::Url::parse(&credentials.database_url).map_err(|_| "invalid_credentials_file")?;
    if !matches!(url.scheme(), "postgres" | "postgresql")
        || url.host_str().is_none()
        || !identifier(url.username())
        || url.password().is_none()
        || url.path().len() <= 1
        || url.fragment().is_some()
        || url.query_pairs().any(|(k, _)| k != "sslmode")
    {
        return Err("invalid_credentials_file");
    }
    let options = crate::db::ConnectionIdentity::for_process("rollout-admin", "offline")
        .connect_options(&credentials.database_url)
        .map_err(|_| "invalid_credentials_file")?
        .options([
            (
                "search_path",
                format!("pg_catalog,\"{}\",pg_temp", credentials.installation_schema),
            ),
            ("statement_timeout", "15000".into()),
            ("lock_timeout", "10000".into()),
            ("idle_in_transaction_session_timeout", "20000".into()),
        ])
        .disable_statement_logging();
    let mut conn = tokio::time::timeout(
        Duration::from_secs(10),
        sqlx::PgConnection::connect_with(&options),
    )
    .await
    .map_err(|_| "database_connection_denied")?
    .map_err(|_| "database_connection_denied")?;
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        Repo::assert_identity(&mut conn, url.username())
            .await
            .map_err(|_| "installation_denied")?;
        Repo::assert_trusted_schema(&mut conn, &credentials.installation_schema)
            .await
            .map_err(|_| "provisioning_denied")?;
        let status = Repo::status(&mut conn)
            .await
            .map_err(|_| "installation_denied")?;
        if status["installation"]["uuid"] != credentials.installation_id.to_string()
            || status["installation"]["schema"] != credentials.installation_schema
            || status["installation"]["database_oid"] != credentials.database_oid
            || status["installation"]["schema_oid"] != credentials.schema_oid
            || status["installation"]["capability_role"] != credentials.capability_role
            || status["installation"]["database"] != options.get_database().unwrap_or("")
        {
            return Err("installation_denied");
        }
        if a.command == "status" {
            return Ok(status);
        }
        Repo::verify(
            &mut conn,
            &credentials.installation_schema,
            &credentials.runtime_roles,
        )
        .await
        .map_err(|_| "provisioning_denied")?;
        match a.command.as_str() {
            "verify-provisioning" => {
                Ok(json!({"verified":true,"installation_id":credentials.installation_id}))
            }
            "stage" => {
                let m = manifest(a.manifest.as_deref().ok_or("invalid_arguments")?)?;
                let output =
                    ReceiptOutput::reserve(a.receipt.as_deref().ok_or("invalid_arguments")?)?;
                let mut tx = conn.begin().await.map_err(|_| "stage_denied")?;
                let receipt = Repo::stage(
                    &mut tx,
                    &serde_json::to_value(&m).map_err(|_| "invalid_manifest")?,
                )
                .await
                .map_err(|_| "stage_denied")?;
                tx.commit()
                    .await
                    .map_err(|_| "stage_acknowledgement_unknown")?;
                output.publish(&receipt)?;
                Ok(json!({"staged":true,"generation":m.snapshot.rollout_generation}))
            }
            "activate" | "revoke" => {
                let receipt = if let Some(id) = a.receipt_row {
                    Repo::receipt(&mut conn, id)
                        .await
                        .map_err(|_| "receipt_denied")?
                } else {
                    read_json(a.receipt.as_deref().ok_or("invalid_arguments")?, 16384)?
                };
                if receipt["target"]["generation"]
                    != a.confirm.ok_or("invalid_arguments")?.to_string()
                {
                    return Err("target_confirmation_denied");
                }
                let mut tx = conn.begin().await.map_err(|_| "receipt_denied")?;
                if a.command == "activate" {
                    Repo::activate(&mut tx, &receipt).await
                } else {
                    Repo::revoke(&mut tx, &receipt).await
                }
                .map_err(|_| "receipt_denied")?;
                tx.commit()
                    .await
                    .map_err(|_| "activation_acknowledgement_unknown")?;
                Ok(json!({"operation":a.command,"completed":true}))
            }
            _ => Err("invalid_arguments"),
        }
    })
    .await
    .unwrap_or(Err("operation_timeout"));
    let closed = tokio::time::timeout(Duration::from_secs(5), conn.close()).await;
    if matches!(closed, Ok(Ok(()))) {
        result
    } else {
        match result {
            Err(code) => Err(code),
            Ok(_) => Err("database_close_failed"),
        }
    }
}

/// Call from a separate binary only, before any multithreaded runtime/config.
pub fn main_entry() -> std::process::ExitCode {
    if !cfg!(target_os = "linux") {
        eprintln!("{{\"error\":\"unsupported_platform\"}}");
        return std::process::ExitCode::FAILURE;
    }
    let values = std::env::args_os()
        .skip(1)
        .map(|value| value.into_string().map_err(|_| "invalid_arguments"))
        .collect::<SafeResult<Vec<String>>>();
    if crate::parent_process_protection::protect_service_parent().is_err() {
        eprintln!("{{\"error\":\"parent_protection_failed\"}}");
        return std::process::ExitCode::FAILURE;
    }
    // Process-local and before runtime threads: exact owner-only file creation
    // must not depend on a launcher's inherited umask.
    #[cfg(unix)]
    unsafe {
        libc::umask(0o077);
    }
    // This binary owns its process. No service/PG/profile/credential env survives.
    for (key, _) in std::env::vars_os() {
        std::env::remove_var(key);
    }
    let values = match values {
        Ok(values) => values,
        Err(code) => {
            eprintln!("{}", json!({"error":code}));
            return std::process::ExitCode::FAILURE;
        }
    };
    if values == ["--help"] || values == ["-h"] {
        println!("{HELP}");
        return std::process::ExitCode::SUCCESS;
    }
    let result=args(values).and_then(|a|{
    if a.command=="validate" {return Ok(json!({"valid":true,"manifest":manifest(a.manifest.as_deref().ok_or("invalid_arguments")?)?}));}
    let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_|"runtime_failed")?;
    rt.block_on(database_command(a))
 });
    match result {
        Ok(value) => {
            println!("{value}");
            std::process::ExitCode::SUCCESS
        }
        Err(code) => {
            eprintln!("{}", json!({"error":code}));
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::{symlink, PermissionsExt};
    #[test]
    #[cfg(unix)]
    fn private_fd_and_output_boundaries() {
        use std::os::unix::ffi::OsStrExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = std::panic::catch_unwind(|| {
            let p = root.path().join("input");
            std::fs::write(&p, b"{}").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
            let meta = std::fs::metadata(&p).unwrap();
            assert!(!private_metadata(&meta, meta.uid().wrapping_add(1), 10));
            assert!(read_private_with(&p, 10, || {}).is_ok());
            assert!(read_private_with(&p, 1, || {}).is_err());
            let link = root.path().join("link");
            symlink(&p, &link).unwrap();
            assert!(read_private_with(&link, 10, || {}).is_err());
            std::fs::remove_file(&link).unwrap();
            std::fs::hard_link(&p, &link).unwrap();
            assert!(read_private_with(&p, 10, || {}).is_err());
            std::fs::remove_file(&link).unwrap();
            assert!(read_private_with(&p, 10, || {
                std::fs::rename(&p, &link).unwrap();
                std::fs::write(&p, b"{}").unwrap();
            })
            .is_err());
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read_private_with(&p, 10, || {}).is_err());
            assert!(read_private_with(root.path(), 10, || {}).is_err());
            assert!(read_private_with(Path::new("/dev/null"), 10, || {}).is_err());
            let fifo = root.path().join("fifo");
            let c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            assert!(read_private_with(&fifo, 10, || {}).is_err());
            let socket = root.path().join("socket");
            let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
            assert!(read_private_with(&socket, 10, || {}).is_err());
            drop(listener);
            let output = root.path().join("receipt");
            let held = ReceiptOutput::reserve(&output).unwrap();
            std::fs::write(&output, b"sentinel").unwrap();
            assert!(held.publish(&json!({"nonce":"dummy"})).is_err());
            assert_eq!(std::fs::read(&output).unwrap(), b"sentinel");
            std::fs::remove_file(&output).unwrap();
            ReceiptOutput::reserve(&output)
                .unwrap()
                .publish(&json!({"version":1}))
                .unwrap();
            let m = std::fs::metadata(&output).unwrap();
            assert!(private_metadata(&m, m.uid(), 16384));
        });
        root.close().unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
