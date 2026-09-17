use attune_common::{config::Config, test_database::TestDatabase};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> attune_common::Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    if command.is_none() || command.as_deref() == Some("--list") {
        return Ok(());
    }

    let config_path = args.next();
    let database_name = args.next();
    if args.next().is_some() {
        return Err(usage_error());
    }

    let config_path = config_path.ok_or_else(|| {
        attune_common::Error::InvalidState(
            "test database lifecycle command requires a config path".to_string(),
        )
    })?;
    let config = Config::load_from_file(&config_path)?;

    match command.as_deref() {
        Some("create") if database_name.is_none() => {
            let database = TestDatabase::create_detached(&config.database).await?;
            println!("{}\t{}", database.database_name(), database.database_url());
            Ok(())
        }
        Some("drop") => {
            let database_name = database_name.ok_or_else(|| {
                attune_common::Error::InvalidState(
                    "drop requires the detached database name".to_string(),
                )
            })?;
            TestDatabase::cleanup_detached(&config.database, &database_name).await
        }
        _ => Err(usage_error()),
    }
}

fn usage_error() -> attune_common::Error {
    attune_common::Error::InvalidState(
        "usage: test_database_lifecycle {create <config>|drop <config> <database-name>}"
            .to_string(),
    )
}
