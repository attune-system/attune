//! Offline operator tool. Never uses deployed-service configuration or credentials.
use attune_common::repositories::encryption_conversion::convert_offline;

const HELP: &str = "Offline compatibility conversion of Key values and retained fingerprint metadata.\n\nUsage: attune-encryption-convert [--dry-run | --apply --offline-writers-stopped]\n\nRequired environment:\n  ATTUNE_ENCRYPTION_CONVERSION_DATABASE_URL  Explicit operator-owned target\n  ATTUNE_ENCRYPTION_CONVERSION_SECRET        Configured high-entropy server secret\n\nExamples:\n  attune-encryption-convert --dry-run\n  attune-encryption-convert --apply --offline-writers-stopped\n\nStop every service writer before applying. Use a dedicated maintenance operator.\nRead docs/deployment/encryption-fingerprint-remediation.md.\nNo server configuration is loaded. Secrets and fingerprints are never printed.";

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!("{HELP}");
        return std::process::ExitCode::SUCCESS;
    }
    let apply = match args.as_slice() {
        [] => false,
        [dry] if dry == "--dry-run" => false,
        [apply, stopped] if apply == "--apply" && stopped == "--offline-writers-stopped" => true,
        _ => {
            eprintln!("Invalid arguments. Use --dry-run or --apply --offline-writers-stopped. See --help.");
            return std::process::ExitCode::FAILURE;
        }
    };
    let result = async {
        let url = std::env::var("ATTUNE_ENCRYPTION_CONVERSION_DATABASE_URL").map_err(|_| {
            "Missing ATTUNE_ENCRYPTION_CONVERSION_DATABASE_URL. See --help.".to_string()
        })?;
        let secret = std::env::var("ATTUNE_ENCRYPTION_CONVERSION_SECRET")
            .map_err(|_| "Missing ATTUNE_ENCRYPTION_CONVERSION_SECRET. See --help.".to_string())?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .map_err(|_| {
                "Cannot connect to the explicitly selected conversion database".to_string()
            })?;
        let result = convert_offline(&pool, &secret, apply).await;
        pool.close().await;
        result.map_err(safe_error)
    }
    .await;
    match result {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string(&report).expect("Conversion counts serialize")
            );
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn safe_error(error: attune_common::Error) -> String {
    // Database/serde/provider diagnostics can include complete rows or payloads.
    // Never forward them to an operator transcript, even on corrupt input.
    match error {
        attune_common::Error::Database(sqlx::Error::Database(error))
            if error.message() == "Offline encryption maintenance role required" =>
            "A dedicated operator must have explicit attune_encryption_maintenance membership. See the runbook.".into(),
        attune_common::Error::Encryption(_) => "Wrong master secret, unknown verifier or corrupt ciphertext; no changes were committed.".into(),
        _ => "Conversion validation or database operation failed; no changes were committed. Check retained metadata integrity and maintenance prerequisites.".into(),
    }
}
