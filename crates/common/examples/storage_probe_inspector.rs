//! Controlled local operator adapter, not an HTTP service or authorization API.
//! Build centrally with the matching source; never prints DB errors/credentials.
use attune_common::repositories::storage_probe_inspection::{
    InspectionAccess, InspectionRequest, StorageProbeInspectionRepository,
};
use serde::Deserialize;
use std::{
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    time::Duration,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    database_url: String,
    schema: String,
    access: InspectionAccess,
}

fn private(path: &Path) -> anyhow::Result<Vec<u8>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0
            && metadata.len() <= 16384,
        "invalid private context"
    );
    let mut bytes = Vec::new();
    file.take(16385).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 16384, "context exceeds bound");
    Ok(bytes)
}

async fn inspect() -> anyhow::Result<()> {
    let path = std::env::var_os("ATTUNE_STORAGE_PROBE_INSPECTOR_CONTEXT")
        .ok_or_else(|| anyhow::anyhow!("private context required"))?;
    let context: Context = serde_json::from_slice(&private(Path::new(&path))?)?;
    let url = url::Url::parse(&context.database_url)?;
    anyhow::ensure!(
        matches!(url.scheme(), "postgres" | "postgresql") && url.host_str() == Some("127.0.0.1"),
        "owned loopback DB endpoint required"
    );
    let mut bytes = Vec::new();
    std::io::stdin().take(16385).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 16384, "request exceeds bound");
    let request: InspectionRequest = serde_json::from_slice(&bytes)?;
    context.access.validate_scope(&request.scope)?;
    let pool = tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&context.database_url),
    )
    .await??;
    let response = tokio::time::timeout(
        Duration::from_secs(15),
        StorageProbeInspectionRepository::inspect(&pool, &context.schema, &context.access, request),
    )
    .await??;
    let bytes = serde_json::to_vec(&response)?;
    anyhow::ensure!(bytes.len() <= 32768, "response exceeds bound");
    println!("{}", std::str::from_utf8(&bytes)?);
    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--help" {
        println!("storage_probe_inspector --execute\nOne bounded JSON request on stdin. Private operator context path comes only from ATTUNE_STORAGE_PROBE_INSPECTOR_CONTEXT. No SQL argument or HTTP listener.");
        return;
    }
    if args.len() != 1 || args[0] != "--execute" || inspect().await.is_err() {
        eprintln!("Storage probe inspection rejected or unavailable; inspect the owned context and matching precompiled artifact. Database errors and credentials withheld.");
        std::process::exit(1);
    }
}
