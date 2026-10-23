//! Central-only read-only adapter. Does not alter F's storage inspector protocol.
use attune_common::repositories::interoperability_inspection::{
    Access, InteroperabilityInspectionRepository, Request,
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
    access: Access,
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
        "private context required"
    );
    let mut bytes = Vec::new();
    file.take(16385).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 16384, "context limit");
    Ok(bytes)
}

async fn inspect() -> anyhow::Result<()> {
    let path = std::env::var_os("ATTUNE_INTEROPERABILITY_INSPECTOR_CONTEXT")
        .ok_or_else(|| anyhow::anyhow!("private context required"))?;
    let context: Context = serde_json::from_slice(&private(Path::new(&path))?)?;
    let url = url::Url::parse(&context.database_url)?;
    anyhow::ensure!(
        matches!(url.scheme(), "postgres" | "postgresql") && url.host_str() == Some("127.0.0.1"),
        "owned loopback DB required"
    );
    let mut input = Vec::new();
    std::io::stdin().take(16385).read_to_end(&mut input)?;
    anyhow::ensure!(input.len() <= 16384, "request limit");
    let request: Request = serde_json::from_slice(&input)?;
    context.access.validate(&request)?;
    let pool = tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&context.database_url),
    )
    .await??;
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        InteroperabilityInspectionRepository::inspect(
            &pool,
            &context.schema,
            &context.access,
            request,
        ),
    )
    .await;
    pool.close().await;
    let result = result??;
    let result = serde_json::to_vec(&result)?;
    anyhow::ensure!(result.len() <= 262144, "observation limit");
    println!("{}", std::str::from_utf8(&result)?);
    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--help" {
        println!("interoperability_inspector --execute\nBounded JSON request on stdin; private context via ATTUNE_INTEROPERABILITY_INSPECTOR_CONTEXT. No SQL argument, credential on argv, HTTP listener or write operation.");
        return;
    }
    if args.len() != 1 || args[0] != "--execute" || inspect().await.is_err() {
        eprintln!("Owned interoperability inspection rejected; verify the SELECT-only observer context and matching centrally compiled binary. Credentials and database details suppressed.");
        std::process::exit(1);
    }
}
