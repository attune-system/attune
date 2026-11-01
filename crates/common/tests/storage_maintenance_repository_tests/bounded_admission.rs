use super::{
    artifact_input, create_test_pool, ArtifactRepository, ArtifactVersionRepository, Create,
    Evidence, StorageMaintenanceRepository,
};
use attune_common::repositories::maintenance_admission::{MaintenanceBudget, MaintenanceOutcome};
use attune_common::repositories::object_maintenance::ObjectMaintenanceRepository as Objects;
use chrono::{Duration as ChronoDuration, Utc};
use std::time::Duration;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

async fn pending(pool: &sqlx::PgPool, suffix: &str) -> (i64, i64) {
    let artifact = ArtifactRepository::create(pool, artifact_input(suffix))
        .await
        .unwrap();
    let version = ArtifactVersionRepository::create_object_pending(
        pool,
        artifact.id,
        None,
        "application/octet-stream".into(),
        None,
        None,
    )
    .await
    .unwrap();
    let mut seed = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut seed).await.unwrap();
    sqlx::query("UPDATE artifact_version SET body_updated=NOW()-INTERVAL '2 hours' WHERE id=$1")
        .bind(version.id)
        .execute(&mut *seed)
        .await
        .unwrap();
    // Storage ownership fixture only, not a fabricated disclosure authority.
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_key,staging_done) VALUES($1,$2,99201,$3,true)")
        .bind(version.id).bind(artifact.id).bind(&version.object_key).execute(&mut *seed).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) VALUES($1,'execution',99202)")
        .bind(version.id)
        .execute(&mut *seed)
        .await
        .unwrap();
    seed.commit().await.unwrap();
    (artifact.id, version.id)
}

