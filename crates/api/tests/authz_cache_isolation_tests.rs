mod helpers;

use attune_common::config::Config;
use axum::http::StatusCode;
use helpers::{fail_after_database_creation_for_test, Result, TestContext};
use std::path::PathBuf;

async fn database_exists(database_name: &str) -> Result<bool> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config.test.yaml");
    let config = Config::load_from_file(path.to_str().expect("UTF-8 config path"))?;
    let pool = sqlx::PgPool::connect(&config.database.url).await?;
    let exists = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_database WHERE datname = $1)",
    )
    .bind(database_name)
    .fetch_one(&pool)
    .await?;
    pool.close().await;
    Ok(exists)
}

#[tokio::test]
async fn partial_context_construction_drops_database_owner() -> Result<()> {
    let mut database_name = String::new();
    assert!(fail_after_database_creation_for_test(&mut database_name)
        .await
        .is_err());
    assert!(!database_name.is_empty());
    assert!(!database_exists(&database_name).await?);
    Ok(())
}

#[tokio::test]
async fn cache_enabled_equal_identity_ids_remain_app_state_isolated() -> Result<()> {
    // Fresh database clones start their identity sequences at the same value. Give the
    // equal IDs different grants, prime the first app-state cache, then prove
    // the second state evaluates its own repository data.
    let admin = TestContext::new().await?.with_admin_auth().await?;
    let installer = TestContext::new().await?.with_pack_install_auth().await?;

    let admin_identity: i64 = sqlx::query_scalar(
        "SELECT identity FROM permission_assignment ORDER BY created DESC LIMIT 1",
    )
    .fetch_one(&admin.pool)
    .await?;
    let installer_identity: i64 = sqlx::query_scalar(
        "SELECT identity FROM permission_assignment ORDER BY created DESC LIMIT 1",
    )
    .fetch_one(&installer.pool)
    .await?;
    assert_eq!(
        admin_identity, installer_identity,
        "regression requires equal primary IDs across isolated databases"
    );

    let admin_response = admin.get("/api/v1/permissions/sets", admin.token()).await?;
    assert_eq!(admin_response.status(), StatusCode::OK);

    let installer_response = installer
        .get("/api/v1/permissions/sets", installer.token())
        .await?;
    assert_eq!(installer_response.status(), StatusCode::FORBIDDEN);
    Ok(())
}
