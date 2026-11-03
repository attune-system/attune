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

#[tokio::test]
async fn over_budget_first_deletion_does_not_hide_the_257th_candidate() {
    let db = database().await;
    sqlx::query("INSERT INTO object_maintenance_ledger(object_key,object_kind,provider_version,state,eligible_at) SELECT 'bounded/fair/'||n,'artifact','v:bounded','deletion_pending',NOW()-INTERVAL '1 hour' FROM generate_series(1,257) n ORDER BY n")
        .execute(db.pool()).await.unwrap();
    sqlx::raw_sql("CREATE FUNCTION owned_claim_delay() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN IF OLD.object_key='bounded/fair/1' THEN PERFORM pg_sleep(0.3); END IF; RETURN NEW; END; $$; CREATE TRIGGER owned_claim_delay BEFORE UPDATE OF state ON object_maintenance_ledger FOR EACH ROW WHEN (NEW.state='deleting') EXECUTE FUNCTION owned_claim_delay()")
        .execute(db.pool()).await.unwrap();
    let budget = MaintenanceBudget {
        statement_timeout: Duration::from_millis(50),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let cutoff = Utc::now();
    let mut claimed = std::collections::HashSet::new();
    let mut oldest = None;
    for _ in 0..270 {
        let report =
            Objects::claim_deletions_bounded(db.pool(), cutoff, 256, &budget, deadline(), &cancel)
                .await;
        assert!(
            matches!(
                report.outcome,
                MaintenanceOutcome::Progress
                    | MaintenanceOutcome::NoWork
                    | MaintenanceOutcome::DeadlineDeferred
            ),
            "{report:?}"
        );
        let rows = report.value.unwrap_or_default();
        assert_eq!(report.changed, rows.len() as u64);
        for row in rows {
            assert_ne!(row.object_key, "bounded/fair/1");
            assert_eq!(row.attempts, 1);
            assert!(
                claimed.insert(row.object_key),
                "a confirmed claim was returned twice"
            );
        }
        let age: Option<chrono::DateTime<Utc>> = sqlx::query_scalar("SELECT LEAST(deferred_since,oversized_since) FROM graph_reclamation_progress WHERE phase='object_deletions'")
            .fetch_one(db.pool()).await.unwrap();
        assert!(
            age.is_some(),
            "an acknowledged over-budget visit must persist retry progress"
        );
        if let Some(oldest) = oldest {
            assert_eq!(age, Some(oldest), "cheap claims must not erase failure age");
        } else {
            oldest = age;
        }
        if claimed.contains("bounded/fair/257") {
            break;
        }
    }
    assert_eq!(
        claimed.len(),
        256,
        "a slow first claim cannot hide the cheap tail"
    );
    assert!(oldest.is_some());
    assert!(Objects::deletion_scan_pending(db.pool()).await.unwrap());
    let slow: (String, i32) = sqlx::query_as(
        "SELECT state,attempts FROM object_maintenance_ledger WHERE object_key='bounded/fair/1'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(slow, ("deletion_pending".into(), 0));
    let repeated =
        Objects::claim_deletions_bounded(db.pool(), cutoff, 256, &budget, deadline(), &cancel)
            .await;
    assert_eq!(
        repeated.outcome,
        MaintenanceOutcome::DeadlineDeferred,
        "{repeated:?}"
    );
    assert_eq!(repeated.changed, 0);
    assert!(repeated.value.is_none());
    let age: Option<chrono::DateTime<Utc>> = sqlx::query_scalar("SELECT LEAST(deferred_since,oversized_since) FROM graph_reclamation_progress WHERE phase='object_deletions'")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(age, oldest);
    sqlx::raw_sql("DROP TRIGGER owned_claim_delay ON object_maintenance_ledger; DROP FUNCTION owned_claim_delay()")
        .execute(db.pool()).await.unwrap();
    let mut repaired = 0;
    for _ in 0..3 {
        let report = Objects::claim_deletions_bounded(
            db.pool(),
            cutoff,
            256,
            &MaintenanceBudget::default(),
            deadline(),
            &cancel,
        )
        .await;
        assert!(
            matches!(
                report.outcome,
                MaintenanceOutcome::Progress | MaintenanceOutcome::NoWork
            ),
            "{report:?}"
        );
        repaired += report.changed;
        if !Objects::deletion_scan_pending(db.pool()).await.unwrap() {
            break;
        }
    }
    assert_eq!(repaired, 1);
    let progress: (i64,Option<chrono::DateTime<Utc>>) = sqlx::query_as("SELECT probe_limit,LEAST(deferred_since,oversized_since) FROM graph_reclamation_progress WHERE phase='object_deletions'")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        progress,
        (256, None),
        "only a complete clean pass resets failure state"
    );
    assert!(!Objects::deletion_scan_pending(db.pool()).await.unwrap());
    assert_reader_admitted(db.pool()).await;
    db.cleanup().await.unwrap();
}

async fn seed_claim_cancellation(pool: &sqlx::PgPool, deferred: bool) {
    sqlx::query("INSERT INTO object_maintenance_ledger(object_key,object_kind,provider_version,state,eligible_at) SELECT 'bounded/cancel/'||n,'artifact','v:bounded','deletion_pending',NOW()-INTERVAL '1 hour' FROM generate_series(1,2) n")
        .execute(pool).await.unwrap();
    // PgSleep is the readiness barrier. The delay also keeps the statement in
    // flight until cancellation or socket disposal, not a scheduler sleep.
    sqlx::raw_sql("CREATE FUNCTION owned_claim_cancel() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(10); RETURN NEW; END; $$")
        .execute(pool).await.unwrap();
    let trigger = if deferred {
        "CREATE CONSTRAINT TRIGGER owned_claim_cancel AFTER UPDATE ON object_maintenance_ledger DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NEW.state='deleting') EXECUTE FUNCTION owned_claim_cancel()"
    } else {
        "CREATE TRIGGER owned_claim_cancel BEFORE UPDATE OF state ON object_maintenance_ledger FOR EACH ROW WHEN (NEW.state='deleting') EXECUTE FUNCTION owned_claim_cancel()"
    };
    sqlx::raw_sql(trigger).execute(pool).await.unwrap();
}

async fn await_claim_sleep(pool: &sqlx::PgPool, commit: bool) -> i32 {
    timeout(Duration::from_secs(2), async {
        loop {
            let pid: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND state='active' AND wait_event='PgSleep' AND CASE WHEN $1 THEN query='COMMIT' ELSE query LIKE 'WITH candidates AS (%' END")
                .bind(commit).fetch_optional(pool).await.unwrap();
            if let Some(pid) = pid { break pid; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap()
}

async fn assert_claim_progress_untouched(pool: &sqlx::PgPool) {
    #[derive(Debug, PartialEq, sqlx::FromRow)]
    struct ClaimProgress {
        cursor_id: i64,
        high_water: i64,
        revision: i64,
        probe_limit: i64,
        pass_deferred: bool,
        deferred_since: Option<chrono::DateTime<Utc>>,
        oversized_since: Option<chrono::DateTime<Utc>>,
    }

    let progress: ClaimProgress = sqlx::query_as("SELECT cursor_id,high_water,revision,probe_limit,pass_deferred,deferred_since,oversized_since FROM graph_reclamation_progress WHERE phase='object_deletions'")
        .fetch_one(pool).await.unwrap();
    assert_eq!(
        progress,
        ClaimProgress {
            cursor_id: 0,
            high_water: 0,
            revision: 0,
            probe_limit: 256,
            pass_deferred: false,
            deferred_since: None,
            oversized_since: None,
        }
    );
    let unchanged: i64 = sqlx::query_scalar("SELECT count(*) FROM object_maintenance_ledger WHERE state='deletion_pending' AND attempts=0")
        .fetch_one(pool).await.unwrap();
    assert_eq!(unchanged, 2);
    assert_eq!(Objects::deletion_deferred_since(pool).await.unwrap(), None);
    assert!(!Objects::deletion_scan_pending(pool).await.unwrap());
}

#[tokio::test]
async fn whole_page_deletion_timeout_needs_caller_reserve_for_failed_visit_bookkeeping() {
    let db = database().await;
    seed_claim_cancellation(db.pool(), false).await;
    let budget = MaintenanceBudget {
        statement_timeout: Duration::from_secs(1),
        page_timeout: Duration::from_millis(250),
        ..Default::default()
    };
    let cutoff = Utc::now();
    let cancel = CancellationToken::new();

    // Poll until the claim owns admission and is in its trigger. Then leave the
    // owner unpolled until its caller deadline expires. This deliberate client
    // gap tests the wall-clock budget, not sleep-based ordering of a race.
    let exhausted_end = Instant::now() + budget.page_timeout;
    let mut exhausted = Box::pin(Objects::claim_deletions_bounded(
        db.pool(),
        cutoff,
        256,
        &budget,
        exhausted_end,
        &cancel,
    ));
    let pid = tokio::select! {
        pid = await_claim_sleep(db.pool(),false) => pid,
        report = &mut exhausted => panic!("claim ended before whole-page deadline barrier: {report:?}"),
    };
    tokio::time::sleep_until(exhausted_end).await;
    let exhausted = exhausted.await;
    assert_eq!(
        exhausted.outcome,
        MaintenanceOutcome::DeadlineDeferred,
        "{exhausted:?}"
    );
    assert!(exhausted.rollback_acknowledged, "{exhausted:?}");
    assert_eq!(exhausted.candidates, 2);
    assert_eq!(exhausted.changed, 0);
    assert!(exhausted.value.is_none());
    await_backend_exit(db.pool(), pid).await;
    assert_claim_progress_untouched(db.pool()).await;
    assert_reader_admitted(db.pool()).await;

    // The statement cap exceeds the whole page, so this cannot be an early
    // statement-budget failure. Only progress-only bookkeeping may consume the
    // caller's reserve after run() ends and acknowledges the claim rollback.
    let reserved_end = Instant::now() + budget.page_timeout + 3 * budget.statement_timeout;
    let reserved =
        Objects::claim_deletions_bounded(db.pool(), cutoff, 256, &budget, reserved_end, &cancel)
            .await;
    assert_eq!(
        reserved.outcome,
        MaintenanceOutcome::DeadlineDeferred,
        "{reserved:?}"
    );
    assert!(reserved.rollback_acknowledged, "{reserved:?}");
    assert_eq!(reserved.candidates, 2);
    assert_eq!(reserved.changed, 0);
    assert!(reserved.value.is_none());
    let progress: (i64,i64,i64,i64,bool) = sqlx::query_as("SELECT cursor_id,high_water,revision,probe_limit,pass_deferred FROM graph_reclamation_progress WHERE phase='object_deletions'")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        progress.0, 0,
        "a failed multi-row probe must not skip either row"
    );
    assert!(progress.1 > 0);
    assert_eq!((progress.2, progress.3, progress.4), (1, 1, true));
    let untouched: i64 = sqlx::query_scalar("SELECT count(*) FROM object_maintenance_ledger WHERE state='deletion_pending' AND attempts=0")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        untouched, 2,
        "progress-only bookkeeping cannot claim ledger rows"
    );
    assert!(Objects::deletion_scan_pending(db.pool()).await.unwrap());
    assert!(Objects::deletion_deferred_since(db.pool())
        .await
        .unwrap()
        .is_some());
    assert_reader_admitted(db.pool()).await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn failed_singleton_deletion_stays_pending_after_cursor_wraps_to_zero() {
    let db = database().await;
    seed_claim_cancellation(db.pool(), false).await;
    sqlx::query("DELETE FROM object_maintenance_ledger WHERE object_key='bounded/cancel/2'")
        .execute(db.pool())
        .await
        .unwrap();
    let budget = MaintenanceBudget {
        statement_timeout: Duration::from_millis(50),
        ..Default::default()
    };
    let cutoff = Utc::now();
    let cancel = CancellationToken::new();
    let mut oldest = None;
    for _ in 0..2 {
        let report =
            Objects::claim_deletions_bounded(db.pool(), cutoff, 256, &budget, deadline(), &cancel)
                .await;
        assert_eq!(
            report.outcome,
            MaintenanceOutcome::DeadlineDeferred,
            "{report:?}"
        );
        assert!(report.rollback_acknowledged);
        assert!(report.value.is_none());
        assert_eq!(report.changed, 0);
        let cursor: (i64,i64) = sqlx::query_as("SELECT cursor_id,high_water FROM graph_reclamation_progress WHERE phase='object_deletions'")
            .fetch_one(db.pool()).await.unwrap();
        assert_eq!(cursor, (0, 0));
        assert!(Objects::deletion_scan_pending(db.pool()).await.unwrap());
        let age = Objects::deletion_deferred_since(db.pool()).await.unwrap();
        assert!(age.is_some());
        if oldest.is_some() {
            assert_eq!(age, oldest);
        }
        oldest = age;
    }
    sqlx::raw_sql("DROP TRIGGER owned_claim_cancel ON object_maintenance_ledger; DROP FUNCTION owned_claim_cancel()")
        .execute(db.pool()).await.unwrap();
    let repaired = Objects::claim_deletions_bounded(
        db.pool(),
        cutoff,
        256,
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
    assert_eq!(repaired.value.unwrap().len(), 1);
    assert!(!Objects::deletion_scan_pending(db.pool()).await.unwrap());
    assert_eq!(
        Objects::deletion_deferred_since(db.pool()).await.unwrap(),
        None
    );
    assert_reader_admitted(db.pool()).await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn deletion_admission_deferral_and_cancelled_claim_do_not_record_failed_visits() {
    let db = database().await;
    seed_claim_cancellation(db.pool(), false).await;
    let budget = MaintenanceBudget {
        statement_timeout: Duration::from_millis(300),
        page_timeout: Duration::from_secs(3),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let mut blocker = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut blocker).await.unwrap();
    let denied =
        Objects::claim_deletions_bounded(db.pool(), Utc::now(), 256, &budget, deadline(), &cancel)
            .await;
    deferred(&denied);
    assert_claim_progress_untouched(db.pool()).await;
    blocker.rollback().await.unwrap();

    let cancellation = async {
        let pid = await_claim_sleep(db.pool(), false).await;
        cancel.cancel();
        pid
    };
    let (report, pid) = tokio::join!(
        Objects::claim_deletions_bounded(db.pool(), Utc::now(), 256, &budget, deadline(), &cancel),
        cancellation,
    );
    assert_eq!(
        report.outcome,
        MaintenanceOutcome::DeadlineDeferred,
        "{report:?}"
    );
    assert!(report.rollback_acknowledged, "{report:?}");
    assert_eq!(report.changed, 0);
    assert!(report.value.is_none());
    await_backend_exit(db.pool(), pid).await;
    assert_claim_progress_untouched(db.pool()).await;
    assert_reader_admitted(db.pool()).await;

    let cancel = CancellationToken::new();
    let mut work = Box::pin(Objects::claim_deletions_bounded(
        db.pool(),
        Utc::now(),
        256,
        &budget,
        deadline(),
        &cancel,
    ));
    let pid = tokio::select! {
        pid = await_claim_sleep(db.pool(),false) => pid,
        report = &mut work => panic!("claim ended before owner-drop barrier: {report:?}"),
    };
    drop(work);
    await_backend_exit(db.pool(), pid).await;
    assert_claim_progress_untouched(db.pool()).await;
    assert_reader_admitted(db.pool()).await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn deletion_unknown_commit_never_records_a_failed_visit_or_returns_claims() {
    let db = database().await;
    sqlx::query("INSERT INTO object_maintenance_ledger(object_key,object_kind,provider_version,state,eligible_at) SELECT 'bounded/unknown/'||n,'artifact','v:bounded','deletion_pending',NOW()-INTERVAL '1 hour' FROM generate_series(1,2) n")
        .execute(db.pool()).await.unwrap();
    sqlx::raw_sql("CREATE FUNCTION owned_claim_commit_gate() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(hashtextextended(current_database() || ':' || current_schema() || ':owned-unknown-commit',0)); RETURN NEW; END; $$; CREATE CONSTRAINT TRIGGER owned_claim_commit_gate AFTER UPDATE ON object_maintenance_ledger DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NEW.state='deleting') EXECUTE FUNCTION owned_claim_commit_gate()")
        .execute(db.pool()).await.unwrap();
    let mut blocker = db.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended(current_database() || ':' || current_schema() || ':owned-unknown-commit',0))")
        .execute(&mut *blocker).await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let budget = MaintenanceBudget {
        statement_timeout: Duration::from_millis(300),
        page_timeout: Duration::from_secs(3),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let lose_ack = async {
        let observed = timeout(Duration::from_secs(2), async {
            loop {
                let pid: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND state='active' AND query='COMMIT' AND $1=ANY(pg_blocking_pids(pid))")
                    .bind(blocker_pid).fetch_optional(db.pool()).await.map_err(|error|error.to_string())?;
                if let Some(pid) = pid { return Ok::<_,String>(pid); }
                tokio::task::yield_now().await;
            }
        }).await.map_err(|_|"COMMIT did not reach the owned advisory gate within two seconds".to_owned()).and_then(|result|result);
        cancel.cancel();
        observed
    };
    let (report, observed) = tokio::join!(
        Objects::claim_deletions_bounded(db.pool(), Utc::now(), 256, &budget, deadline(), &cancel),
        lose_ack,
    );
    // Check the contract while COMMIT is still behind our gate, but defer any
    // panic until the gate and database fixture have been explicitly released.
    let checked = if report.outcome == MaintenanceOutcome::CommitUnknown
        && !report.rollback_acknowledged
        && report.value.is_none()
        && report.changed == 0
    {
        Ok(())
    } else {
        Err(format!(
            "uncertain COMMIT exposed claims or acknowledged rollback: {report:?}"
        ))
    };
    let released = blocker.rollback().await;
    let result = async {
        released.map_err(|error|error.to_string())?;
        let pid = observed?;
        // Socket disposal need not interrupt a running COMMIT. Once our gate is
        // released, the uncertain transaction can finish or fail without delay.
        timeout(Duration::from_secs(3), async {
            loop {
                let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1)")
                    .bind(pid).fetch_one(db.pool()).await.map_err(|error|error.to_string())?;
                if !exists { return Ok::<_,String>(()); }
                tokio::task::yield_now().await;
            }
        }).await.map_err(|_|format!("backend {pid} did not exit within three seconds after releasing owned COMMIT gate"))??;
        checked?;
        // Either original COMMIT outcome is valid. Neither authorizes another
        // progress-only failed visit or a physical cleanup descriptor.
        let progress: (i64,i64,bool,Option<chrono::DateTime<Utc>>,Option<chrono::DateTime<Utc>>) = sqlx::query_as("SELECT revision,probe_limit,pass_deferred,deferred_since,oversized_since FROM graph_reclamation_progress WHERE phase='object_deletions'")
            .fetch_one(db.pool()).await.map_err(|error|error.to_string())?;
        if !(progress.0==0 || progress.0==1)
            || (progress.1,progress.2,progress.3,progress.4)!=(256,false,None,None) {
            return Err(format!("unknown COMMIT recorded a failed visit: {progress:?}"));
        }
        let excess: i64 = sqlx::query_scalar("SELECT count(*) FROM object_maintenance_ledger WHERE attempts>1")
            .fetch_one(db.pool()).await.map_err(|error|error.to_string())?;
        if excess!=0 || Objects::deletion_deferred_since(db.pool()).await.map_err(|error|error.to_string())?.is_some() {
            return Err("unknown COMMIT retried claims or recorded failure age".to_owned());
        }
        let mut reader = db.pool().begin().await.map_err(|error|error.to_string())?;
        let admitted = timeout(Duration::from_secs(2), Evidence::lock_capture(&mut reader)).await;
        let ended = reader.rollback().await;
        admitted.map_err(|_|"reader admission timed out after releasing COMMIT gate".to_owned())?.map_err(|error|error.to_string())?;
        ended.map_err(|error|error.to_string())?;
        Ok::<_,String>(())
    }.await;
    let cleanup = db.cleanup().await;
    assert!(
        result.is_ok() && cleanup.is_ok(),
        "case={result:?}; cleanup={cleanup:?}"
    );
}