async fn await_graph_waiter(pool: &sqlx::PgPool, blocker: i32) {
    timeout(Duration::from_secs(2), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND NOT granted AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(blocker).fetch_one(pool).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
}

fn queued_budget() -> MaintenanceBudget {
    MaintenanceBudget {
        queued_admission: true,
        queued_lock_timeout: Duration::from_secs(2),
        statement_timeout: Duration::from_secs(3),
        page_timeout: Duration::from_secs(4),
    }
}

#[tokio::test]
async fn bounded_pending_claim_and_delete_defer_without_releasing_evidence() {
    let db = create_test_pool().await.unwrap();
    let (_, version) = pending(db.pool(), "bounded_claim").await;
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let cutoff = Utc::now() - ChronoDuration::hours(1);
    let mut capture = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    let report = timeout(
        Duration::from_secs(2),
        StorageMaintenanceRepository::claim_abandoned_pending_bounded(
            db.pool(),
            version,
            cutoff,
            &budget,
            deadline(),
            &cancel,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        report.outcome,
        MaintenanceOutcome::LockDeferred,
        "{report:?}"
    );
    assert!(report.value.is_none());
    let group: Option<i64> =
        sqlx::query_scalar("SELECT cleanup_group FROM file_evidence_owner WHERE version=$1")
            .bind(version)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(group.is_none());
    capture.rollback().await.unwrap();
    let claim = StorageMaintenanceRepository::claim_abandoned_pending_bounded(
        db.pool(),
        version,
        cutoff,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(claim.outcome, MaintenanceOutcome::Progress, "{claim:?}");
    assert_eq!(claim.value, Some(true));

    let mut capture = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    let report = timeout(
        Duration::from_secs(2),
        StorageMaintenanceRepository::delete_cleanup_claimed_bounded(
            db.pool(),
            version,
            &budget,
            deadline(),
            &cancel,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        report.outcome,
        MaintenanceOutcome::LockDeferred,
        "{report:?}"
    );
    assert!(report.value.is_none());
    capture.rollback().await.unwrap();
    let deleted = StorageMaintenanceRepository::delete_cleanup_claimed_bounded(
        db.pool(),
        version,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(deleted.outcome, MaintenanceOutcome::Progress, "{deleted:?}");
    assert_eq!(deleted.value, Some(true));
    assert!(ArtifactVersionRepository::find_by_id(db.pool(), version)
        .await
        .unwrap()
        .is_none());
    let obligations: (bool,bool,i64) = sqlx::query_as("SELECT metadata_gone,object_done,(SELECT count(*) FROM file_evidence_pin WHERE owner=$1) FROM file_evidence_owner WHERE version=$1")
        .bind(version).fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        obligations,
        (true, false, 1),
        "metadata deletion cannot complete physical cleanup"
    );
    let mut capture = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    let empty = StorageMaintenanceRepository::delete_cleanup_claimed_bounded(
        db.pool(),
        version,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(empty.outcome, MaintenanceOutcome::NoWork, "{empty:?}");
    assert_eq!(empty.candidates, 0);
    capture.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn bounded_pending_claim_rechecks_identity_and_rolls_back_owner_claim() {
    let db = create_test_pool().await.unwrap();
    let (_, version) = pending(db.pool(), "bounded_stale_claim").await;
    let budget = queued_budget();
    let cancel = CancellationToken::new();
    let cutoff = Utc::now() - ChronoDuration::hours(1);
    let mut capture = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *capture)
        .await
        .unwrap();
    let refresh = async {
        await_graph_waiter(db.pool(), pid).await;
        sqlx::query("UPDATE artifact_version SET body_updated=NOW() WHERE id=$1")
            .bind(version)
            .execute(&mut *capture)
            .await
            .unwrap();
        capture.commit().await.unwrap();
    };
    let (report, ()) = tokio::join!(
        StorageMaintenanceRepository::claim_abandoned_pending_bounded(
            db.pool(),
            version,
            cutoff,
            &budget,
            deadline(),
            &cancel
        ),
        refresh
    );
    assert_eq!(report.outcome, MaintenanceOutcome::NoWork, "{report:?}");
    assert_eq!(report.candidates, 1);
    assert_eq!(report.value, Some(false));
    let group: Option<i64> =
        sqlx::query_scalar("SELECT cleanup_group FROM file_evidence_owner WHERE version=$1")
            .bind(version)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(
        group.is_none(),
        "a stale claim must not leave durable cleanup ownership"
    );
    let empty = StorageMaintenanceRepository::claim_abandoned_pending_bounded(
        db.pool(),
        version,
        cutoff,
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
async fn bounded_cleanup_delete_rechecks_new_pins_after_candidate_discovery() {
    let db = create_test_pool().await.unwrap();
    let (_, version) = pending(db.pool(), "bounded_late_pin").await;
    let budget = queued_budget();
    let cancel = CancellationToken::new();
    let claim = StorageMaintenanceRepository::claim_abandoned_pending_bounded(
        db.pool(),
        version,
        Utc::now(),
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(claim.value, Some(true), "{claim:?}");
    let mut capture = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *capture)
        .await
        .unwrap();
    let pin = async {
        await_graph_waiter(db.pool(), pid).await;
        sqlx::query(
            "INSERT INTO file_evidence_owner(version,artifact,root_read) VALUES(99301,99302,99303)",
        )
        .execute(&mut *capture)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO file_evidence_pin(owner,kind,source) VALUES(99301,'artifact_version',$1)",
        )
        .bind(version)
        .execute(&mut *capture)
        .await
        .unwrap();
        capture.commit().await.unwrap();
    };
    let (report, ()) = tokio::join!(
        StorageMaintenanceRepository::delete_cleanup_claimed_bounded(
            db.pool(),
            version,
            &budget,
            deadline(),
            &cancel
        ),
        pin
    );
    assert_eq!(report.outcome, MaintenanceOutcome::NoWork, "{report:?}");
    assert_eq!(report.candidates, 1);
    assert_eq!(report.value, Some(false));
    assert!(ArtifactVersionRepository::find_by_id(db.pool(), version)
        .await
        .unwrap()
        .is_some());
    let metadata_gone: bool =
        sqlx::query_scalar("SELECT metadata_gone FROM file_evidence_owner WHERE version=$1")
            .bind(version)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(!metadata_gone);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn bounded_object_claims_visit_past_256_pinned_rows_and_wrap_to_new_eligibility() {
    let db = create_test_pool().await.unwrap();
    let mut seed = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut seed).await.unwrap();
    let older: i64 = sqlx::query_scalar("INSERT INTO object_maintenance_ledger(object_key,object_kind,provider_version,state,eligible_at,updated) VALUES('bounded/fair/older','artifact','e:fair','deletion_pending',NOW()+INTERVAL '1 hour',NOW()-INTERVAL '2 hours') RETURNING id")
        .fetch_one(&mut *seed).await.unwrap();
    sqlx::query("INSERT INTO object_maintenance_ledger(object_key,object_kind,provider_version,state,eligible_at,updated) SELECT 'bounded/fair/'||n,'artifact','e:fair','deletion_pending',NOW()-INTERVAL '2 hours',NOW()-INTERVAL '2 hours' FROM generate_series(1,257) n ORDER BY n")
        .execute(&mut *seed).await.unwrap();
    // Each of the first 256 objects has an unresolved native writer and frozen
    // proof pins. Neither repeated visits nor a cursor may retire those owners.
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_key,provider_version) SELECT 99500+n,100000+n,101000+n,'bounded/fair/'||n,'e:fair' FROM generate_series(1,256) n")
        .execute(&mut *seed).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_put(attempt,owner,protocol) SELECT gen_random_uuid(),99500+n,'legacy_unknown' FROM generate_series(1,256) n")
        .execute(&mut *seed).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) SELECT 99500+n,'execution',102000+n FROM generate_series(1,256) n")
        .execute(&mut *seed).await.unwrap();
    seed.commit().await.unwrap();
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let cutoff = Utc::now();
    let skipped =
        Objects::claim_deletions_bounded(db.pool(), cutoff, 256, &budget, deadline(), &cancel)
            .await;
    assert_eq!(skipped.outcome, MaintenanceOutcome::NoWork, "{skipped:?}");
    assert_eq!(skipped.candidates, 256);
    assert!(skipped.value.unwrap().is_empty());
    let (cursor,high_water): (i64,i64) = sqlx::query_as("SELECT cursor_id,high_water FROM graph_reclamation_progress WHERE phase='object_deletions'").fetch_one(db.pool()).await.unwrap();
    assert!(cursor > older);
    assert!(high_water > cursor);

    sqlx::query(
        "UPDATE object_maintenance_ledger SET eligible_at=NOW()-INTERVAL '2 hours' WHERE id=$1",
    )
    .bind(older)
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO object_maintenance_ledger(object_key,object_kind,provider_version,state,eligible_at,updated) VALUES('bounded/fair/inserted','artifact','e:fair','deletion_pending',NOW()-INTERVAL '2 hours',NOW()-INTERVAL '2 hours')")
        .execute(db.pool()).await.unwrap();
    let tail =
        Objects::claim_deletions_bounded(db.pool(), cutoff, 256, &budget, deadline(), &cancel)
            .await;
    assert_eq!(tail.outcome, MaintenanceOutcome::Progress, "{tail:?}");
    let tail = tail.value.unwrap();
    assert_eq!(
        tail.len(),
        1,
        "new insertions cannot extend the frozen pass"
    );
    assert_eq!(tail[0].object_key, "bounded/fair/257");

    let wrapped =
        Objects::claim_deletions_bounded(db.pool(), cutoff, 1, &budget, deadline(), &cancel).await;
    assert_eq!(wrapped.outcome, MaintenanceOutcome::Progress, "{wrapped:?}");
    assert_eq!(
        wrapped.value.unwrap()[0].id,
        older,
        "a formerly ineligible old ID is revisited after wrap"
    );
    let skipped_again =
        Objects::claim_deletions_bounded(db.pool(), cutoff, 256, &budget, deadline(), &cancel)
            .await;
    assert_eq!(
        skipped_again.outcome,
        MaintenanceOutcome::NoWork,
        "{skipped_again:?}"
    );
    assert_eq!(skipped_again.candidates, 256);
    let inserted =
        Objects::claim_deletions_bounded(db.pool(), cutoff, 256, &budget, deadline(), &cancel)
            .await;
    assert_eq!(
        inserted.outcome,
        MaintenanceOutcome::Progress,
        "{inserted:?}"
    );
    assert_eq!(
        inserted.value.unwrap()[0].object_key,
        "bounded/fair/inserted"
    );
    let pinned: (i64,i64) = sqlx::query_as("SELECT count(*),sum(attempts)::bigint FROM object_maintenance_ledger WHERE object_key LIKE 'bounded/fair/%' AND state='deletion_pending'").fetch_one(db.pool()).await.unwrap();
    assert_eq!(pinned, (256, 0));
    let puts: i64 = sqlx::query_scalar("SELECT count(*) FROM file_evidence_put")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(puts, 256);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn vanished_object_tail_completes_the_cursor_and_revisits_a_newly_eligible_head() {
    let db = create_test_pool().await.unwrap();
    sqlx::query("INSERT INTO object_maintenance_ledger(object_key,object_kind,provider_version,state,eligible_at,updated) SELECT 'bounded/vanished/'||n,'artifact','e:vanished','deletion_pending',NOW()-INTERVAL '2 hours',NOW()-INTERVAL '2 hours' FROM generate_series(1,3) n ORDER BY n")
        .execute(db.pool()).await.unwrap();
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let cutoff = Utc::now();
    let first =
        Objects::claim_deletions_bounded(db.pool(), cutoff, 1, &budget, deadline(), &cancel).await;
    assert_eq!(first.outcome, MaintenanceOutcome::Progress, "{first:?}");
    let first = first.value.unwrap()[0].id;
    assert!(Objects::deletion_scan_pending(db.pool()).await.unwrap());
    // These ledger-only fixtures have no stored bodies. Simulate another owner
    // finishing all remaining rows before the continuation discovers its tail.
    let mut removed = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut removed).await.unwrap();
    sqlx::query("DELETE FROM object_maintenance_ledger WHERE id>$1")
        .bind(first)
        .execute(&mut *removed)
        .await
        .unwrap();
    removed.commit().await.unwrap();
    let mut capture = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    let deferred =
        Objects::claim_deletions_bounded(db.pool(), cutoff, 1, &budget, deadline(), &cancel).await;
    assert_eq!(
        deferred.outcome,
        MaintenanceOutcome::LockDeferred,
        "{deferred:?}"
    );
    assert!(
        Objects::deletion_scan_pending(db.pool()).await.unwrap(),
        "discovery alone cannot complete a durable pass"
    );
    capture.rollback().await.unwrap();
    let empty =
        Objects::claim_deletions_bounded(db.pool(), cutoff, 1, &budget, deadline(), &cancel).await;
    assert_eq!(empty.outcome, MaintenanceOutcome::NoWork, "{empty:?}");
    assert_eq!(empty.changed, 0);
    assert_eq!(empty.candidates, 0);
    assert!(empty.value.unwrap().is_empty());
    assert!(!Objects::deletion_scan_pending(db.pool()).await.unwrap());

    sqlx::query(
        "UPDATE object_maintenance_ledger SET updated=NOW()-INTERVAL '2 hours' WHERE id=$1",
    )
    .bind(first)
    .execute(db.pool())
    .await
    .unwrap();
    let retry =
        Objects::claim_deletions_bounded(db.pool(), cutoff, 1, &budget, deadline(), &cancel).await;
    assert_eq!(retry.outcome, MaintenanceOutcome::Progress, "{retry:?}");
    let retry = retry.value.unwrap();
    assert_eq!(retry[0].id, first);
    assert_eq!(retry[0].attempts, 2);
    assert!(!Objects::deletion_scan_pending(db.pool()).await.unwrap());
    db.cleanup().await.unwrap();
}

async fn claimed_pending(pool: &sqlx::PgPool, suffix: &str) -> (i64, i64) {
    let (artifact, version) = pending(pool, suffix).await;
    let claim = StorageMaintenanceRepository::claim_abandoned_pending_bounded(
        pool,
        version,
        Utc::now(),
        &MaintenanceBudget::default(),
        deadline(),
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(claim.value, Some(true), "{claim:?}");
    (artifact, version)
}

async fn assert_physical_obligations(pool: &sqlx::PgPool, version: i64, metadata_gone: bool) {
    let obligations: (bool,bool,i64) = sqlx::query_as("SELECT metadata_gone,object_done,(SELECT count(*) FROM file_evidence_pin WHERE owner=$1) FROM file_evidence_owner WHERE version=$1")
        .bind(version).fetch_one(pool).await.unwrap();
    assert_eq!(obligations, (metadata_gone, false, 1));
}

#[tokio::test]
async fn combined_cleanup_rolls_back_when_artifact_lock_defers_and_retries_the_whole_unit() {
    let db = create_test_pool().await.unwrap();
    let (artifact, version) = claimed_pending(db.pool(), "atomic_parent_conflict").await;
    // Hold only the parent row, not graph admission or its allocation advisory
    // lock. This must exercise the new unit's artifact-before-version ordering.
    let mut blocker = db.pool().begin().await.unwrap();
    sqlx::query("SELECT id FROM artifact WHERE id=$1 FOR UPDATE")
        .bind(artifact)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let budget = MaintenanceBudget {
        statement_timeout: Duration::from_millis(500),
        page_timeout: Duration::from_secs(2),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let observe = async {
        timeout(Duration::from_secs(2),async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE NOT granted AND locktype IN ('transactionid','tuple') AND $1=ANY(pg_blocking_pids(pid)))")
                    .bind(blocker_pid).fetch_one(db.pool()).await.unwrap();
                if waiting { break; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        let mut version_check = db.pool().begin().await.unwrap();
        sqlx::query("SELECT id FROM artifact_version WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(version)
            .execute(&mut *version_check)
            .await
            .unwrap();
        version_check.rollback().await.unwrap();
    };
    let (report, ()) = tokio::join!(
        StorageMaintenanceRepository::delete_cleanup_claimed_and_refresh_bounded(
            db.pool(),
            version,
            &budget,
            deadline(),
            &cancel
        ),
        observe
    );
    assert!(
        matches!(
            report.outcome,
            MaintenanceOutcome::DeadlineDeferred | MaintenanceOutcome::LockDeferred
        ),
        "{report:?}"
    );
    assert!(report.rollback_acknowledged, "{report:?}");
    assert!(report.value.is_none());
    assert!(ArtifactVersionRepository::find_by_id(db.pool(), version)
        .await
        .unwrap()
        .is_some());
    assert_physical_obligations(db.pool(), version, false).await;
    blocker.rollback().await.unwrap();
    let retry = StorageMaintenanceRepository::delete_cleanup_claimed_and_refresh_bounded(
        db.pool(),
        version,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(retry.outcome, MaintenanceOutcome::Progress, "{retry:?}");
    assert_eq!(retry.value, Some(true));
    let remaining: (bool,bool) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM artifact_version WHERE id=$1),EXISTS(SELECT 1 FROM artifact WHERE id=$2)")
        .bind(version).bind(artifact).fetch_one(db.pool()).await.unwrap();
    assert_eq!(remaining, (false, false));
    assert_physical_obligations(db.pool(), version, true).await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn combined_cleanup_rolls_back_version_triggers_when_parent_cleanup_fails() {
    let db = create_test_pool().await.unwrap();
    let (artifact, version) = claimed_pending(db.pool(), "atomic_parent_failure").await;
    let failure = format!("CREATE FUNCTION owned_parent_cleanup_failure() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN IF NOT EXISTS(SELECT 1 FROM file_evidence_owner WHERE version={version} AND metadata_gone) OR NOT EXISTS(SELECT 1 FROM file_evidence_pin WHERE owner={version}) THEN RAISE EXCEPTION 'owned expected version cleanup bookkeeping'; END IF; RAISE EXCEPTION 'owned post-delete failure'; END; $$; CREATE TRIGGER owned_parent_cleanup_failure BEFORE DELETE ON artifact FOR EACH ROW EXECUTE FUNCTION owned_parent_cleanup_failure()");
    sqlx::raw_sql(&failure).execute(db.pool()).await.unwrap();
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let report = StorageMaintenanceRepository::delete_cleanup_claimed_and_refresh_bounded(
        db.pool(),
        version,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(report.outcome, MaintenanceOutcome::Failed, "{report:?}");
    assert!(report.rollback_acknowledged);
    assert!(report.value.is_none());
    match report.error.as_ref() {
        Some(attune_common::Error::Database(sqlx::Error::Database(error))) => {
            assert_eq!(error.message(), "owned post-delete failure")
        }
        _ => panic!("expected the parent failure after version triggers ran: {report:?}"),
    }
    let remaining: (bool,bool) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM artifact_version WHERE id=$1),EXISTS(SELECT 1 FROM artifact WHERE id=$2)")
        .bind(version).bind(artifact).fetch_one(db.pool()).await.unwrap();
    assert_eq!(remaining, (true, true));
    assert_physical_obligations(db.pool(), version, false).await;
    sqlx::raw_sql("DROP TRIGGER owned_parent_cleanup_failure ON artifact; DROP FUNCTION owned_parent_cleanup_failure()")
        .execute(db.pool()).await.unwrap();
    let retry = StorageMaintenanceRepository::delete_cleanup_claimed_and_refresh_bounded(
        db.pool(),
        version,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(retry.value, Some(true), "{retry:?}");
    assert_physical_obligations(db.pool(), version, true).await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn combined_cleanup_keeps_parent_metadata_for_a_remaining_non_ready_version() {
    let db = create_test_pool().await.unwrap();
    let (artifact, version) = claimed_pending(db.pool(), "atomic_pending_sibling").await;
    let sibling = ArtifactVersionRepository::create_object_pending(
        db.pool(),
        artifact,
        None,
        "application/octet-stream".into(),
        None,
        None,
    )
    .await
    .unwrap();
    let report = StorageMaintenanceRepository::delete_cleanup_claimed_and_refresh_bounded(
        db.pool(),
        version,
        &MaintenanceBudget::default(),
        deadline(),
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(report.value, Some(true), "{report:?}");
    assert!(ArtifactVersionRepository::find_by_id(db.pool(), version)
        .await
        .unwrap()
        .is_none());
    assert!(ArtifactVersionRepository::find_by_id(db.pool(), sibling.id)
        .await
        .unwrap()
        .is_some());
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifact WHERE id=$1)")
        .bind(artifact)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(
        exists,
        "no ready frontier does not mean the artifact is empty"
    );
    assert_physical_obligations(db.pool(), version, true).await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn combined_noop_delete_does_not_commit_incidental_trigger_writes() {
    let db = create_test_pool().await.unwrap();
    let (artifact, version) = claimed_pending(db.pool(), "atomic_noop_trigger").await;
    let before: Option<i64> = sqlx::query_scalar("SELECT size_bytes FROM artifact WHERE id=$1")
        .bind(artifact)
        .fetch_one(db.pool())
        .await
        .unwrap();
    sqlx::raw_sql("CREATE FUNCTION owned_suppress_cleanup() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN UPDATE artifact SET size_bytes=711 WHERE id=OLD.artifact; RETURN NULL; END; $$; CREATE TRIGGER zz_owned_suppress_cleanup BEFORE DELETE ON artifact_version FOR EACH ROW EXECUTE FUNCTION owned_suppress_cleanup()")
        .execute(db.pool()).await.unwrap();
    let report = StorageMaintenanceRepository::delete_cleanup_claimed_and_refresh_bounded(
        db.pool(),
        version,
        &MaintenanceBudget::default(),
        deadline(),
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(report.outcome, MaintenanceOutcome::NoWork, "{report:?}");
    assert_eq!(report.value, Some(false));
    assert_eq!(report.changed, 0);
    let after: Option<i64> = sqlx::query_scalar("SELECT size_bytes FROM artifact WHERE id=$1")
        .bind(artifact)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        after, before,
        "a suppressed version deletion must roll back its trigger writes"
    );
    assert!(ArtifactVersionRepository::find_by_id(db.pool(), version)
        .await
        .unwrap()
        .is_some());
    assert_physical_obligations(db.pool(), version, false).await;
    db.cleanup().await.unwrap();
}

async fn shared_pending(
    pool: &sqlx::PgPool,
    suffix: &str,
) -> attune_common::repositories::storage_maintenance::SharedLogBodyCandidate {
    let artifact = ArtifactRepository::create(pool, artifact_input(suffix))
        .await
        .unwrap();
    let execution: i64 = sqlx::query_scalar("INSERT INTO execution(action_ref,status) VALUES('core.owned_shared_cleanup','failed') RETURNING id")
        .fetch_one(pool).await.unwrap();
    let version = ArtifactVersionRepository::create_log_pending(
        pool,
        artifact.id,
        &artifact.r#ref,
        super::LogStreamBackend::SharedFile,
        "text/plain".into(),
        Some(execution),
        None,
        None,
    )
    .await
    .unwrap();
    super::LogStreamRepository::create_with_backend(
        pool,
        version.id,
        super::LogStreamBackend::SharedFile,
        1024,
        1000,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE artifact_version SET body_updated=NOW()-interval '2 hours' WHERE id=$1")
        .bind(version.id)
        .execute(pool)
        .await
        .unwrap();
    let mut seed = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut seed).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,staging_path,object_done) VALUES($1,$2,99601,$3,true)")
        .bind(version.id).bind(artifact.id).bind(&version.file_path).execute(&mut *seed).await.unwrap();
    seed.commit().await.unwrap();
    attune_common::repositories::storage_maintenance::SharedLogBodyCandidate {
        id: version.id,
        artifact: artifact.id,
        file_path: version.file_path.unwrap(),
    }
}

#[tokio::test]
async fn shared_cleanup_claim_defers_then_commits_the_fresh_path_and_atomic_completion() {
    let db = create_test_pool().await.unwrap();
    let candidate = shared_pending(db.pool(), "bounded_shared").await;
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let mut capture = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    let report = timeout(
        Duration::from_secs(2),
        StorageMaintenanceRepository::claim_abandoned_shared_log_pending_bounded(
            db.pool(),
            candidate.clone(),
            Utc::now(),
            &budget,
            deadline(),
            &cancel,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        report.outcome,
        MaintenanceOutcome::LockDeferred,
        "{report:?}"
    );
    assert!(
        report.into_result().is_err(),
        "deferral cannot become empty success"
    );
    let mut reader = db.pool().begin().await.unwrap();
    timeout(Duration::from_secs(1), Evidence::lock_capture(&mut reader))
        .await
        .unwrap()
        .unwrap();
    reader.rollback().await.unwrap();
    capture.rollback().await.unwrap();
    let report = StorageMaintenanceRepository::claim_abandoned_shared_log_pending_bounded(
        db.pool(),
        candidate.clone(),
        Utc::now(),
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(report.outcome, MaintenanceOutcome::Progress, "{report:?}");
    assert_eq!(
        report.value.unwrap().unwrap().file_path,
        candidate.file_path
    );
    let completed = StorageMaintenanceRepository::delete_cleanup_claimed_and_refresh_bounded(
        db.pool(),
        candidate.id,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(completed.value, Some(true), "{completed:?}");
    let remaining: (bool,bool) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM artifact_version WHERE id=$1),EXISTS(SELECT 1 FROM artifact WHERE id=$2)")
        .bind(candidate.id).bind(candidate.artifact).fetch_one(db.pool()).await.unwrap();
    assert_eq!(remaining, (false, false));
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn shared_cleanup_stale_identity_rolls_back_the_evidence_group() {
    let db = create_test_pool().await.unwrap();
    let candidate = shared_pending(db.pool(), "bounded_shared_stale").await;
    let id = candidate.id;
    let mut capture = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *capture)
        .await
        .unwrap();
    let update = async {
        await_graph_waiter(db.pool(), pid).await;
        sqlx::query("UPDATE artifact_version SET file_path='logs/owned-new-path.txt' WHERE id=$1")
            .bind(id)
            .execute(&mut *capture)
            .await
            .unwrap();
        capture.commit().await.unwrap();
    };
    let budget = queued_budget();
    let cancel = CancellationToken::new();
    let (report, ()) = tokio::join!(
        StorageMaintenanceRepository::claim_abandoned_shared_log_pending_bounded(
            db.pool(),
            candidate,
            Utc::now(),
            &budget,
            deadline(),
            &cancel
        ),
        update
    );
    assert_eq!(report.outcome, MaintenanceOutcome::NoWork, "{report:?}");
    assert!(report.value.unwrap().is_none());
    let state: (String,Option<i64>) = sqlx::query_as("SELECT av.body_state::text,o.cleanup_group FROM artifact_version av JOIN file_evidence_owner o ON o.version=av.id WHERE av.id=$1")
        .bind(id).fetch_one(db.pool()).await.unwrap();
    assert_eq!(state, ("pending".into(), None));
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn shared_cleanup_unknown_commit_never_returns_physical_deletion_authority() {
    let db = create_test_pool().await.unwrap();
    let candidate = shared_pending(db.pool(), "bounded_shared_unknown").await;
    let id = candidate.id;
    sqlx::raw_sql("CREATE FUNCTION owned_deferred_shared_commit() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(2); RETURN NEW; END $$; CREATE CONSTRAINT TRIGGER owned_deferred_shared_commit AFTER UPDATE ON artifact_version DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NEW.body_state='cleanup_claimed' AND OLD.body_state IS DISTINCT FROM NEW.body_state) EXECUTE FUNCTION owned_deferred_shared_commit()")
        .execute(db.pool()).await.unwrap();
    let budget = MaintenanceBudget {
        statement_timeout: Duration::from_millis(150),
        page_timeout: Duration::from_secs(1),
        ..Default::default()
    };
    let report = StorageMaintenanceRepository::claim_abandoned_shared_log_pending_bounded(
        db.pool(),
        candidate,
        Utc::now(),
        &budget,
        deadline(),
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(
        report.outcome,
        MaintenanceOutcome::CommitUnknown,
        "{report:?}"
    );
    assert!(report.value.is_none());
    assert!(!report.rollback_acknowledged);
    assert!(report.into_result().is_err());
    let state: (String,Option<i64>) = sqlx::query_as("SELECT av.body_state::text,o.cleanup_group FROM artifact_version av JOIN file_evidence_owner o ON o.version=av.id WHERE av.id=$1")
        .bind(id).fetch_one(db.pool()).await.unwrap();
    assert_eq!(state, ("pending".into(), None));
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn ready_mismatch_cleanup_defers_and_rechecks_the_identity_observed_before_head() {
    let db = create_test_pool().await.unwrap();
    let (artifact, version) = pending(db.pool(), "bounded_ready_mismatch").await;
    ArtifactVersionRepository::mark_body_ready(db.pool(), version, "e:owned", 9, &"a".repeat(64))
        .await
        .unwrap()
        .unwrap();
    let candidate = StorageMaintenanceRepository::ready_objects(db.pool(), 0, 10)
        .await
        .unwrap()
        .remove(0);
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let mut capture = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    let deferred = timeout(
        Duration::from_secs(2),
        StorageMaintenanceRepository::delete_mismatched_ready_and_refresh_bounded(
            db.pool(),
            candidate.clone(),
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
    let mut reader = db.pool().begin().await.unwrap();
    timeout(Duration::from_secs(1), Evidence::lock_capture(&mut reader))
        .await
        .unwrap()
        .unwrap();
    reader.rollback().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *capture)
        .await
        .unwrap();
    let update = async {
        await_graph_waiter(db.pool(), pid).await;
        sqlx::query("UPDATE artifact_version SET meta='{}' WHERE id=$1")
            .bind(version)
            .execute(&mut *capture)
            .await
            .unwrap();
        capture.commit().await.unwrap();
    };
    let queued = queued_budget();
    let (stale, ()) = tokio::join!(
        StorageMaintenanceRepository::delete_mismatched_ready_and_refresh_bounded(
            db.pool(),
            candidate,
            &queued,
            deadline(),
            &cancel
        ),
        update
    );
    assert_eq!(stale.value, Some(false), "{stale:?}");
    let group: Option<i64> =
        sqlx::query_scalar("SELECT cleanup_group FROM file_evidence_owner WHERE version=$1")
            .bind(version)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(group.is_none());
    let fresh = StorageMaintenanceRepository::ready_objects(db.pool(), 0, 10)
        .await
        .unwrap()
        .remove(0);
    let deleted = StorageMaintenanceRepository::delete_mismatched_ready_and_refresh_bounded(
        db.pool(),
        fresh,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(deleted.value, Some(true), "{deleted:?}");
    let remaining: (bool,bool) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM artifact_version WHERE id=$1),EXISTS(SELECT 1 FROM artifact WHERE id=$2)")
        .bind(version).bind(artifact).fetch_one(db.pool()).await.unwrap();
    assert_eq!(remaining, (false, false));
    assert_physical_obligations(db.pool(), version, true).await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn legacy_snapshot_admission_is_bounded_and_rechecks_path_expiry_and_pins() {
    use attune_common::repositories::maintenance_admission::PhysicalCleanup;
    let db = create_test_pool().await.unwrap();
    let artifact = ArtifactRepository::create(db.pool(), artifact_input("bounded_snapshot"))
        .await
        .unwrap();
    let version: i64 = sqlx::query_scalar("INSERT INTO artifact_version(artifact,version,file_path,legacy_snapshot_expires_at) VALUES($1,1,'owned-snapshot.txt',NOW()-interval '1 day') RETURNING id")
        .bind(artifact.id).fetch_one(db.pool()).await.unwrap();
    let pack: i64 = sqlx::query_scalar("INSERT INTO pack(ref,label,version) VALUES('owned_snapshot','owned_snapshot','1.0.0') RETURNING id").fetch_one(db.pool()).await.unwrap();
    let release: i64 = sqlx::query_scalar("INSERT INTO pack_release(pack,pack_ref,version,digest,archive_path,content_path,archive_size,manifest,legacy_snapshot_expires_at) VALUES($1,'owned_snapshot','1.0.0',repeat('a',64),'owned-snapshot.tar','owned-content',0,'{}',NOW()-interval '1 day') RETURNING id")
        .bind(pack).fetch_one(db.pool()).await.unwrap();
    let budget = MaintenanceBudget::default();
    let artifact_candidate =
        StorageMaintenanceRepository::expired_snapshots_bounded(db.pool(), false, 10, &budget)
            .await
            .unwrap()
            .remove(0);
    let pack_candidate =
        StorageMaintenanceRepository::expired_snapshots_bounded(db.pool(), true, 10, &budget)
            .await
            .unwrap()
            .remove(0);
    let mut capture = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    assert!(timeout(
        Duration::from_secs(2),
        PhysicalCleanup::begin(db.pool(), &budget)
    )
    .await
    .unwrap()
    .is_err());
    let mut reader = db.pool().begin().await.unwrap();
    timeout(Duration::from_secs(1), Evidence::lock_capture(&mut reader))
        .await
        .unwrap()
        .unwrap();
    reader.rollback().await.unwrap();
    sqlx::query("UPDATE artifact_version SET file_path='owned-replaced-snapshot.txt' WHERE id=$1")
        .bind(version)
        .execute(&mut *capture)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE pack_release SET legacy_snapshot_expires_at=NOW()+interval '1 day' WHERE id=$1",
    )
    .bind(release)
    .execute(&mut *capture)
    .await
    .unwrap();
    capture.commit().await.unwrap();
    for (candidate, pack) in [(artifact_candidate, false), (pack_candidate, true)] {
        let mut cleanup = PhysicalCleanup::begin(db.pool(), &budget).await.unwrap();
        let current = cleanup
            .metadata(move |conn| {
                Box::pin(async move {
                    StorageMaintenanceRepository::clear_expired_snapshot_with_conn(
                        conn, candidate, pack,
                    )
                    .await
                })
            })
            .await
            .unwrap();
        assert!(!current, "stale path/expiry must never authorize unlink");
        cleanup.rollback().await.unwrap();
    }
    let candidate =
        StorageMaintenanceRepository::expired_snapshots_bounded(db.pool(), false, 10, &budget)
            .await
            .unwrap()
            .remove(0);
    let mut pin = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut pin).await.unwrap();
    sqlx::query(
        "INSERT INTO file_evidence_owner(version,artifact,root_read) VALUES(99701,99702,99703)",
    )
    .execute(&mut *pin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO file_evidence_pin(owner,kind,source) VALUES(99701,'artifact_version',$1)",
    )
    .bind(version)
    .execute(&mut *pin)
    .await
    .unwrap();
    pin.commit().await.unwrap();
    let mut cleanup = PhysicalCleanup::begin(db.pool(), &budget).await.unwrap();
    assert!(!cleanup
        .metadata(move |conn| Box::pin(async move {
            StorageMaintenanceRepository::clear_expired_snapshot_with_conn(conn, candidate, false)
                .await
        }))
        .await
        .unwrap());
    cleanup.rollback().await.unwrap();
    let path: Option<String> =
        sqlx::query_scalar("SELECT file_path FROM artifact_version WHERE id=$1")
            .bind(version)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(path.as_deref(), Some("owned-replaced-snapshot.txt"));
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn physical_cleanup_metadata_deadline_rolls_back_and_releases_admission() {
    use attune_common::repositories::maintenance_admission::PhysicalCleanup;
    let db = create_test_pool().await.unwrap();
    let budget = MaintenanceBudget {
        statement_timeout: Duration::from_millis(100),
        page_timeout: Duration::from_millis(200),
        ..Default::default()
    };
    let mut cleanup = PhysicalCleanup::begin(db.pool(), &budget).await.unwrap();
    let result = cleanup
        .metadata(|conn| {
            Box::pin(async move {
                sqlx::query("SELECT pg_sleep(2)").execute(conn).await?;
                Ok(())
            })
        })
        .await;
    assert!(result.is_err());
    cleanup
        .rollback()
        .await
        .expect("rollback must be acknowledged after statement cancellation");
    let mut reader = db.pool().begin().await.unwrap();
    timeout(Duration::from_secs(1), Evidence::lock_capture(&mut reader))
        .await
        .unwrap()
        .unwrap();
    reader.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn physical_cleanup_queued_admission_restores_the_inherited_lock_timeout() {
    use attune_common::repositories::maintenance_admission::PhysicalCleanup;
    let db = create_test_pool().await.unwrap();
    let artifact =
        ArtifactRepository::create(db.pool(), artifact_input("physical_inherited_timeout"))
            .await
            .unwrap();
    let schema = format!("\"{}\"", db.schema().replace('"', "\"\""));
    let isolated = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |conn, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path',$1,false),set_config('lock_timeout','10ms',false)")
                    .bind(schema).execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(db.pool().connect_options().as_ref().clone())
        .await.unwrap();
    let mut blocker = db.pool().begin().await.unwrap();
    sqlx::query("SELECT id FROM artifact WHERE id=$1 FOR UPDATE")
        .bind(artifact.id)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let budget = MaintenanceBudget {
        queued_admission: true,
        queued_lock_timeout: Duration::from_millis(50),
        statement_timeout: Duration::from_secs(1),
        page_timeout: Duration::from_secs(2),
    };
    let mut cleanup = PhysicalCleanup::begin(&isolated, &budget).await.unwrap();
    let restored = cleanup
        .metadata(|conn| {
            Box::pin(async move {
                Ok(
                    sqlx::query_scalar::<_, String>("SELECT current_setting('lock_timeout')")
                        .fetch_one(conn)
                        .await?,
                )
            })
        })
        .await
        .unwrap();
    let id = artifact.id;
    let blocked = cleanup
        .metadata(move |conn| {
            Box::pin(async move {
                Ok(
                    sqlx::query_scalar::<_, i64>("SELECT id FROM artifact WHERE id=$1 FOR UPDATE")
                        .bind(id)
                        .fetch_one(conn)
                        .await?,
                )
            })
        })
        .await;
    cleanup.rollback().await.unwrap();
    blocker.rollback().await.unwrap();
    isolated.close().await;
    db.cleanup().await.unwrap();
    assert_eq!(
        restored, "10ms",
        "queued admission must restore the inherited setting"
    );
    match blocked {
        Err(attune_common::Error::Database(sqlx::Error::Database(error))) => {
            assert_eq!(
                error.code().as_deref(),
                Some("55P03"),
                "row contention must use the restored lock timeout, not the statement deadline"
            );
        }
        other => panic!("expected a lock timeout from the held artifact row: {other:?}"),
    }
}
