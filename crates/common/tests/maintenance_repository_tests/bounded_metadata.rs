use super::setup_db;
use attune_common::{
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{
            ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
            CreateArtifactVersionInput,
        },
        file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        maintenance::MaintenanceRepository,
        maintenance_admission::{MaintenanceBudget, MaintenanceOutcome},
        Create, FindById,
    },
};
use std::time::Duration;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

async fn artifact(pool: &sqlx::PgPool) -> i64 {
    ArtifactRepository::create(
        pool,
        CreateArtifactInput {
            r#ref: format!("bounded.metadata_{}", uuid::Uuid::new_v4().simple()),
            scope: OwnerType::System,
            owner: "maintenance-metadata-test".into(),
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
    .unwrap()
    .id
}

#[tokio::test]
async fn bounded_empty_artifact_deletion_defers_and_retries_without_a_queued_writer() {
    let db = setup_db().await;
    let id = artifact(db.pool()).await;
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let mut capture = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    let deferred = timeout(
        Duration::from_secs(2),
        MaintenanceRepository::refresh_or_delete_artifact_metadata_bounded(
            db.pool(),
            id,
            &budget,
            deadline(),
            &cancel,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        deferred.outcome,
        MaintenanceOutcome::LockDeferred,
        "{deferred:?}"
    );
    assert!(deferred.value.is_none());
    // Keep the original capture open. An abandoned queued writer would block
    // this second reader and recreate the foreground regression.
    let mut reader = db.pool().begin().await.unwrap();
    timeout(Duration::from_secs(2), Evidence::lock_capture(&mut reader))
        .await
        .unwrap()
        .unwrap();
    reader.rollback().await.unwrap();
    capture.rollback().await.unwrap();
    let deleted = MaintenanceRepository::refresh_or_delete_artifact_metadata_bounded(
        db.pool(),
        id,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(deleted.outcome, MaintenanceOutcome::Progress, "{deleted:?}");
    assert_eq!(deleted.value, Some(true));
    let empty = MaintenanceRepository::refresh_or_delete_artifact_metadata_bounded(
        db.pool(),
        id,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(empty.outcome, MaintenanceOutcome::NoWork, "{empty:?}");
    assert_eq!(empty.candidates, 0);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn bounded_metadata_refresh_keeps_latest_ready_version_and_pinned_empty_artifacts() {
    let db = setup_db().await;
    let id = artifact(db.pool()).await;
    ArtifactVersionRepository::create(
        db.pool(),
        CreateArtifactVersionInput {
            artifact: id,
            file_path: None,
            content: Some(vec![0; 19]),
            content_json: None,
            content_type: Some("application/owned-fixture".into()),
            execution: None,
            meta: None,
            created_by: None,
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE artifact SET size_bytes=0 WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let refreshed = MaintenanceRepository::refresh_or_delete_artifact_metadata_bounded(
        db.pool(),
        id,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(
        refreshed.outcome,
        MaintenanceOutcome::Progress,
        "{refreshed:?}"
    );
    assert_eq!(
        refreshed.value,
        Some(false),
        "false means refreshed, not deleted"
    );
    let size: Option<i64> = sqlx::query_scalar("SELECT size_bytes FROM artifact WHERE id=$1")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(size, Some(19));

    let empty = artifact(db.pool()).await;
    let mut seed = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut seed).await.unwrap();
    sqlx::query(
        "INSERT INTO file_evidence_owner(version,artifact,root_read) VALUES(99401,99402,99403)",
    )
    .execute(&mut *seed)
    .await
    .unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) VALUES(99401,'artifact',$1)")
        .bind(empty)
        .execute(&mut *seed)
        .await
        .unwrap();
    seed.commit().await.unwrap();
    let pinned = MaintenanceRepository::refresh_or_delete_artifact_metadata_bounded(
        db.pool(),
        empty,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(pinned.outcome, MaintenanceOutcome::NoWork, "{pinned:?}");
    assert_eq!(pinned.value, Some(false));
    assert!(ArtifactRepository::find_by_id(db.pool(), empty)
        .await
        .unwrap()
        .is_some());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn expired_version_and_parent_metadata_are_one_rollback_unit() {
    use attune_common::repositories::maintenance_admission::PhysicalCleanup;
    let db = setup_db().await;
    let id = artifact(db.pool()).await;
    let version = ArtifactVersionRepository::create(
        db.pool(),
        CreateArtifactVersionInput {
            artifact: id,
            file_path: None,
            content: Some(vec![0; 9]),
            content_json: None,
            content_type: None,
            execution: None,
            meta: None,
            created_by: None,
        },
    )
    .await
    .unwrap();
    sqlx::raw_sql("CREATE FUNCTION owned_expired_parent_failure() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned expired parent failure'; END $$; CREATE TRIGGER owned_expired_parent_failure BEFORE DELETE ON artifact FOR EACH ROW EXECUTE FUNCTION owned_expired_parent_failure()")
        .execute(db.pool()).await.unwrap();
    let version_id = version.id;
    let mut cleanup = PhysicalCleanup::begin(db.pool(), &MaintenanceBudget::default())
        .await
        .unwrap();
    let failed = cleanup
        .metadata(move |conn| {
            Box::pin(async move {
                MaintenanceRepository::delete_artifact_version_and_refresh_with_conn(
                    conn, version_id, id,
                )
                .await
            })
        })
        .await;
    assert!(failed.is_err());
    cleanup.rollback().await.unwrap();
    assert!(ArtifactVersionRepository::find_by_id(db.pool(), version_id)
        .await
        .unwrap()
        .is_some());
    assert!(ArtifactRepository::find_by_id(db.pool(), id)
        .await
        .unwrap()
        .is_some());
    sqlx::raw_sql("DROP TRIGGER owned_expired_parent_failure ON artifact; DROP FUNCTION owned_expired_parent_failure()")
        .execute(db.pool()).await.unwrap();
    let mut retry = PhysicalCleanup::begin(db.pool(), &MaintenanceBudget::default())
        .await
        .unwrap();
    let deleted = retry
        .metadata(move |conn| {
            Box::pin(async move {
                MaintenanceRepository::delete_artifact_version_and_refresh_with_conn(
                    conn, version_id, id,
                )
                .await
            })
        })
        .await
        .unwrap();
    assert_eq!(deleted, (true, true));
    retry.commit().await.unwrap();
    assert!(ArtifactVersionRepository::find_by_id(db.pool(), version_id)
        .await
        .unwrap()
        .is_none());
    assert!(ArtifactRepository::find_by_id(db.pool(), id)
        .await
        .unwrap()
        .is_none());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn expired_artifact_page_does_not_depend_on_the_global_reporting_count() {
    let db = setup_db().await;
    let id = artifact(db.pool()).await;
    sqlx::query("UPDATE artifact SET retention_policy='days',retention_limit=1 WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO artifact_version(artifact,version,created) SELECT $1,n,NOW()-interval '2 days'+n*interval '1 second' FROM generate_series(1,7) n")
        .bind(id).execute(db.pool()).await.unwrap();
    // A query-shape guard, not a timing/performance simulation. Keep the real
    // eligibility implementation and reject only the top-level reporting COUNT
    // in this owned database. A bounded page must not execute that query at all.
    sqlx::raw_sql("ALTER FUNCTION file_evidence_version_eligible(BIGINT) RENAME TO owned_real_version_eligibility;
        CREATE FUNCTION file_evidence_version_eligible(v BIGINT) RETURNS BOOLEAN LANGUAGE plpgsql STABLE AS $$
        BEGIN
          IF current_query() LIKE '%SELECT COUNT(*)::BIGINT FROM expired av%' THEN
            RAISE EXCEPTION 'owned global reporting count must not gate page discovery';
          END IF;
          RETURN owned_real_version_eligibility(v);
        END $$;")
        .execute(db.pool()).await.unwrap();
    let count = MaintenanceRepository::expired_artifact_version_count(db.pool()).await;
    let page = MaintenanceRepository::expired_artifact_candidates_bounded(
        db.pool(),
        2,
        &MaintenanceBudget::default(),
    )
    .await;
    sqlx::raw_sql("DROP FUNCTION file_evidence_version_eligible(BIGINT); ALTER FUNCTION owned_real_version_eligibility(BIGINT) RENAME TO file_evidence_version_eligible")
        .execute(db.pool()).await.unwrap();
    db.cleanup().await.unwrap();
    assert!(
        count.is_err(),
        "the fixture must reject the actual global count query"
    );
    let page = page.expect("a reporting-count failure must not prevent bounded page discovery");
    assert_eq!(
        page.versions.len(),
        2,
        "report only the selected page, not the seven-row backlog"
    );
    assert!(page.versions.iter().all(|version| version.artifact == id));
}

#[tokio::test]
async fn artifact_discovery_crosses_empty_pages_freezes_pass_and_wraps() {
    let db = setup_db().await;
    let id = artifact(db.pool()).await;
    sqlx::query("UPDATE artifact SET retention_policy='days',retention_limit=1 WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    // Oldest raw prefix is unexpired under a different current policy.
    let protected = artifact(db.pool()).await;
    sqlx::query("UPDATE artifact SET retention_policy='versions',retention_limit=100 WHERE id=$1")
        .bind(protected)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO artifact_version(artifact,version,created) SELECT $1,n,NOW()-interval '4 days'+n*interval '1 second' FROM generate_series(1,5) n")
        .bind(protected).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO artifact_version(artifact,version,created) SELECT $1,n,NOW()-interval '2 days'+n*interval '1 second' FROM generate_series(1,3) n")
        .bind(id).execute(db.pool()).await.unwrap();
    // A deliberately failing evidence predicate on the tail proves that the
    // raw bound precedes expensive evaluation; output LIMIT alone cannot pass.
    sqlx::raw_sql("ALTER FUNCTION file_evidence_version_eligible(BIGINT) RENAME TO owned_real_eligibility;
        CREATE FUNCTION file_evidence_version_eligible(v BIGINT) RETURNS BOOLEAN LANGUAGE plpgsql STABLE AS $$
        BEGIN RAISE EXCEPTION 'tail evidence evaluated outside raw page'; END $$;")
        .execute(db.pool()).await.unwrap();
    let budget = MaintenanceBudget::default();
    let page = MaintenanceRepository::expired_artifact_candidates_bounded(db.pool(), 2, &budget)
        .await
        .unwrap();
    assert_eq!(page.scanned, 2);
    assert!(page.versions.is_empty() && page.pending);
    sqlx::raw_sql("DROP FUNCTION file_evidence_version_eligible(BIGINT); ALTER FUNCTION owned_real_eligibility(BIGINT) RENAME TO file_evidence_version_eligible")
        .execute(db.pool()).await.unwrap();
    let high: i64 = sqlx::query_scalar("SELECT high_id FROM artifact_retention_discovery")
        .fetch_one(db.pool())
        .await
        .unwrap();
    // Restart is represented by independent repository calls: no in-memory cursor.
    // Continuous newer arrivals cannot extend this pass's frozen frontier.
    sqlx::query("INSERT INTO artifact_version(artifact,version,created) VALUES ($1,4,NOW()-interval '1 day')")
        .bind(id).execute(db.pool()).await.unwrap();
    let mut found = Vec::new();
    for _ in 0..4 {
        let page =
            MaintenanceRepository::expired_artifact_candidates_bounded(db.pool(), 2, &budget)
                .await
                .unwrap();
        found.extend(page.versions.into_iter().map(|v| v.id));
        if !page.pending {
            break;
        }
    }
    assert_eq!(found.len(), 3);
    assert!(found.iter().all(|v| *v <= high));
    // Policy changes and formerly ineligible rows are revisited after wrap.
    sqlx::query("UPDATE artifact SET retention_policy='days',retention_limit=1 WHERE id=$1")
        .bind(protected)
        .execute(db.pool())
        .await
        .unwrap();
    let page = MaintenanceRepository::expired_artifact_candidates_bounded(db.pool(), 2, &budget)
        .await
        .unwrap();
    assert_eq!(page.versions.len(), 2);
    assert!(page.versions.iter().all(|v| v.artifact == protected));
    assert!(page.pending);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn artifact_discovery_competing_scanners_and_cancelled_progress() {
    let db = setup_db().await;
    let id = artifact(db.pool()).await;
    sqlx::query("UPDATE artifact SET retention_policy='days',retention_limit=1 WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO artifact_version(artifact,version,created) SELECT $1,n,NOW()-interval '2 days' FROM generate_series(1,8) n")
        .bind(id).execute(db.pool()).await.unwrap();
    let budget = MaintenanceBudget {
        statement_timeout: Duration::from_secs(2),
        page_timeout: Duration::from_secs(4),
        ..Default::default()
    };
    let (a, b) = tokio::join!(
        MaintenanceRepository::expired_artifact_candidates_bounded(db.pool(), 2, &budget),
        MaintenanceRepository::expired_artifact_candidates_bounded(db.pool(), 2, &budget)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.versions.len() + b.versions.len(), 4);
    assert!(a
        .versions
        .iter()
        .all(|v| !b.versions.iter().any(|w| w.id == v.id)));
    let before: i64 = sqlx::query_scalar("SELECT last_id FROM artifact_retention_discovery")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let mut blocker = db.pool().begin().await.unwrap();
    sqlx::query("SELECT singleton FROM artifact_retention_discovery FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    assert!(timeout(
        Duration::from_millis(100),
        MaintenanceRepository::expired_artifact_candidates_bounded(db.pool(), 2, &budget)
    )
    .await
    .is_err());
    blocker.rollback().await.unwrap();
    let after: i64 = sqlx::query_scalar("SELECT last_id FROM artifact_retention_discovery")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(before, after, "cancelled progress must not publish a page");
    let page = MaintenanceRepository::expired_artifact_candidates_bounded(db.pool(), 2, &budget)
        .await
        .unwrap();
    assert_eq!(page.versions.len(), 2);
    assert!(page.versions.iter().all(|v| v.id > before));
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn artifact_raw_hard_cap_crosses_long_pinned_prefix_and_revisits_unpins() {
    let db = setup_db().await;
    let id = artifact(db.pool()).await;
    sqlx::query("UPDATE artifact SET retention_policy='days',retention_limit=1 WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO artifact_version(artifact,version,created) SELECT $1,n,NOW()-interval '2 days'+n*interval '1 second' FROM generate_series(1,600) n")
        .bind(id).execute(db.pool()).await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut tx).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_done,staging_done) VALUES(990031,990032,990033,true,true)")
        .execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) SELECT 990031,'artifact_version',id FROM artifact_version WHERE artifact=$1 AND version<=550")
        .bind(id).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let budget = MaintenanceBudget {
        statement_timeout: Duration::from_secs(3),
        page_timeout: Duration::from_secs(5),
        ..Default::default()
    };
    for _ in 0..2 {
        let page = MaintenanceRepository::expired_artifact_candidates_bounded(
            db.pool(),
            i64::MAX,
            &budget,
        )
        .await
        .unwrap();
        assert_eq!(page.scanned, 256);
        assert!(page.versions.is_empty() && page.pending);
    }
    let page =
        MaintenanceRepository::expired_artifact_candidates_bounded(db.pool(), i64::MAX, &budget)
            .await
            .unwrap();
    assert_eq!(page.scanned, 88);
    assert_eq!(page.versions.len(), 50);
    assert!(!page.pending);
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    sqlx::query("UPDATE file_evidence_owner SET metadata_gone=true WHERE version=990031")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT file_evidence_release(990031)")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let page = MaintenanceRepository::expired_artifact_candidates_bounded(db.pool(), 1, &budget)
        .await
        .unwrap();
    assert_eq!(page.scanned, 1);
    assert_eq!(page.versions.len(), 1);
    assert_eq!(page.versions[0].version, 1);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn artifact_discovery_unconfirmed_state_commit_returns_no_candidates() {
    let db = setup_db().await;
    let id = artifact(db.pool()).await;
    sqlx::query("UPDATE artifact SET retention_policy='days',retention_limit=1 WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO artifact_version(artifact,version,created) VALUES($1,1,NOW()-interval '2 days')")
        .bind(id).execute(db.pool()).await.unwrap();
    // Terminate this disposable fixture's scanning backend during COMMIT,
    // after it has constructed a positive eligible page but before acknowledgment.
    sqlx::raw_sql("CREATE FUNCTION owned_discovery_disconnect() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_terminate_backend(pg_backend_pid()); RETURN NEW; END $$;
        CREATE CONSTRAINT TRIGGER owned_discovery_disconnect AFTER UPDATE ON artifact_retention_discovery DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION owned_discovery_disconnect();")
        .execute(db.pool()).await.unwrap();
    assert!(MaintenanceRepository::expired_artifact_candidates_bounded(
        db.pool(),
        2,
        &MaintenanceBudget::default()
    )
    .await
    .is_err());
    sqlx::raw_sql("DROP TRIGGER owned_discovery_disconnect ON artifact_retention_discovery; DROP FUNCTION owned_discovery_disconnect();")
        .execute(db.pool()).await.unwrap();
    let page = MaintenanceRepository::expired_artifact_candidates_bounded(
        db.pool(),
        2,
        &MaintenanceBudget::default(),
    )
    .await
    .unwrap();
    assert_eq!(page.versions.len(), 1);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn artifact_discovery_late_committing_older_row_is_revisited_after_wrap() {
    let db = setup_db().await;
    let a = artifact(db.pool()).await;
    let late = artifact(db.pool()).await;
    sqlx::query("UPDATE artifact SET retention_policy='days',retention_limit=1 WHERE id=ANY($1)")
        .bind(vec![a, late])
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO artifact_version(artifact,version,created) SELECT $1,n,NOW()-interval '2 days'+n*interval '1 second' FROM generate_series(1,3) n")
        .bind(a).execute(db.pool()).await.unwrap();
    let mut writer = db.pool().begin().await.unwrap();
    let late_id:i64=sqlx::query_scalar("INSERT INTO artifact_version(artifact,version,created) VALUES($1,1,NOW()-interval '3 days') RETURNING id")
        .bind(late).fetch_one(&mut *writer).await.unwrap();
    let budget = MaintenanceBudget::default();
    let page = MaintenanceRepository::expired_artifact_candidates_bounded(db.pool(), 2, &budget)
        .await
        .unwrap();
    assert!(page.pending);
    writer.commit().await.unwrap();
    let page = MaintenanceRepository::expired_artifact_candidates_bounded(db.pool(), 2, &budget)
        .await
        .unwrap();
    assert!(!page.pending);
    assert!(page.versions.iter().all(|v| v.id != late_id));
    let page = MaintenanceRepository::expired_artifact_candidates_bounded(db.pool(), 1, &budget)
        .await
        .unwrap();
    assert_eq!(
        page.versions[0].id, late_id,
        "created/id ordering must revisit late older commits, independently of ID allocation order"
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn artifact_discovery_does_not_authorize_changed_pin_or_physical_identity() {
    let db = setup_db().await;
    let id = artifact(db.pool()).await;
    sqlx::query("UPDATE artifact SET retention_policy='days',retention_limit=1 WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    let version:i64=sqlx::query_scalar("INSERT INTO artifact_version(artifact,version,file_path,created) VALUES($1,1,'owned-before',NOW()-interval '2 days') RETURNING id")
        .bind(id).fetch_one(db.pool()).await.unwrap();
    let page = MaintenanceRepository::expired_artifact_candidates_bounded(
        db.pool(),
        1,
        &MaintenanceBudget::default(),
    )
    .await
    .unwrap();
    assert_eq!(page.versions[0].file_path.as_deref(), Some("owned-before"));
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut tx).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_done,staging_done) VALUES(990041,990042,990043,true,true)")
        .execute(&mut *tx).await.unwrap();
    sqlx::query(
        "INSERT INTO file_evidence_pin(owner,kind,source) VALUES(990041,'artifact_version',$1)",
    )
    .bind(version)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    assert!(
        MaintenanceRepository::claim_expired_artifact_version(&mut tx, version)
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    sqlx::query("UPDATE file_evidence_owner SET metadata_gone=true WHERE version=990041")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT file_evidence_release(990041)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE artifact_version SET file_path='owned-after' WHERE id=$1")
        .bind(version)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    let current = MaintenanceRepository::claim_expired_artifact_version(&mut tx, version)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        current.file_path.as_deref(),
        Some("owned-after"),
        "only freshly locked identity can be used for physical deletion"
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}
