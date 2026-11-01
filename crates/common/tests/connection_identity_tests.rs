use attune_common::{
    config::Config,
    db::{ConnectionIdentity, Database},
    test_database::TestDatabase,
};
use sqlx::PgConnection;

async fn assert_session(connection: &mut PgConnection, expected_name: &str, expected_schema: &str) {
    assert!(expected_name.is_ascii());
    assert!(expected_name.len() <= 63);
    let name: String = sqlx::query_scalar("SHOW application_name")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    let schema: String = sqlx::query_scalar("SELECT current_schema()")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    let search_path: String = sqlx::query_scalar("SHOW search_path")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(name, expected_name);
    assert!(name.is_ascii());
    assert!(name.len() <= 63);
    assert_eq!(schema, expected_schema);
    assert_eq!(search_path, format!("{expected_schema}, public"));
}

#[tokio::test]
async fn named_pool_keeps_identity_and_schema_on_replacement() -> anyhow::Result<()> {
    let path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
    let config = Config::load_from_file(&path)?;
    let fixture = TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop();
    let mut database_config = config.database;
    database_config.url = fixture.database_url().to_string();
    database_config.schema = Some("attune".into());
    database_config.max_connections = 2;
    database_config.min_connections = 0;
    for identity in [
        ConnectionIdentity::new("api", "integration-界".repeat(20), "main"),
        ConnectionIdentity::new("api", "界", "main"),
    ] {
        let database = Database::new_named(&database_config, identity.clone()).await?;
        let mut first = database.pool().acquire().await?;
        let mut second = database.pool().acquire().await?;
        assert_session(&mut first, &identity.application_name(), database.schema()).await;
        assert_session(&mut second, &identity.application_name(), database.schema()).await;
        first.close().await?;
        let mut replacement = database.pool().acquire().await?;
        assert_session(
            &mut replacement,
            &identity.application_name(),
            database.schema(),
        )
        .await;
        drop(replacement);
        drop(second);

        let mut connection = database.pool().acquire().await?;
        assert_session(
            &mut connection,
            &identity.application_name(),
            database.schema(),
        )
        .await;
        drop(connection);
        database.close().await;
    }
    fixture.cleanup().await?;
    Ok(())
}
