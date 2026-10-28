//! Integration tests for API-managed pack registry index persistence.

mod helpers;

use attune_common::repositories::{
    pack_registry_index::{
        CreatePackRegistryIndexInput, PackRegistryIndexRepository, UpdatePackRegistryIndexInput,
    },
    Delete, FindById, List,
};
use helpers::create_test_pool;

#[tokio::test]
async fn standard_index_is_managed_and_custom_indices_append() {
    let pool = create_test_pool().await.unwrap();

    let initial = PackRegistryIndexRepository::list(&pool).await.unwrap();
    assert_eq!(initial.len(), 1);
    assert!(initial[0].is_standard);
    assert_eq!(initial[0].position, 0);
    assert!(initial[0].enabled);

    let custom = PackRegistryIndexRepository::create(
        &pool,
        CreatePackRegistryIndexInput {
            name: Some("Company Packs".to_string()),
            url: "https://packs.example.com/index.json".to_string(),
            position: None,
            enabled: true,
            headers: serde_json::json!("encrypted-empty-headers"),
            source_headers: serde_json::json!({}),
        },
    )
    .await
    .unwrap();

    assert_eq!(custom.position, 1);
}

#[tokio::test]
async fn deleting_standard_index_does_not_recreate_it() {
    let pool = create_test_pool().await.unwrap();

    let initial = PackRegistryIndexRepository::list(&pool).await.unwrap();
    let standard = initial.iter().find(|index| index.is_standard).unwrap();

    assert!(PackRegistryIndexRepository::delete(&pool, standard.id)
        .await
        .unwrap());
    assert!(PackRegistryIndexRepository::list(&pool)
        .await
        .unwrap()
        .iter()
        .all(|index| !index.is_standard));
}

#[tokio::test]
async fn appending_at_max_position_saturates_and_preserves_id_order() {
    let pool = create_test_pool().await.unwrap();
    sqlx::query("UPDATE pack_registry_index SET position = $1 WHERE is_standard")
        .bind(i32::MAX)
        .execute(&pool)
        .await
        .unwrap();

    let custom = PackRegistryIndexRepository::create(
        &pool,
        CreatePackRegistryIndexInput {
            name: Some("Company Packs".to_string()),
            url: "https://packs.example.com/index.json".to_string(),
            position: None,
            enabled: true,
            headers: serde_json::json!({}),
            source_headers: serde_json::json!({}),
        },
    )
    .await
    .unwrap();

    assert_eq!(custom.position, i32::MAX);
    let ordered = PackRegistryIndexRepository::list(&pool).await.unwrap();
    assert_eq!(ordered.last().map(|index| index.id), Some(custom.id));
}

#[tokio::test]
async fn header_compare_and_set_does_not_overwrite_concurrent_rotation() {
    let pool = create_test_pool().await.unwrap();
    let index = PackRegistryIndexRepository::create(
        &pool,
        CreatePackRegistryIndexInput {
            name: Some("CAS registry".to_string()),
            url: "https://cas.example.com/index.json".to_string(),
            position: None,
            enabled: true,
            headers: serde_json::json!("legacy-ciphertext"),
            source_headers: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let original = index.headers.clone();

    let rotated = serde_json::json!("rotated-ciphertext");
    let won = PackRegistryIndexRepository::compare_and_set_headers(
        &pool,
        index.id,
        &original,
        rotated.clone(),
    )
    .await
    .unwrap();
    assert_eq!(won.unwrap().headers, rotated);

    let stale_write = PackRegistryIndexRepository::compare_and_set_headers(
        &pool,
        index.id,
        &original,
        serde_json::json!("stale-migration-ciphertext"),
    )
    .await
    .unwrap();
    assert!(stale_write.is_none());

    let persisted: serde_json::Value =
        sqlx::query_scalar("SELECT headers FROM pack_registry_index WHERE id = $1")
            .bind(index.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(persisted, rotated);
}

#[tokio::test]
async fn source_headers_are_normalized_preserved_and_explicitly_cleared() {
    let pool = create_test_pool().await.unwrap();
    let index = PackRegistryIndexRepository::create(
        &pool,
        CreatePackRegistryIndexInput {
            name: None,
            url: "https://registry.example.com/index.json".to_string(),
            position: None,
            enabled: true,
            headers: serde_json::json!({}),
            source_headers: serde_json::json!({"HTTPS://ARTIFACTS.EXAMPLE.COM.:443/": "encrypted-source-headers"}),
        },
    ).await.unwrap();
    assert_eq!(
        index.source_headers,
        serde_json::json!({"https://artifacts.example.com": "encrypted-source-headers"})
    );
    let unchanged = PackRegistryIndexRepository::update(
        &pool,
        index.id,
        UpdatePackRegistryIndexInput {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(unchanged.source_headers, index.source_headers);
    let cleared = PackRegistryIndexRepository::update(
        &pool,
        index.id,
        UpdatePackRegistryIndexInput {
            source_headers: Some(serde_json::json!({})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(cleared.source_headers, serde_json::json!({}));
    let persisted = PackRegistryIndexRepository::find_by_id(&pool, index.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.source_headers, serde_json::json!({}));
    pool.cleanup().await.unwrap();
}
