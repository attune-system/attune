use super::{database, Evidence};
use attune_common::repositories::{
    maintenance_admission::{self, MaintenanceBudget, MaintenanceOutcome, MaintenanceResult},
    object_maintenance::ObjectMaintenanceRepository as Objects,
};
use chrono::Utc;
use std::time::Duration;
use tokio::{
    sync::oneshot,
    time::{timeout, Instant},
};
use tokio_util::sync::CancellationToken;

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

fn deferred<T>(result: &MaintenanceResult<T>) {
    assert_eq!(result.outcome, MaintenanceOutcome::LockDeferred);
    assert!(result.candidates > 0);
    assert_eq!(result.changed, 0);
    assert!(
        result.value.is_none(),
        "deferral must not return an empty successful page"
    );
}

async fn assert_reader_admitted(pool: &sqlx::PgPool) {
    let mut reader = pool.begin().await.unwrap();
    timeout(Duration::from_secs(2), Evidence::lock_capture(&mut reader))
        .await
        .unwrap()
        .unwrap();
    reader.rollback().await.unwrap();
}

#[tokio::test]
async fn empty_metadata_passes_never_queue_behind_capture() {
    let db = database().await;
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let mut blocker = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut blocker).await.unwrap();
    let audit = timeout(
        Duration::from_secs(2),
        Evidence::audit_pending_bounded(db.pool(), 8, &budget, deadline(), &cancel),
    )
    .await
    .unwrap();
    let recovery = timeout(
        Duration::from_secs(2),
        Evidence::recovery_candidates_bounded(db.pool(), 8, &budget, deadline(), &cancel),
    )
    .await
    .unwrap();
    let claims = timeout(
        Duration::from_secs(2),
        Objects::claim_deletions_bounded(db.pool(), Utc::now(), 8, &budget, deadline(), &cancel),
    )
    .await
    .unwrap();
    assert_eq!(audit.outcome, MaintenanceOutcome::NoWork);
    assert_eq!(recovery.outcome, MaintenanceOutcome::NoWork);
    assert_eq!(claims.outcome, MaintenanceOutcome::NoWork);
    assert_eq!(
        (audit.candidates, recovery.candidates, claims.candidates),
        (0, 0, 0)
    );
    assert_reader_admitted(db.pool()).await;
    blocker.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn metadata_candidates_defer_explicitly_and_retry_after_capture_ends() {
    let db = database().await;
    let mut seed = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut seed).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,evidence_state) VALUES(98001,98002,98003,'pending_audit')")
        .execute(&mut *seed).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_key,metadata_gone) VALUES(98011,98012,98013,'bounded/recovery',true)")
        .execute(&mut *seed).await.unwrap();
    sqlx::query(
        "INSERT INTO file_evidence_put(attempt,owner,protocol) VALUES($1,98011,'atomic_create_v1')",
    )
    .bind(uuid::Uuid::new_v4())
    .execute(&mut *seed)
    .await
    .unwrap();
    seed.commit().await.unwrap();
    Objects::reserve_upload(db.pool(), "bounded/recovery", "artifact")
        .await
        .unwrap();
    Objects::reserve_upload(db.pool(), "bounded/delete", "artifact")
        .await
        .unwrap();
    sqlx::query("UPDATE object_maintenance_ledger SET state='deletion_pending',provider_version='v:bounded',eligible_at=NOW()-INTERVAL '1 hour' WHERE object_key='bounded/delete'")
        .execute(db.pool()).await.unwrap();
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let mut blocker = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut blocker).await.unwrap();
    deferred(&Evidence::audit_pending_bounded(db.pool(), 8, &budget, deadline(), &cancel).await);
    deferred(&Evidence::readmit_bounded(db.pool(), 98001, &budget, deadline(), &cancel).await);
    deferred(
        &Evidence::recovery_candidates_bounded(db.pool(), 8, &budget, deadline(), &cancel).await,
    );
    deferred(
        &Objects::claim_deletions_bounded(db.pool(), Utc::now(), 8, &budget, deadline(), &cancel)
            .await,
    );
    let state: String =
        sqlx::query_scalar("SELECT evidence_state FROM file_evidence_owner WHERE version=98001")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(state, "pending_audit");
    assert_reader_admitted(db.pool()).await;
    blocker.rollback().await.unwrap();
    let audit = Evidence::audit_pending_bounded(db.pool(), 8, &budget, deadline(), &cancel).await;
    assert_eq!(audit.outcome, MaintenanceOutcome::Progress, "{audit:?}");
    assert_eq!(audit.changed, 1);
    assert_eq!(
        audit.value,
        Some(0),
        "invalid evidence is quarantined, never admitted"
    );
    let recovery =
        Evidence::recovery_candidates_bounded(db.pool(), 8, &budget, deadline(), &cancel).await;
    assert_eq!(
        recovery.outcome,
        MaintenanceOutcome::Progress,
        "{recovery:?}"
    );
    let plans = recovery.value.unwrap();
    assert_eq!(plans.len(), 1);
    let directory = tempfile::tempdir().unwrap();
    let store = attune_common::blob_store::FilesystemBlobStore::new(directory.path()).unwrap();
    let settled = attune_common::blob_store::settle_atomic_file_put(
        &store,
        &plans[0].key().unwrap(),
        plans[0].token(),
    )
    .await
    .unwrap();
    let mut blocker = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut blocker).await.unwrap();
    deferred(
        &Evidence::complete_put_recovery_bounded(
            db.pool(),
            &plans[0],
            &settled,
            &budget,
            deadline(),
            &cancel,
        )
        .await,
    );
    let unresolved: i64 =
        sqlx::query_scalar("SELECT count(*) FROM file_evidence_put WHERE owner=98011")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(unresolved, 1);
    blocker.rollback().await.unwrap();
    let completed = Evidence::complete_put_recovery_bounded(
        db.pool(),
        &plans[0],
        &settled,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(
        completed.outcome,
        MaintenanceOutcome::Progress,
        "{completed:?}"
    );
    assert_eq!(completed.value, Some(true));
    let claims =
        Objects::claim_deletions_bounded(db.pool(), Utc::now(), 8, &budget, deadline(), &cancel)
            .await;
    assert_eq!(claims.outcome, MaintenanceOutcome::Progress, "{claims:?}");
    let entries = claims.value.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].object_key, "bounded/delete");
    let mut blocker = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut blocker).await.unwrap();
    deferred(
        &Objects::complete_delete_bounded(db.pool(), entries[0].id, &budget, deadline(), &cancel)
            .await,
    );
    blocker.rollback().await.unwrap();
    let completed =
        Objects::complete_delete_bounded(db.pool(), entries[0].id, &budget, deadline(), &cancel)
            .await;
    assert_eq!(
        completed.outcome,
        MaintenanceOutcome::Progress,
        "{completed:?}"
    );
    assert_eq!(completed.value, Some(true));
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn aged_queued_admission_is_bounded_and_does_not_strand_a_writer() {
    let db = database().await;
    Objects::reserve_upload(db.pool(), "bounded/queued", "artifact")
        .await
        .unwrap();
    sqlx::query("UPDATE object_maintenance_ledger SET state='deletion_pending',provider_version='v:bounded',eligible_at=NOW()-INTERVAL '1 hour'")
        .execute(db.pool()).await.unwrap();
    let budget = MaintenanceBudget {
        queued_admission: true,
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let mut blocker = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut blocker).await.unwrap();
    let report = timeout(
        Duration::from_secs(2),
        Objects::claim_deletions_bounded(db.pool(), Utc::now(), 8, &budget, deadline(), &cancel),
    )
    .await
    .unwrap();
    deferred(&report);
    assert_reader_admitted(db.pool()).await;
    blocker.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn deletion_claim_rechecks_identity_after_waiting_for_admission() {
    let db = database().await;
    Objects::reserve_upload(db.pool(), "bounded/rechecked", "artifact")
        .await
        .unwrap();
    sqlx::query("UPDATE object_maintenance_ledger SET state='deletion_pending',provider_version='v:bounded',eligible_at=NOW()-INTERVAL '1 hour'")
        .execute(db.pool()).await.unwrap();
    let budget = MaintenanceBudget {
        queued_admission: true,
        queued_lock_timeout: Duration::from_secs(2),
        statement_timeout: Duration::from_secs(3),
        page_timeout: Duration::from_secs(4),
    };
    let cancel = CancellationToken::new();
    let mut blocker = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut blocker).await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let change = async {
        timeout(Duration::from_secs(2), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND NOT granted AND $1=ANY(pg_blocking_pids(pid)))")
                    .bind(blocker_pid).fetch_one(db.pool()).await.unwrap();
                if waiting { break; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        // Keep the row eligible but change its MVCC identity after discovery.
        sqlx::query("UPDATE object_maintenance_ledger SET attempts=attempts+1 WHERE object_key='bounded/rechecked'")
            .execute(&mut *blocker).await.unwrap();
        blocker.commit().await.unwrap();
    };
    let (report, ()) = tokio::join!(
        Objects::claim_deletions_bounded(db.pool(), Utc::now(), 8, &budget, deadline(), &cancel),
        change
    );
    assert_eq!(report.outcome, MaintenanceOutcome::NoWork, "{report:?}");
    assert_eq!(report.candidates, 1);
    assert!(report.value.unwrap().is_empty());
    let retry =
        Objects::claim_deletions_bounded(db.pool(), Utc::now(), 8, &budget, deadline(), &cancel)
            .await;
    assert_eq!(retry.outcome, MaintenanceOutcome::Progress, "{retry:?}");
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn statement_timeout_rolls_back_metadata_and_claims_together() {
    let db = database().await;
    Objects::reserve_upload(db.pool(), "bounded/atomic", "artifact")
        .await
        .unwrap();
    let budget = MaintenanceBudget {
        statement_timeout: Duration::from_millis(50),
        ..Default::default()
    };
    let report = maintenance_admission::run(db.pool(), &budget, deadline(), &CancellationToken::new(),
        |conn| Box::pin(async move { Ok(sqlx::query_scalar::<_,i64>("SELECT id FROM object_maintenance_ledger").fetch_all(conn).await?) }),
        |conn, candidates| Box::pin(async move {
            sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read) VALUES(98101,98102,98103)").execute(&mut *conn).await?;
            sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) VALUES(98101,'execution',98104)").execute(&mut *conn).await?;
            sqlx::query("UPDATE object_maintenance_ledger SET state='deleting',provider_version='v:bounded',attempts=attempts+1 WHERE id=$1").bind(candidates[0]).execute(&mut *conn).await?;
            sqlx::query("SELECT pg_sleep(10)").execute(conn).await?;
            Ok(((),1))
        }),
    ).await;
    assert_eq!(
        report.outcome,
        MaintenanceOutcome::DeadlineDeferred,
        "{report:?}"
    );
    assert!(report.value.is_none());
    let state: (String, i32) =
        sqlx::query_as("SELECT state,attempts FROM object_maintenance_ledger")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(state, ("uploading".into(), 0));
    let owners: i64 =
        sqlx::query_scalar("SELECT count(*) FROM file_evidence_owner WHERE version=98101")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(owners, 0);
    assert_reader_admitted(db.pool()).await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn stale_upload_completion_has_the_same_bounded_admission_contract() {
    let db = database().await;
    Objects::reserve_upload(db.pool(), "bounded/absent", "artifact")
        .await
        .unwrap();
    let (id, observed): (i64, chrono::DateTime<Utc>) =
        sqlx::query_as("SELECT id,updated FROM object_maintenance_ledger")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let mut blocker = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut blocker).await.unwrap();
    deferred(
        &Objects::remove_stale_upload_bounded(
            db.pool(),
            id,
            observed,
            &budget,
            deadline(),
            &cancel,
        )
        .await,
    );
    blocker.rollback().await.unwrap();
    let report =
        Objects::remove_stale_upload_bounded(db.pool(), id, observed, &budget, deadline(), &cancel)
            .await;
    assert_eq!(report.outcome, MaintenanceOutcome::Progress, "{report:?}");
    assert_eq!(report.value, Some(true));
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn page_deadline_bounds_client_gaps_and_sql_failures_are_not_empty_passes() {
    let db = database().await;
    let budget = MaintenanceBudget {
        page_timeout: Duration::from_millis(200),
        ..Default::default()
    };
    let (entered, ready) = oneshot::channel();
    let cancel = CancellationToken::new();
    let work = maintenance_admission::run(
        db.pool(),
        &budget,
        deadline(),
        &cancel,
        |_| Box::pin(async { Ok(vec![()]) }),
        move |_, _| {
            Box::pin(async move {
                entered.send(()).unwrap();
                std::future::pending::<()>().await;
                Ok(((), 0))
            })
        },
    );
    let (report, entered) = tokio::join!(work, ready);
    entered.unwrap();
    assert_eq!(
        report.outcome,
        MaintenanceOutcome::DeadlineDeferred,
        "{report:?}"
    );
    assert_reader_admitted(db.pool()).await;
    let failed = maintenance_admission::run(
        db.pool(),
        &MaintenanceBudget::default(),
        deadline(),
        &cancel,
        |_| Box::pin(async { Ok(vec![()]) }),
        |conn, _| {
            Box::pin(async move {
                sqlx::query("SELECT 1/0").execute(conn).await?;
                Ok(((), 0))
            })
        },
    )
    .await;
    assert_eq!(failed.outcome, MaintenanceOutcome::Failed, "{failed:?}");
    assert!(failed.value.is_none());
    assert_reader_admitted(db.pool()).await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn cancellation_awaits_rollback_and_dropped_owner_never_returns_a_holder() {
    let db = database().await;
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    let (admitted, ready) = oneshot::channel();
    let work = maintenance_admission::run(
        db.pool(),
        &budget,
        deadline(),
        &cancel,
        |_| Box::pin(async { Ok(vec![()]) }),
        move |conn, _| {
            Box::pin(async move {
                let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                    .fetch_one(conn)
                    .await?;
                admitted.send(pid).unwrap();
                std::future::pending::<()>().await;
                Ok(((), 0))
            })
        },
    );
    let cancellation = async {
        let pid = ready.await.unwrap();
        signal.cancel();
        pid
    };
    let (report, pid) = tokio::join!(work, cancellation);
    assert_eq!(
        report.outcome,
        MaintenanceOutcome::DeadlineDeferred,
        "{report:?}"
    );
    assert_reader_admitted(db.pool()).await;
    await_backend_exit(db.pool(), pid).await;

    drop_owned_maintenance(db.pool()).await;
    db.cleanup().await.unwrap();
}

async fn drop_owned_maintenance(pool: &sqlx::PgPool) {
    let (admitted, ready) = oneshot::channel();
    let cancel = CancellationToken::new();
    let budget = MaintenanceBudget::default();
    let mut work = Box::pin(maintenance_admission::run(
        pool,
        &budget,
        deadline(),
        &cancel,
        |_| Box::pin(async { Ok(vec![()]) }),
        move |conn, _| {
            Box::pin(async move {
                let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                    .fetch_one(conn)
                    .await?;
                admitted.send(pid).unwrap();
                std::future::pending::<()>().await;
                Ok(((), 0))
            })
        },
    ));
    let pid = tokio::select! {
        pid = ready => pid.unwrap(),
        _ = &mut work => panic!("maintenance ended before the cancellation barrier"),
    };
    drop(work);
    await_backend_exit(pool, pid).await;
    assert_reader_admitted(pool).await;
}

async fn await_backend_exit(pool: &sqlx::PgPool, pid: i32) {
    timeout(Duration::from_secs(3), async {
        loop {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1)")
                    .bind(pid)
                    .fetch_one(pool)
                    .await
                    .unwrap();
            if !exists {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn seed_pending_audits(pool: &sqlx::PgPool, base: i64, count: i64) {
    let mut seed = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut seed).await.unwrap();
    // Deliberately incomplete historical reservations. A real audit must
    // quarantine them, but a timeout must leave the owner pending and intact.
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,evidence_state) SELECT $1+n,$1+100+n,$1+200+n,'pending_audit' FROM generate_series(1,$2::bigint) n")
        .bind(base).bind(count).execute(&mut *seed).await.unwrap();
    seed.commit().await.unwrap();
}

#[tokio::test]
async fn audit_owner_commits_survive_a_later_page_deadline() {
    let db = database().await;
    seed_pending_audits(db.pool(), 99700, 5).await;
    // Each owner is cheaper than a statement budget. Their combined cost is
    // larger than the page budget. This is an elapsed-time contract, not a sleep
    // used to order a race: prefix commits must survive the later deadline.
    sqlx::raw_sql("CREATE FUNCTION owned_audit_delay() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(0.2); RETURN NEW; END; $$; CREATE TRIGGER owned_audit_delay BEFORE UPDATE OF evidence_state ON file_evidence_owner FOR EACH ROW WHEN (NEW.evidence_state='quarantined') EXECUTE FUNCTION owned_audit_delay()")
        .execute(db.pool()).await.unwrap();
    let budget = MaintenanceBudget {
        page_timeout: Duration::from_millis(600),
        ..Default::default()
    };
    let report = Evidence::audit_pending_bounded(
        db.pool(),
        5,
        &budget,
        deadline(),
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(
        report.outcome,
        MaintenanceOutcome::DeadlineDeferred,
        "{report:?}"
    );
    assert!(
        report.changed > 0 && report.changed < 5,
        "each owner, not the whole page, is the commit unit: {report:?}"
    );
    let quarantined: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM file_evidence_owner WHERE evidence_state='quarantined'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        quarantined, report.changed as i64,
        "only acknowledged owner commits contribute progress"
    );
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM file_evidence_owner WHERE evidence_state='pending_audit'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(pending, 5 - quarantined);
    assert!(Evidence::audit_scan_pending(db.pool()).await.unwrap());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn over_budget_first_audit_owner_does_not_hide_cheap_owners_or_clear_failure_age() {
    let db = database().await;
    seed_pending_audits(db.pool(), 99800, 3).await;
    sqlx::raw_sql("CREATE FUNCTION owned_audit_delay() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN IF NEW.version=99801 THEN PERFORM pg_sleep(0.3); END IF; RETURN NEW; END; $$; CREATE TRIGGER owned_audit_delay BEFORE UPDATE OF evidence_state ON file_evidence_owner FOR EACH ROW WHEN (NEW.evidence_state='quarantined') EXECUTE FUNCTION owned_audit_delay()")
        .execute(db.pool()).await.unwrap();
    let budget = MaintenanceBudget {
        statement_timeout: Duration::from_millis(50),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let report = Evidence::audit_pending_bounded(db.pool(), 8, &budget, deadline(), &cancel).await;
    assert_eq!(
        report.outcome,
        MaintenanceOutcome::DeadlineDeferred,
        "{report:?}"
    );
    assert_eq!(
        report.changed, 2,
        "cheap owners after the slow head must commit: {report:?}"
    );
    let first: String =
        sqlx::query_scalar("SELECT evidence_state FROM file_evidence_owner WHERE version=99801")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        first, "pending_audit",
        "timeout is not invalid-evidence quarantine authority"
    );
    let cheap: i64 = sqlx::query_scalar("SELECT count(*) FROM file_evidence_owner WHERE version>99801 AND evidence_state='quarantined'").fetch_one(db.pool()).await.unwrap();
    assert_eq!(cheap, 2);
    let oldest = Evidence::audit_deferred_since(db.pool()).await.unwrap();
    assert!(
        oldest.is_some(),
        "later cheap commits cannot hide the unresolved failure"
    );
    assert!(Evidence::audit_scan_pending(db.pool()).await.unwrap());
    let repeated =
        Evidence::audit_pending_bounded(db.pool(), 8, &budget, deadline(), &cancel).await;
    assert_eq!(
        repeated.outcome,
        MaintenanceOutcome::DeadlineDeferred,
        "{repeated:?}"
    );
    assert_eq!(
        Evidence::audit_deferred_since(db.pool()).await.unwrap(),
        oldest
    );
    sqlx::raw_sql(
        "DROP TRIGGER owned_audit_delay ON file_evidence_owner; DROP FUNCTION owned_audit_delay()",
    )
    .execute(db.pool())
    .await
    .unwrap();
    let repaired = Evidence::audit_pending_bounded(
        db.pool(),
        8,
        &MaintenanceBudget::default(),
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(
        repaired.outcome,
        MaintenanceOutcome::Progress,
        "{repaired:?}"
    );
    assert_eq!(repaired.changed, 1);
    assert_eq!(
        Evidence::audit_deferred_since(db.pool()).await.unwrap(),
        None
    );
    assert!(!Evidence::audit_scan_pending(db.pool()).await.unwrap());
    db.cleanup().await.unwrap();
}
