//! Integration tests for API-managed pack registry index persistence.

mod helpers;

use attune_common::repositories::{
    pack_registry_index::{CreatePackRegistryIndexInput, PackRegistryIndexRepository},
    Delete, List,
};
use helpers::create_test_pool;

const STANDARD_INDEX_URL: &str = attune_common::pack_registry::STANDARD_PACK_INDEX_URL;

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn standard_index_is_managed_and_custom_indices_append() {
    let pool = create_test_pool().await.unwrap();

    let initial = PackRegistryIndexRepository::list(&pool).await.unwrap();
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].url, STANDARD_INDEX_URL);
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
        },
    )
    .await
    .unwrap();

    assert_eq!(custom.position, 1);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn deleting_standard_index_does_not_recreate_it() {
    let pool = create_test_pool().await.unwrap();

    let initial = PackRegistryIndexRepository::list(&pool).await.unwrap();
    let standard = initial
        .iter()
        .find(|index| index.url == STANDARD_INDEX_URL)
        .unwrap();

    assert!(PackRegistryIndexRepository::delete(&pool, standard.id)
        .await
        .unwrap());
    assert!(PackRegistryIndexRepository::list(&pool)
        .await
        .unwrap()
        .iter()
        .all(|index| index.url != STANDARD_INDEX_URL));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn appending_at_max_position_saturates_and_preserves_id_order() {
    let pool = create_test_pool().await.unwrap();
    sqlx::query("UPDATE pack_registry_index SET position = $1 WHERE url = $2")
        .bind(i32::MAX)
        .bind(STANDARD_INDEX_URL)
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
        },
    )
    .await
    .unwrap();

    assert_eq!(custom.position, i32::MAX);
    let ordered = PackRegistryIndexRepository::list(&pool).await.unwrap();
    assert_eq!(ordered.last().map(|index| index.id), Some(custom.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
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
