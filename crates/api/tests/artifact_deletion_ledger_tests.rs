mod helpers;

use attune_common::{
    blob_store::{body_from_bytes, sha256, BlobStore, FilesystemBlobStore, ObjectKey},
    models::enums::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
        object_maintenance::ObjectMaintenanceRepository,
        Create,
    },
};
use axum::{body::Bytes, http::StatusCode};
use helpers::TestContext;

#[tokio::test]
async fn api_deletion_enqueues_object_without_deleting_it_on_request_path() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();
    let identity_id: i64 = sqlx::query_scalar("SELECT id FROM identity ORDER BY id DESC LIMIT 1")
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    let artifact = ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: format!("deletion_{}", uuid::Uuid::new_v4().simple()),
            scope: OwnerType::Identity,
            owner: identity_id.to_string(),
            r#type: ArtifactType::FileBinary,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::General,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 5,
            name: None,
            description: None,
            content_type: None,
            data: None,
        },
    )
    .await
    .unwrap();
    let version = ArtifactVersionRepository::create_object_pending(
        &ctx.pool,
        artifact.id,
        None,
        "application/octet-stream".into(),
        None,
        None,
    )
    .await
    .unwrap();
    let root = ctx.test_packs_dir.join("blobs");
    let store = FilesystemBlobStore::new(&root).unwrap();
    let key = ObjectKey::new(version.object_key.clone().unwrap()).unwrap();
    let bytes = Bytes::from_static(b"retained until supervisor");
    ObjectMaintenanceRepository::reserve_upload(&ctx.pool, key.as_str(), "artifact")
        .await
        .unwrap();
    let stored = store
        .put(&key, body_from_bytes(bytes.clone()), sha256(&bytes))
        .await
        .unwrap();
    ObjectMaintenanceRepository::record_uploaded(
        &ctx.pool,
        key.as_str(),
        stored.provider_version.as_stored(),
        stored.size as i64,
    )
    .await
    .unwrap();
    ArtifactVersionRepository::mark_body_ready(
        &ctx.pool,
        version.id,
        stored.provider_version.as_stored(),
        stored.size as i64,
        &hex::encode(stored.sha256),
    )
    .await
    .unwrap();

    let response = ctx
        .delete(
            &format!("/api/v1/artifacts/{}/versions/1", artifact.id),
            ctx.token(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(store.head(&key).await.unwrap().is_some());
    let state: String =
        sqlx::query_scalar("SELECT state FROM object_maintenance_ledger WHERE object_key = $1")
            .bind(key.as_str())
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(state, "deletion_pending");
}
