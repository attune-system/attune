//! Publication callbacks run under shared source admission. Actual reclamation
//! must use a separate graph-first GC transaction, even with a queued writer.
use attune_common::{
    config::Config,
    models::{
        Artifact, ArtifactBodyState, ArtifactClassification, ArtifactType, ArtifactVisibility,
        LogStreamBackend, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{
            ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
            CreateArtifactVersionInput,
        },
        file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        log_stream::LogStreamRepository,
        maintenance::MaintenanceRepository,
        Create, FindById,
    },
    test_database::TestDatabase,
};
use serde_json::json;
use std::time::Duration;

async fn database() -> TestDatabase {
    let config = Config::load_from_file(&format!(
        "{}/../../config.test.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    TestDatabase::create(&config.database)
        .await
        .unwrap()
        .with_cleanup_on_drop()
}

async fn artifact(db: &TestDatabase, kind: ArtifactType) -> Artifact {
    ArtifactRepository::create(
        db.pool(),
        CreateArtifactInput {
            r#ref: format!("graph_retention.{}", uuid::Uuid::new_v4().simple()),
            scope: OwnerType::System,
            owner: String::new(),
            r#type: kind,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::General,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 1,
            name: None,
            description: None,
            content_type: None,
            data: None,
        },
    )
    .await
    .unwrap()
}

fn inline(artifact: i64, value: i64) -> CreateArtifactVersionInput {
    CreateArtifactVersionInput {
        artifact,
        execution: None,
        content_type: Some("application/json".into()),
        content: None,
        content_json: Some(json!({"value":value})),
        file_path: None,
        meta: None,
        created_by: None,
    }
}

#[tokio::test]
async fn create_and_both_ready_callbacks_defer_reclamation_without_upgrading_source_admission() {
    let db = database().await;
    let ordinary = artifact(&db, ArtifactType::Other).await;
    let file = artifact(&db, ArtifactType::FileText).await;
    let log = artifact(&db, ArtifactType::FileText).await;
    let mut publication = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut publication).await.unwrap();
    // INSERT fires enforce_artifact_retention even for the FIRST version. The
    // old function attempted DELETE with zero candidates and failed right here.
    let first = ArtifactVersionRepository::create(&mut *publication, inline(ordinary.id, 1))
        .await
        .unwrap();
    let latest = ArtifactVersionRepository::create(&mut *publication, inline(ordinary.id, 2))
        .await
        .unwrap();
    let pending = ArtifactVersionRepository::create_object_pending_with_conn(
        &mut publication,
        file.id,
        None,
        "text/plain".into(),
        None,
        None,
    )
    .await
    .unwrap();
    let ready = ArtifactVersionRepository::mark_body_ready_in_transaction(
        &mut publication,
        pending.id,
        "v:owned-callback-fixture",
        4,
        &"a".repeat(64),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(ready.body_state, Some(ArtifactBodyState::Ready));
    let pending_log = ArtifactVersionRepository::create_log_pending_with_conn(
        &mut publication,
        log.id,
        &log.r#ref,
        LogStreamBackend::SharedFile,
        900001,
        json!({}),
    )
    .await
    .unwrap();
    let stream = LogStreamRepository::create_with_backend_with_conn(
        &mut publication,
        pending_log.id,
        LogStreamBackend::SharedFile,
        1024,
        500,
    )
    .await
    .unwrap();
    LogStreamRepository::seal_shared_file(&mut publication, stream.id, 4, true)
        .await
        .unwrap();
    let ready_log = ArtifactVersionRepository::mark_log_body_ready_in_transaction(
        &mut publication,
        pending_log.id,
        4,
        &"b".repeat(64),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(ready_log.body_state, Some(ArtifactBodyState::Ready));
    let gate: (bool, bool) =
        sqlx::query_as("SELECT file_evidence_graph_held(false),file_evidence_graph_held(true)")
            .fetch_one(&mut *publication)
            .await
            .unwrap();
    assert_eq!(
        gate,
        (true, false),
        "ordinary publication must not become exclusive reclamation"
    );
    let retained: i64 =
        sqlx::query_scalar("SELECT count(*) FROM artifact_version WHERE artifact=$1")
            .bind(ordinary.id)
            .fetch_one(&mut *publication)
            .await
            .unwrap();
    assert_eq!(
        retained, 2,
        "publication cannot destructively prune an older version under source locks"
    );
    assert!(
        MaintenanceRepository::claim_expired_artifact_version(&mut publication, first.id)
            .await
            .is_err(),
        "a cleanup caller may not upgrade its already-held shared source admission"
    );
    publication.commit().await.unwrap();

    let candidates = MaintenanceRepository::find_expired_artifact_versions(db.pool(), 1)
        .await
        .unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].id, first.id,
        "the supervisor must also reclaim ordinary non-log version overflow"
    );
    sqlx::query("UPDATE artifact SET retention_limit=10 WHERE id=$1")
        .bind(ordinary.id)
        .execute(db.pool())
        .await
        .unwrap();
    let mut stale = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut stale).await.unwrap();
    assert!(
        MaintenanceRepository::claim_expired_artifact_version(&mut stale, first.id)
            .await
            .unwrap()
            .is_none(),
        "an outside candidate cannot override a newer retention policy"
    );
    stale.rollback().await.unwrap();
    sqlx::query("UPDATE artifact SET retention_limit=1 WHERE id=$1")
        .bind(ordinary.id)
        .execute(db.pool())
        .await
        .unwrap();
    let mut gc = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut gc).await.unwrap();
    let current = MaintenanceRepository::claim_expired_artifact_version(&mut gc, first.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.id, first.id);
    assert_eq!(
        Evidence::request_cleanup(&mut gc, &[first.id])
            .await
            .unwrap(),
        [first.id]
    );
    assert!(ArtifactVersionRepository::delete(&mut *gc, first.id)
        .await
        .unwrap());
    gc.commit().await.unwrap();
    let kept: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM artifact_version WHERE artifact=$1 ORDER BY version")
            .bind(ordinary.id)
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(kept, [latest.id]);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn deferred_gc_refresh_preserves_latest_ready_metadata_while_a_newer_upload_is_pending() {
    let db = database().await;
    let target = artifact(&db, ArtifactType::FileText).await;
    let ready = ArtifactVersionRepository::create_object_pending(
        db.pool(),
        target.id,
        None,
        "text/plain".into(),
        None,
        None,
    )
    .await
    .unwrap();
    ArtifactVersionRepository::mark_body_ready(
        db.pool(),
        ready.id,
        "v:owned-metadata-callback",
        4,
        &"c".repeat(64),
    )
    .await
    .unwrap()
    .unwrap();
    let pending = ArtifactVersionRepository::create_object_pending(
        db.pool(),
        target.id,
        None,
        "application/octet-stream".into(),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(pending.body_state, Some(ArtifactBodyState::Pending));
    // This is the production metadata refresh called after deferred version GC.
    // An unpublished newer row must not erase the last Ready projection.
    assert!(
        !MaintenanceRepository::refresh_or_delete_artifact_metadata(db.pool(), target.id)
            .await
            .unwrap()
    );
    let current = ArtifactRepository::find_by_id(db.pool(), target.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.size_bytes, Some(4));
    assert_eq!(current.content_type.as_deref(), Some("text/plain"));
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn queued_gc_writer_does_not_make_publication_a_lock_upgrade_and_real_delete_still_denies() {
    let db = database().await;
    let target = artifact(&db, ArtifactType::Other).await;
    let mut publication = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut publication).await.unwrap();
    let first = ArtifactVersionRepository::create(&mut *publication, inline(target.id, 1))
        .await
        .unwrap();
    let writer_pool = db.pool().clone();
    let (pid_tx, pid_rx) = tokio::sync::oneshot::channel();
    let mut writer = tokio::spawn(async move {
        let mut gc = writer_pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *gc)
            .await
            .unwrap();
        pid_tx.send(pid).unwrap();
        let entered = Evidence::lock_reclamation(&mut gc).await;
        gc.rollback().await.unwrap();
        entered
    });
    let pid = pid_rx.await.unwrap();
    let queued = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND NOT granted)").bind(pid).fetch_one(db.pool()).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await;
    let published = tokio::time::timeout(
        Duration::from_secs(2),
        ArtifactVersionRepository::create(&mut *publication, inline(target.id, 2)),
    )
    .await;
    let rejected = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query("DELETE FROM artifact_version WHERE id=$1")
            .bind(first.id)
            .execute(&mut *publication),
    )
    .await;
    // Always release the reader and join the writer BEFORE inspecting results.
    publication.rollback().await.unwrap();
    let joined = tokio::time::timeout(Duration::from_secs(5), &mut writer).await;
    if joined.is_err() {
        writer.abort();
        let _ = writer.await;
    }
    db.cleanup().await.unwrap();
    assert!(
        queued.is_ok(),
        "the negative path must exercise an actually queued graph writer"
    );
    published
        .expect("publication must not wait behind the queued GC writer")
        .unwrap();
    assert!(joined.is_ok());
    joined.unwrap().unwrap().unwrap();
    let error = rejected
        .expect("source-held reclamation must reject rather than wait for a lock upgrade")
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("Reclamation must enter exclusive graph gate before source/target locks"));
}
