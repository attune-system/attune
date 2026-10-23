//! Regression for mixed SQLx argument types on the identical cached advisory SQL.
use attune_common::{
    config::Config,
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, LogStreamBackend, OwnerType,
        RetentionPolicyType,
    },
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
        log_stream::LogStreamRepository,
        Create,
    },
    test_database::TestDatabase,
};

#[tokio::test]
async fn both_log_ready_paths_reuse_decimal_text_lock_and_preserve_cross_connection_exclusion() {
    let config = Config::load_from_file(&format!(
        "{}/../../config.test.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let db = TestDatabase::create(&config.database)
        .await
        .unwrap()
        .with_cleanup_on_drop();
    for backend in [
        LogStreamBackend::SharedFile,
        LogStreamBackend::ObjectSegments,
    ] {
        let reference = format!(
            "wire.{}.stdout.log",
            if backend == LogStreamBackend::SharedFile {
                "shared"
            } else {
                "object"
            }
        );
        let artifact = ArtifactRepository::create(
            db.pool(),
            CreateArtifactInput {
                r#ref: reference.clone(),
                scope: OwnerType::System,
                owner: String::new(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::RuntimeLog,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 1,
                name: None,
                description: None,
                content_type: Some("text/plain".into()),
                data: None,
            },
        )
        .await
        .unwrap();
        let mut allocation = db.pool().begin().await.unwrap();
        let version = ArtifactVersionRepository::create_log_pending_with_conn(
            &mut allocation,
            artifact.id,
            &reference,
            backend,
            777,
            serde_json::json!({}),
        )
        .await
        .unwrap();
        allocation.commit().await.unwrap();
        let stream =
            LogStreamRepository::create_with_backend(db.pool(), version.id, backend, 1024, 100)
                .await
                .unwrap();
        let mut ready = db.pool().begin().await.unwrap();
        // Prime this exact connection's statement cache using the canonical
        // TEXT argument, then execute the formerly-i64 completion path on it.
        assert!(
            ArtifactRepository::find_by_id_for_update(&mut ready, artifact.id)
                .await
                .unwrap()
                .is_some()
        );
        LogStreamRepository::lock(&mut ready, stream.id)
            .await
            .unwrap();
        LogStreamRepository::seal(&mut ready, stream.id, false)
            .await
            .unwrap();
        let digest = "0".repeat(64);
        let published = if backend == LogStreamBackend::SharedFile {
            ArtifactVersionRepository::mark_log_body_ready_in_transaction(
                &mut ready, version.id, 0, &digest,
            )
            .await
        } else {
            ArtifactVersionRepository::mark_body_ready_in_transaction(
                &mut ready,
                version.id,
                "segments:0",
                0,
                &digest,
            )
            .await
        }
        .unwrap();
        assert!(
            published.is_some(),
            "seal must publish Ready instead of UTF-8/NUL failure"
        );
        let mut competing = db.pool().begin().await.unwrap();
        let admitted: bool = sqlx::query_scalar(
            "SELECT pg_try_advisory_xact_lock(hashtext('artifact_version'),hashtext($1::text))",
        )
        .bind(artifact.id.to_string())
        .fetch_one(&mut *competing)
        .await
        .unwrap();
        assert!(
            !admitted,
            "the correction must not create a different lock identity"
        );
        ready.commit().await.unwrap();
        let admitted: bool = sqlx::query_scalar(
            "SELECT pg_try_advisory_xact_lock(hashtext('artifact_version'),hashtext($1::text))",
        )
        .bind(artifact.id.to_string())
        .fetch_one(&mut *competing)
        .await
        .unwrap();
        assert!(admitted);
        competing.rollback().await.unwrap();
    }
    db.cleanup().await.unwrap();
}
