//! Central-owned original producer setup/inspection, never a runtime SDK grant.
//! Application DB access is exclusively through NativeFileSetupRepository.
use attune_common::{
    auth::{
        crypto_provider,
        jwt::{validate_token, JwtConfig},
    },
    repositories::{
        file_native_setup::{
            NativeFileSetupRepository, NativeSetupAccess, NativeSetupOperation, NativeSetupRequest,
            NativeSetupResponse,
        },
        native_generation_origin::{NativeControllerAccess, VerifiedNativeOrigin},
    },
};
use serde::Deserialize;
use std::{
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    enabled_for_controlled_acceptance: bool,
    database_url: String,
    access: NativeSetupAccess,
    controller: NativeControllerAccess,
    maintenance_access_token_file: PathBuf,
    jwt_secret_file: PathBuf,
    encryption_key_file: PathBuf,
}

fn private(path: &Path) -> anyhow::Result<Vec<u8>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let info = file.metadata()?;
    anyhow::ensure!(
        info.is_file()
            && info.uid() == unsafe { libc::geteuid() }
            && info.mode() & 0o077 == 0
            && info.len() <= 16384,
        "invalid private native context"
    );
    let mut bytes = Vec::new();
    file.take(16385).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 16384, "private context bound");
    Ok(bytes)
}

async fn execute() -> anyhow::Result<()> {
    let file = std::env::var_os("ATTUNE_NATIVE_ACCEPTANCE_CONTEXT")
        .ok_or_else(|| anyhow::anyhow!("private native context required"))?;
    let context: Context = serde_json::from_slice(&private(Path::new(&file))?)?;
    anyhow::ensure!(
        context.enabled_for_controlled_acceptance,
        "controlled gate disabled"
    );
    let url = url::Url::parse(&context.database_url)?;
    anyhow::ensure!(
        matches!(url.scheme(), "postgres" | "postgresql")
            && url.host_str() == Some("127.0.0.1")
            && url.port().is_some()
            && url.path().trim_start_matches('/') == context.access.database_name
            && url.username() == context.access.database_role,
        "owned scoped loopback DB required"
    );
    let mut bytes = Vec::new();
    std::io::stdin().take(16385).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 16384, "request bound");
    let request: NativeSetupRequest = serde_json::from_slice(&bytes)?;
    context.access.validate(&request)?;
    crypto_provider::install();
    let jwt = JwtConfig {
        secret: String::from_utf8(private(&context.jwt_secret_file)?)?
            .trim()
            .to_owned(),
        ..Default::default()
    };
    anyhow::ensure!(jwt.secret.len() >= 32, "owned JWT verifier required");
    let token = String::from_utf8(private(&context.maintenance_access_token_file)?)?;
    let claims = validate_token(token.trim(), &jwt)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&context.database_url)
        .await?;
    let data = match request.operation {
        NativeSetupOperation::Setup => {
            anyhow::ensure!(
                request.version_id.is_none(),
                "setup cannot select an existing file"
            );
            if let Some(original) =
                NativeFileSetupRepository::existing(&pool, &context.access, &claims).await?
            {
                original
            } else {
                // Fresh native controller enrollment precedes source/target DB
                // locks. No caller member, provenance, stopped flag or SQL.
                let origin = VerifiedNativeOrigin::acquire(
                    &context.controller,
                    &request.run_id,
                    &request.project,
                    context.access.generation,
                    request.mode,
                )
                .await?;
                let key = String::from_utf8(private(&context.encryption_key_file)?)?;
                NativeFileSetupRepository::setup(
                    &pool,
                    &context.access,
                    &claims,
                    origin,
                    key.trim(),
                )
                .await?
            }
        }
        NativeSetupOperation::Inspect => {
            NativeFileSetupRepository::inspect(&pool, &context.access, &claims, request.version_id)
                .await?
        }
    };
    anyhow::ensure!(
        data["scope"] == context.controller.scope()?,
        "native topology scope changed"
    );
    anyhow::ensure!(
        claims.exp > chrono::Utc::now().timestamp(),
        "operator authorization expired"
    );
    let response = serde_json::to_vec(&NativeSetupResponse { protocol: 1, data })?;
    anyhow::ensure!(response.len() <= 65536, "native observation bound");
    println!("{}", std::str::from_utf8(&response)?);
    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--help" {
        println!("native_settlement_adapter --execute\nBounded JSON stdin: protocol/run_id/project/mode/operation and optional generation/version_id. Private context: ATTUNE_NATIVE_ACCEPTANCE_CONTEXT. Operations: prepare-original-native-producer, observe-original-native-custody. No member, grants, SQL or service command input.");
        return;
    }
    if args.len() != 1 || args[0] != "--execute" {
        eprintln!("Native setup requires --execute and the private controlled context");
        std::process::exit(2);
    }
    match tokio::time::timeout(Duration::from_secs(110), execute()).await {
        Ok(Ok(())) => {}
        _ => {
            eprintln!("Native original producer setup/inspection unavailable; retain original generation and custody. Credentials, payload and database errors withheld.");
            std::process::exit(1);
        }
    }
}
