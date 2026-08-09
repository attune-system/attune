//! Integration tests for database migrations
//!
//! These tests verify that migrations run successfully, the schema is correct,
//! and basic database operations work as expected.

mod helpers;

use helpers::*;
use sqlx::Row;

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_migrations_applied() {
    let pool = create_test_pool().await.unwrap();

    // Verify migrations were applied by checking that core tables exist
    // We check for multiple tables to ensure the schema is properly set up
    let tables = vec!["pack", "action", "trigger", "rule", "execution"];

    for table_name in tables {
        let row = sqlx::query(&format!(
            r#"
            SELECT EXISTS (
                SELECT FROM information_schema.tables
                WHERE table_schema = current_schema()
                AND table_name = '{}'
            ) as exists
            "#,
            table_name
        ))
        .fetch_one(&pool)
        .await
        .unwrap();

        let exists: bool = row.get("exists");
        assert!(
            exists,
            "Table '{}' does not exist - migrations may not have run",
            table_name
        );
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'pack'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "pack table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_action_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'action'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "action table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_trigger_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'trigger'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "trigger table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_sensor_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'sensor'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "sensor table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_rule_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'rule'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "rule table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_execution_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'execution'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "execution table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_event_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'event'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "event table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_enforcement_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'enforcement'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "enforcement table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_inquiry_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'inquiry'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "inquiry table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_identity_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'identity'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "identity table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_key_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'key'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "key table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'notification'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "notification table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_runtime_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'runtime'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "runtime table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_worker_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'worker'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "worker table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_columns() {
    let pool = create_test_pool().await.unwrap();

    // Verify all expected columns exist in pack table
    let columns: Vec<String> = sqlx::query(
        r#"
        SELECT column_name
        FROM information_schema.columns
        WHERE table_schema = current_schema() AND table_name = 'pack'
        ORDER BY column_name
        "#,
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .iter()
    .map(|row| row.get("column_name"))
    .collect();

    let expected_columns = vec![
        "conf_schema",
        "config",
        "created",
        "dependencies",
        "description",
        "id",
        "is_standard",
        "label",
        "meta",
        "ref",
        "runtime_deps",
        "tags",
        "updated",
        "version",
    ];

    for col in &expected_columns {
        assert!(
            columns.contains(&col.to_string()),
            "Column '{}' not found in pack table",
            col
        );
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_action_columns() {
    let pool = create_test_pool().await.unwrap();

    // Verify all expected columns exist in action table
    let columns: Vec<String> = sqlx::query(
        r#"
        SELECT column_name
        FROM information_schema.columns
        WHERE table_schema = current_schema() AND table_name = 'action'
        ORDER BY column_name
        "#,
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .iter()
    .map(|row| row.get("column_name"))
    .collect();

    let expected_columns = vec![
        "created",
        "description",
        "entrypoint",
        "id",
        "label",
        "out_schema",
        "pack",
        "pack_ref",
        "param_schema",
        "ref",
        "runtime",
        "updated",
    ];

    for col in &expected_columns {
        assert!(
            columns.contains(&col.to_string()),
            "Column '{}' not found in action table",
            col
        );
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_timestamps_auto_populated() {
    let pool = create_test_pool().await.unwrap();
    clean_database(&pool).await.unwrap();

    // Create a pack and verify timestamps are set
    let pack = PackFixture::new("timestamp_pack")
        .create(&pool)
        .await
        .unwrap();

    // Timestamps should be set to current time
    let now = chrono::Utc::now();
    assert!(pack.created <= now);
    assert!(pack.updated <= now);
    assert!(pack.created <= pack.updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_json_column_storage() {
    let pool = create_test_pool().await.unwrap();
    clean_database(&pool).await.unwrap();

    // Create pack with JSON data
    let pack = PackFixture::new("json_pack")
        .with_description("Pack with JSON data")
        .create(&pool)
        .await
        .unwrap();

    // Verify JSON data is stored and retrieved correctly
    assert!(pack.conf_schema.is_object());
    assert!(pack.config.is_object());
    assert!(pack.meta.is_object());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_array_column_storage() {
    let pool = create_test_pool().await.unwrap();
    clean_database(&pool).await.unwrap();

    // Create pack with arrays
    let pack = PackFixture::new("array_pack")
        .with_tags(vec![
            "test".to_string(),
            "example".to_string(),
            "demo".to_string(),
        ])
        .create(&pool)
        .await
        .unwrap();

    // Verify arrays are stored correctly
    assert_eq!(pack.tags.len(), 3);
    assert!(pack.tags.contains(&"test".to_string()));
    assert!(pack.tags.contains(&"example".to_string()));
    assert!(pack.tags.contains(&"demo".to_string()));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_unique_constraints() {
    let pool = create_test_pool().await.unwrap();
    clean_database(&pool).await.unwrap();

    // Create a pack
    PackFixture::new("unique_pack").create(&pool).await.unwrap();

    // Try to create another pack with the same ref - should fail
    let result = PackFixture::new("unique_pack").create(&pool).await;

    assert!(result.is_err(), "Should not allow duplicate pack refs");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_foreign_key_constraints() {
    let pool = create_test_pool().await.unwrap();
    clean_database(&pool).await.unwrap();

    // Try to create an action with non-existent pack_id - should fail
    let result = sqlx::query(
        r#"
        INSERT INTO action (ref, pack, pack_ref, label, description, entrypoint)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind("test_pack.test_action")
    .bind(99999i64) // Non-existent pack ID
    .bind("test_pack")
    .bind("Test Action")
    .bind("Test action description")
    .bind("main.py")
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "Should not allow action with non-existent pack"
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_enum_types_exist() {
    let pool = create_test_pool().await.unwrap();

    // Check that custom enum types are created
    let enums: Vec<String> = sqlx::query(
        r#"
        SELECT typname
        FROM pg_type
        WHERE typnamespace = (SELECT oid FROM pg_namespace WHERE nspname = current_schema())
        AND typtype = 'e'
        ORDER BY typname
        "#,
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .iter()
    .map(|row| row.get("typname"))
    .collect();

    let expected_enums = vec![
        "artifact_retention_enum",
        "artifact_type_enum",
        "enforcement_condition_enum",
        "enforcement_status_enum",
        "execution_status_enum",
        "inquiry_status_enum",
        "notification_status_enum",
        "owner_type_enum",
        "policy_method_enum",
        "worker_status_enum",
        "worker_type_enum",
    ];

    for enum_type in &expected_enums {
        assert!(
            enums.contains(&enum_type.to_string()),
            "Enum type '{}' not found",
            enum_type
        );
    }
}
