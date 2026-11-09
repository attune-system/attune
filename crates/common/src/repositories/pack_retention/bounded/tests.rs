use super::*;
use crate::{
    config::Config, repositories::schema_type::SchemaTypeRepository, test_database::TestDatabase,
};

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

fn options() -> GraphReclamationOptions {
    GraphReclamationOptions {
        statement_timeout: Duration::from_secs(5),
        page_timeout: Duration::from_secs(15),
        cycle_timeout: Duration::from_secs(60),
        queued_lock_timeout: Duration::from_secs(3),
        ..GraphReclamationOptions::default()
    }
}

async fn run(pool: &PgPool, options: &GraphReclamationOptions) -> GraphReclamationResult {
    // These tests inspect individual cursor steps, deliberately forcing one
    // page even when the production scheduler would wait for its next deadline.
    collect(
        pool,
        Utc::now(),
        0,
        options,
        CollectionMode::SinglePage { force: true },
    )
    .await
    .unwrap()
}

fn epoch(result: &GraphReclamationResult) -> &GraphReclamationPhaseResult {
    result
        .phases
        .iter()
        .find(|phase| phase.phase == GraphReclamationPhase::CacheEpochs)
        .unwrap()
}

async fn seed(pool: &PgPool, ids: &[i64]) {
    // Dormant historical identities with no raw workflow iteration or frontier.
    // 06014 drops 01017's iteration FK so this history can outlive its raw owner.
    sqlx::query("INSERT INTO runtime_cache_iteration_epoch(iteration,creation_transaction) SELECT id,'owned-collector-fixture' FROM unnest($1::bigint[]) id")
        .bind(ids).execute(pool).await.unwrap();
}

async fn remaining(pool: &PgPool) -> Vec<i64> {
    sqlx::query_scalar("SELECT iteration FROM runtime_cache_iteration_epoch ORDER BY iteration")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn make_due(pool: &PgPool, aged: bool) {
    sqlx::query("UPDATE graph_reclamation_progress SET next_attempt=clock_timestamp(),deferred_since=CASE WHEN $1 THEN clock_timestamp()-interval '10 minutes' ELSE deferred_since END WHERE phase='cache_epochs'")
        .bind(aged).execute(pool).await.unwrap();
}

async fn pin(conn: &mut PgConnection, id: i64) {
    // No physical bytes exist in this fixture. The real cleanup obligation tests
    // live in file_evidence_retention_tests; this owner exercises the same pins.
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_done,staging_done) VALUES(990001,990002,990003,true,true)")
        .execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) VALUES(990001,'runtime_cache_iteration_epoch',$1)")
        .bind(id).execute(conn).await.unwrap();
}

async fn unpin(pool: &PgPool) {
    let mut tx = pool.begin().await.unwrap();
    SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    sqlx::query("SELECT file_evidence_request_cleanup(ARRAY[990001]::bigint[])")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE file_evidence_owner SET metadata_gone=true WHERE version=990001")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT file_evidence_release(990001)")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn delete_barrier(pool: &PgPool, key: i64) {
    delete_barrier_for(pool, key, None).await;
}

async fn delete_barrier_for(pool: &PgPool, key: i64, leaf: Option<i64>) {
    sqlx::query(&format!("CREATE FUNCTION collector_test_barrier() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN OLD; END; $$"))
        .execute(pool).await.unwrap();
    let condition = leaf
        .map(|id| format!("WHEN (OLD.iteration={id})"))
        .unwrap_or_default();
    sqlx::query(&format!("CREATE TRIGGER z_collector_test_barrier BEFORE DELETE ON runtime_cache_iteration_epoch FOR EACH ROW {condition} EXECUTE FUNCTION collector_test_barrier()"))
        .execute(pool).await.unwrap();
}

async fn epoch_progress(pool: &PgPool) -> GraphReclamationProgress {
    super::super::PackRetentionRepository::reclamation_progress(pool)
        .await
        .unwrap()
        .into_iter()
        .find(|phase| phase.phase == "cache_epochs")
        .unwrap()
}

async fn waiting(pool: &PgPool, mode: &str, key: Option<i64>) -> i32 {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let pid: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_locks WHERE database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND locktype='advisory' AND NOT granted AND mode=$1 AND ($2::bigint IS NULL OR objid::bigint=$2) LIMIT 1")
            .bind(mode).bind(key).fetch_optional(pool).await.unwrap();
        if let Some(pid) = pid {
            return pid;
        }
        assert!(
            Instant::now() < end,
            "collector actor never reached its owned advisory wait"
        );
        tokio::task::yield_now().await;
    }
}

async fn waiting_for_collector(
    pool: &PgPool,
    mode: &str,
    key: Option<i64>,
    collector: &mut tokio::task::JoinHandle<GraphReclamationResult>,
) -> i32 {
    tokio::select! {
        pid=waiting(pool,mode,key)=>pid,
        result=collector=>panic!("collector completed before its expected barrier: {result:?}"),
    }
}

async fn graph_available(pool: &PgPool) {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let mut tx = pool.begin().await.unwrap();
        let available: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock_shared(hashtextextended(current_schema() || ':named-type-graph',0))")
            .fetch_one(&mut *tx).await.unwrap();
        tx.rollback().await.unwrap();
        if available {
            return;
        }
        assert!(
            Instant::now() < end,
            "cancelled collector stranded graph admission"
        );
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn independent_phase_failure_keeps_other_confirmed_progress() {
    let db = database().await;
    seed(db.pool(), &[11]).await;
    // owner_id/policy_revision are deliberately plain IDs in 20261014000003.
    // The accepted-input admission FK below still targets this real inserted row.
    sqlx::query("INSERT INTO runtime_authority_admission(owner_kind,owner_id,policy_revision,authority,authority_digest) VALUES('execution',990011,990012,'{}',repeat('a',64))")
        .execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO runtime_accepted_input(execution,admission,body,body_digest,encryption_key_hash) SELECT owner_id,id,'{}',repeat('b',64),'owned-collector-fixture' FROM runtime_authority_admission WHERE owner_id=990011")
        .execute(db.pool()).await.unwrap();
    sqlx::query("CREATE FUNCTION collector_test_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned phase failure'; END; $$")
        .execute(db.pool()).await.unwrap();
    sqlx::query("CREATE TRIGGER z_collector_test_failure BEFORE DELETE ON runtime_cache_iteration_epoch FOR EACH ROW EXECUTE FUNCTION collector_test_failure()")
        .execute(db.pool()).await.unwrap();
    let result = run(db.pool(), &options()).await;
    assert_eq!(epoch(&result).outcome, GraphReclamationOutcome::Failed);
    assert_eq!(epoch(&result).changed, 0);
    let accepted = result
        .phases
        .iter()
        .find(|phase| phase.phase == GraphReclamationPhase::AcceptedInputs)
        .unwrap();
    assert_eq!(accepted.outcome, GraphReclamationOutcome::Progress);
    assert_eq!(
        accepted.changed, 1,
        "earlier committed phases survive a later failure"
    );
    let admissions = result
        .phases
        .iter()
        .find(|phase| phase.phase == GraphReclamationPhase::Admissions)
        .unwrap();
    assert_eq!(admissions.outcome, GraphReclamationOutcome::Progress);
    assert_eq!(admissions.changed, 1);
    assert!(!result.complete());
    assert_eq!(remaining(db.pool()).await, vec![11]);
    let persisted: (i64,bool,bool) = sqlx::query_as("SELECT cursor_id,deferred_since IS NOT NULL,last_progress IS NOT NULL FROM graph_reclamation_progress WHERE phase='cache_epochs'")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(persisted, (0, true, false));
    let progress = epoch_progress(db.pool()).await;
    assert_eq!(
        (progress.cursor, progress.high_water, progress.probe_limit),
        (0, 0, 256)
    );
    assert!(
        !progress.pass_deferred && progress.oversized_since.is_none(),
        "generic SQL errors must not change visit progress"
    );
    graph_available(db.pool()).await;
    sqlx::query("DROP TRIGGER z_collector_test_failure ON runtime_cache_iteration_epoch")
        .execute(db.pool())
        .await
        .unwrap();
    make_due(db.pool(), false).await;
    let next = run(db.pool(), &options()).await;
    assert_eq!(epoch(&next).changed, 1, "{next:?}");
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn raw_pages_cross_pinned_prefix_and_wrap_even_with_new_arrivals() {
    let db = database().await;
    seed(db.pool(), &[11, 12]).await;
    let mut tx = db.pool().begin().await.unwrap();
    SchemaTypeRepository::lock_selection(&mut tx).await.unwrap();
    pin(&mut tx, 11).await;
    tx.commit().await.unwrap();
    let options = GraphReclamationOptions {
        candidate_limit: 1,
        delete_limit: 1,
        ..options()
    };
    let first = run(db.pool(), &options).await;
    assert_eq!(
        (
            epoch(&first).candidates,
            epoch(&first).changed,
            epoch(&first).cursor
        ),
        (1, 0, 11)
    );
    seed(db.pool(), &[13]).await;
    let second = run(db.pool(), &options).await;
    assert_eq!((epoch(&second).changed, epoch(&second).cursor), (1, 12));
    // The durable pass ends at its ORIGINAL upper bound, not the growing tail.
    let third = run(db.pool(), &options).await;
    assert_eq!((epoch(&third).candidates, epoch(&third).cursor), (0, 0));
    unpin(db.pool()).await;
    let fourth = run(db.pool(), &options).await;
    assert_eq!(epoch(&fourth).changed, 1);
    assert_eq!(remaining(db.pool()).await, vec![13]);
    assert_eq!(epoch(&run(db.pool(), &options).await).changed, 1);
    assert!(remaining(db.pool()).await.is_empty());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn admission_first_rechecks_new_pin_after_queued_gate_with_fresh_snapshot() {
    let db = database().await;
    seed(db.pool(), &[21]).await;
    make_due(db.pool(), true).await;
    let mut capture = db.pool().begin().await.unwrap();
    SchemaTypeRepository::lock_selection(&mut capture)
        .await
        .unwrap();
    let pool = db.pool().clone();
    let collector = tokio::spawn(async move { run(&pool, &options()).await });
    waiting(db.pool(), "ExclusiveLock", None).await;
    // The collector already discovered this identity. This new pin must be seen
    // by a statement AFTER admission, not the queued lock statement's snapshot.
    pin(&mut capture, 21).await;
    capture.commit().await.unwrap();
    let result = collector.await.unwrap();
    assert_eq!(epoch(&result).outcome, GraphReclamationOutcome::NoWork);
    assert_eq!(epoch(&result).candidates, 1);
    assert_eq!(remaining(db.pool()).await, vec![21]);
    unpin(db.pool()).await;
    assert_eq!(epoch(&run(db.pool(), &options()).await).changed, 1);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn replacement_identity_after_discovery_is_skipped_until_the_next_pass() {
    let db = database().await;
    seed(db.pool(), &[25]).await;
    make_due(db.pool(), true).await;
    let mut replacement = db.pool().begin().await.unwrap();
    SchemaTypeRepository::lock_reclamation(&mut replacement)
        .await
        .unwrap();
    let pool = db.pool().clone();
    let collector = tokio::spawn(async move { run(&pool, &options()).await });
    waiting(db.pool(), "ExclusiveLock", None).await;
    sqlx::query("DELETE FROM runtime_cache_iteration_epoch WHERE iteration=25")
        .execute(&mut *replacement)
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtime_cache_iteration_epoch(iteration,creation_transaction) VALUES(25,'replacement-fixture')")
        .execute(&mut *replacement).await.unwrap();
    replacement.commit().await.unwrap();
    let result = collector.await.unwrap();
    assert_eq!(
        epoch(&result).outcome,
        GraphReclamationOutcome::NoWork,
        "{result:?}"
    );
    assert_eq!(epoch(&result).candidates, 1);
    assert_eq!(
        epoch(&result).changed,
        0,
        "an equal PK is not an equal discovery identity"
    );
    assert_eq!(remaining(db.pool()).await, vec![25]);
    let next = run(db.pool(), &options()).await;
    assert_eq!(epoch(&next).changed, 1, "{next:?}");
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn gc_first_capture_waits_before_reading_the_reclaimed_identity() {
    let db = database().await;
    seed(db.pool(), &[31]).await;
    let key = 990031_i64;
    delete_barrier(db.pool(), key).await;
    let mut barrier = db.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *barrier)
        .await
        .unwrap();
    let pool = db.pool().clone();
    let collector = tokio::spawn(async move { run(&pool, &options()).await });
    waiting(db.pool(), "ExclusiveLock", Some(key)).await;
    let pool = db.pool().clone();
    let capture = tokio::spawn(async move {
        let mut tx = pool.begin().await.unwrap();
        SchemaTypeRepository::lock_selection(&mut tx).await.unwrap();
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_cache_iteration_epoch WHERE iteration=31)",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        exists
    });
    waiting(db.pool(), "ShareLock", None).await;
    barrier.rollback().await.unwrap();
    assert_eq!(epoch(&collector.await.unwrap()).changed, 1);
    assert!(
        !capture.await.unwrap(),
        "capture cannot authorize an already-collected identity"
    );
    graph_available(db.pool()).await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn deadline_rolls_back_delete_and_cursor_before_persisting_deferral() {
    let db = database().await;
    seed(db.pool(), &[41]).await;
    let key = 990041_i64;
    delete_barrier(db.pool(), key).await;
    let mut barrier = db.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *barrier)
        .await
        .unwrap();
    let mut budgets = options();
    budgets.statement_timeout = Duration::from_secs(2);
    let pool = db.pool().clone();
    let collector = tokio::spawn(async move { run(&pool, &budgets).await });
    waiting(db.pool(), "ExclusiveLock", Some(key)).await;
    let result = collector.await.unwrap();
    assert_eq!(
        epoch(&result).outcome,
        GraphReclamationOutcome::DeadlineDeferred
    );
    assert_eq!(epoch(&result).changed, 0);
    assert_eq!(remaining(db.pool()).await, vec![41]);
    let persisted:(i64,i64,bool)=sqlx::query_as("SELECT cursor_id,high_water,deferred_since IS NOT NULL FROM graph_reclamation_progress WHERE phase='cache_epochs'").fetch_one(db.pool()).await.unwrap();
    assert_eq!(persisted, (41, 41, true));
    let progress = epoch_progress(db.pool()).await;
    assert_eq!(
        progress.probe_limit, 256,
        "one actual raw row skips immediately even with a larger configured cap"
    );
    assert!(progress.pass_deferred && progress.oversized_since.is_some());
    // Still holding the test's downstream barrier. Only completed rollback can
    // make the graph available here while leaving the candidate intact.
    graph_available(db.pool()).await;
    barrier.rollback().await.unwrap();
    sqlx::query("DROP TRIGGER z_collector_test_barrier ON runtime_cache_iteration_epoch")
        .execute(db.pool())
        .await
        .unwrap();
    make_due(db.pool(), false).await;
    assert_eq!(epoch(&run(db.pool(), &options()).await).cursor, 0);
    assert_eq!(epoch(&run(db.pool(), &options()).await).changed, 1);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn cancellation_disposes_owned_backend_without_committing_cursor_or_delete() {
    let db = database().await;
    seed(db.pool(), &[51]).await;
    let key = 990051_i64;
    delete_barrier(db.pool(), key).await;
    let mut barrier = db.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *barrier)
        .await
        .unwrap();
    let pool = db.pool().clone();
    let collector = tokio::spawn(async move { run(&pool, &options()).await });
    let pid = waiting(db.pool(), "ExclusiveLock", Some(key)).await;
    collector.abort();
    assert!(collector.await.unwrap_err().is_cancelled());
    graph_available(db.pool()).await;
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let present: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1)")
                .bind(pid)
                .fetch_one(db.pool())
                .await
                .unwrap();
        if !present {
            break;
        }
        assert!(
            Instant::now() < end,
            "cancelled owned backend survived socket disposal"
        );
        tokio::task::yield_now().await;
    }
    assert_eq!(remaining(db.pool()).await, vec![51]);
    let cursor: i64 = sqlx::query_scalar(
        "SELECT cursor_id FROM graph_reclamation_progress WHERE phase='cache_epochs'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(cursor, 0);
    let progress = epoch_progress(db.pool()).await;
    assert_eq!((progress.high_water, progress.probe_limit), (0, 256));
    assert!(!progress.pass_deferred && progress.oversized_since.is_none());
    barrier.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn persistent_deferral_backoff_eventually_uses_bounded_queued_admission() {
    let db = database().await;
    seed(db.pool(), &[61]).await;
    let mut capture = db.pool().begin().await.unwrap();
    SchemaTypeRepository::lock_selection(&mut capture)
        .await
        .unwrap();
    let first = run(db.pool(), &options()).await;
    assert_eq!(epoch(&first).outcome, GraphReclamationOutcome::LockDeferred);
    let progress = epoch_progress(db.pool()).await;
    assert_eq!(
        (progress.cursor, progress.high_water, progress.probe_limit),
        (0, 0, 256)
    );
    assert!(!progress.pass_deferred && progress.oversized_since.is_none());
    let before: DateTime<Utc> = sqlx::query_scalar(
        "SELECT deferred_since FROM graph_reclamation_progress WHERE phase='cache_epochs'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    let second = collect(
        db.pool(),
        Utc::now(),
        0,
        &options(),
        CollectionMode::Scheduled,
    )
    .await
    .unwrap();
    assert_eq!(epoch(&second).outcome, GraphReclamationOutcome::NotDue);
    let after: DateTime<Utc> = sqlx::query_scalar(
        "SELECT deferred_since FROM graph_reclamation_progress WHERE phase='cache_epochs'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(before, after);
    // Model restart/elapsed DB time without global clocks or sleeping. A new
    // collect call has no in-memory cursor, deferral age, or retry policy state.
    make_due(db.pool(), true).await;
    let pool = db.pool().clone();
    let collector = tokio::spawn(async move { run(&pool, &options()).await });
    waiting(db.pool(), "ExclusiveLock", None).await;
    capture.rollback().await.unwrap();
    assert_eq!(epoch(&collector.await.unwrap()).changed, 1);
    let persisted:(bool,bool)=sqlx::query_as("SELECT deferred_since IS NULL,last_progress IS NOT NULL FROM graph_reclamation_progress WHERE phase='cache_epochs'").fetch_one(db.pool()).await.unwrap();
    assert_eq!(persisted, (true, true));
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn queued_admission_timeout_does_not_escape_into_dependent_delete_work() {
    let db = database().await;
    seed(db.pool(), &[62]).await;
    let original: String = sqlx::query_scalar("SELECT current_setting('lock_timeout')")
        .fetch_one(db.pool())
        .await
        .unwrap();
    sqlx::query("CREATE FUNCTION collector_test_lock_scope() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF current_setting('lock_timeout')<>TG_ARGV[0] THEN RAISE EXCEPTION 'admission lock timeout escaped into dependent DELETE'; END IF; RETURN OLD; END; $$")
        .execute(db.pool()).await.unwrap();
    sqlx::query(&format!("CREATE TRIGGER z_collector_test_lock_scope BEFORE DELETE ON runtime_cache_iteration_epoch FOR EACH ROW EXECUTE FUNCTION collector_test_lock_scope('{}')",original.replace('\'',"''")))
        .execute(db.pool()).await.unwrap();
    make_due(db.pool(), true).await;
    let mut capture = db.pool().begin().await.unwrap();
    SchemaTypeRepository::lock_selection(&mut capture)
        .await
        .unwrap();
    let pool = db.pool().clone();
    let mut collector = tokio::spawn(async move { run(&pool, &options()).await });
    waiting_for_collector(db.pool(), "ExclusiveLock", None, &mut collector).await;
    capture.rollback().await.unwrap();
    let result = collector.await.unwrap();
    assert_eq!(
        epoch(&result).outcome,
        GraphReclamationOutcome::Progress,
        "{result:?}"
    );
    assert!(remaining(db.pool()).await.is_empty());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn empty_sweeps_need_no_exclusive_admission_and_complete_their_transactions() {
    let db = database().await;
    let progress = super::super::PackRetentionRepository::reclamation_progress(db.pool())
        .await
        .unwrap();
    assert_eq!(progress.len(), 7);
    assert!(progress
        .iter()
        .all(|phase| phase.next_attempt.is_none() && phase.last_progress.is_none()));
    let mut capture = db.pool().begin().await.unwrap();
    SchemaTypeRepository::lock_selection(&mut capture)
        .await
        .unwrap();
    let result = run(db.pool(), &options()).await;
    assert!(result.complete());
    assert!(result
        .phases
        .iter()
        .all(|phase| phase.candidates == 0 && phase.changed == 0 && phase.cursor == 0));
    capture.rollback().await.unwrap();
    let mut writer = db.pool().begin().await.unwrap();
    let acquired:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended(current_schema() || ':named-type-graph',0))").fetch_one(&mut *writer).await.unwrap();
    assert!(acquired);
    writer.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn persistent_slow_leaf_cannot_hide_siblings_or_the_next_original_page() {
    let db = database().await;
    let ids: Vec<i64> = (1..=257).collect();
    seed(db.pool(), &ids).await;
    let key = 990101_i64;
    delete_barrier_for(db.pool(), key, Some(1)).await;
    let mut barrier = db.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *barrier)
        .await
        .unwrap();
    let budgets = GraphReclamationOptions {
        candidate_limit: 256,
        delete_limit: 256,
        statement_timeout: Duration::from_secs(2),
        ..options()
    };
    let mut oldest = None;
    // 256 -> 128 -> ... -> 1, then a single-leaf deferred visit. Every
    // attempted DELETE reaches the same held barrier, never a fixed sleep.
    for expected in [128, 64, 32, 16, 8, 4, 2, 1, 256] {
        let pool = db.pool().clone();
        let attempt_budgets = budgets.clone();
        let mut collector = tokio::spawn(async move { run(&pool, &attempt_budgets).await });
        waiting_for_collector(db.pool(), "ExclusiveLock", Some(key), &mut collector).await;
        let result = collector.await.unwrap();
        assert_eq!(
            epoch(&result).outcome,
            GraphReclamationOutcome::DeadlineDeferred,
            "{result:?}"
        );
        assert_eq!(epoch(&result).changed, 0);
        let progress = epoch_progress(db.pool()).await;
        assert_eq!(progress.probe_limit, expected);
        assert_eq!(
            progress.high_water, 257,
            "adaptive retries retain the original pass boundary"
        );
        assert!(progress.pass_deferred);
        let since = progress
            .oversized_since
            .expect("over-budget age must be durable");
        if let Some(first) = oldest {
            assert_eq!(since, first);
        } else {
            oldest = Some(since);
        }
        assert_eq!(progress.cursor, if expected == 256 { 1 } else { 0 });
        assert_eq!(
            remaining(db.pool()).await,
            ids,
            "every timed-out DELETE fully rolled back"
        );
        make_due(db.pool(), false).await;
    }
    let cheap = run(db.pool(), &budgets).await;
    assert_eq!(epoch(&cheap).changed, 256);
    assert_eq!(
        remaining(db.pool()).await,
        vec![1],
        "siblings 2..256 and beyond-original-page row 257 must all drain"
    );
    let progress = epoch_progress(db.pool()).await;
    assert_eq!((progress.cursor, progress.high_water), (257, 257));
    assert_eq!(
        progress.oversized_since, oldest,
        "cheap progress must not erase the diagnostic"
    );
    assert_eq!(
        progress.deferred_since, oldest,
        "operator status retains the unresolved over-budget age"
    );
    assert_eq!(epoch(&run(db.pool(), &budgets).await).cursor, 0);
    let progress = epoch_progress(db.pool()).await;
    assert!(!progress.pass_deferred);
    assert_eq!(
        progress.oversized_since, oldest,
        "a completed dirty pass cannot clear its diagnostic"
    );

    // Restarted collection revisits the old ID. Only one raw row remains, so
    // it is deferred immediately even though the restored configured cap is 256.
    let pool = db.pool().clone();
    let attempt_budgets = budgets.clone();
    let mut collector = tokio::spawn(async move { run(&pool, &attempt_budgets).await });
    waiting_for_collector(db.pool(), "ExclusiveLock", Some(key), &mut collector).await;
    let revisit = collector.await.unwrap();
    assert_eq!((epoch(&revisit).candidates, epoch(&revisit).cursor), (1, 1));
    assert_eq!(epoch_progress(db.pool()).await.oversized_since, oldest);
    barrier.rollback().await.unwrap();
    make_due(db.pool(), false).await;
    assert_eq!(epoch(&run(db.pool(), &budgets).await).cursor, 0);
    assert_eq!(epoch_progress(db.pool()).await.oversized_since, oldest);
    assert_eq!(epoch(&run(db.pool(), &budgets).await).changed, 1);
    let progress = epoch_progress(db.pool()).await;
    assert!(progress.oversized_since.is_none() && progress.deferred_since.is_none());
    assert!(remaining(db.pool()).await.is_empty());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn raw_candidate_limit_above_delete_limit_revisits_undeleted_siblings_on_wrap() {
    let db = database().await;
    seed(db.pool(), &[1, 2, 3, 4, 5, 6, 7]).await;
    let budgets = GraphReclamationOptions {
        candidate_limit: 4,
        delete_limit: 1,
        ..options()
    };
    let mut deleted = 0;
    for _ in 0..14 {
        let result = run(db.pool(), &budgets).await;
        assert!(result.pages_succeeded());
        assert!(epoch(&result).changed <= 1 && epoch(&result).candidates <= 4);
        deleted += epoch(&result).changed;
        if remaining(db.pool()).await.is_empty() {
            break;
        }
    }
    assert_eq!(deleted, 7);
    assert!(remaining(db.pool()).await.is_empty());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn page_work_deadline_precedes_statement_cap_and_resumes_after_confirmed_rollback() {
    let db = database().await;
    seed(db.pool(), &[71]).await;
    let key = 990171_i64;
    delete_barrier(db.pool(), key).await;
    let mut barrier = db.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *barrier)
        .await
        .unwrap();
    let budgets = GraphReclamationOptions {
        statement_timeout: Duration::from_secs(10),
        page_timeout: Duration::from_secs(2),
        cycle_timeout: Duration::from_secs(90),
        ..options()
    };
    let pool = db.pool().clone();
    let attempt_budgets = budgets.clone();
    let collector = tokio::spawn(async move { run(&pool, &attempt_budgets).await });
    waiting(db.pool(), "ExclusiveLock", Some(key)).await;
    let result = collector.await.unwrap();
    let phase = epoch(&result);
    assert_eq!(
        phase.outcome,
        GraphReclamationOutcome::DeadlineDeferred,
        "{result:?}"
    );
    assert!(phase.elapsed >= budgets.page_timeout - Duration::from_millis(250));
    assert!(
        phase.elapsed < budgets.statement_timeout,
        "the smaller page budget, not the statement cap, must end blocked work"
    );
    assert_eq!((phase.changed, phase.cursor), (0, 71));
    assert_eq!(remaining(db.pool()).await, vec![71]);
    graph_available(db.pool()).await;
    barrier.rollback().await.unwrap();
    make_due(db.pool(), false).await;
    assert_eq!(epoch(&run(db.pool(), &budgets).await).cursor, 0);
    assert_eq!(epoch(&run(db.pool(), &budgets).await).changed, 1);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn cycle_work_budget_stops_later_phases_and_restart_resumes_least_recent_phase() {
    let db = database().await;
    seed(db.pool(), &[81]).await;
    sqlx::query("INSERT INTO runtime_authority_admission(owner_kind,owner_id,policy_revision,authority,authority_digest) VALUES('execution',990081,990082,'{}',repeat('a',64))")
        .execute(db.pool()).await.unwrap();
    let key = 990181_i64;
    delete_barrier(db.pool(), key).await;
    // If a millisecond-rounded server timeout leaves a tiny part of the cycle
    // available, a later admission DELETE cannot race through that remainder.
    sqlx::query("CREATE TRIGGER z_collector_test_admission_barrier BEFORE DELETE ON runtime_authority_admission FOR EACH ROW EXECUTE FUNCTION collector_test_barrier()")
        .execute(db.pool()).await.unwrap();
    let mut barrier = db.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *barrier)
        .await
        .unwrap();
    // The cycle reserves three 5s cleanup budgets. Its remaining 4s of work
    // is shorter than both the 15s page and 5s statement limits.
    let budgets = GraphReclamationOptions {
        statement_timeout: Duration::from_secs(5),
        page_timeout: Duration::from_secs(15),
        cycle_timeout: Duration::from_secs(19),
        ..options()
    };
    let pool = db.pool().clone();
    let attempt_budgets = budgets.clone();
    let collector = tokio::spawn(async move { run(&pool, &attempt_budgets).await });
    waiting(db.pool(), "ExclusiveLock", Some(key)).await;
    let result = collector.await.unwrap();
    assert_eq!(
        epoch(&result).outcome,
        GraphReclamationOutcome::DeadlineDeferred,
        "{result:?}"
    );
    let admission = result
        .phases
        .iter()
        .find(|phase| phase.phase == GraphReclamationPhase::Admissions)
        .unwrap();
    assert_eq!(
        (admission.outcome, admission.changed),
        (GraphReclamationOutcome::DeadlineDeferred, 0)
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_authority_admission WHERE owner_id=990081",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "no new phase work can start after the cycle work deadline"
    );
    graph_available(db.pool()).await;
    barrier.rollback().await.unwrap();
    let statuses = super::super::PackRetentionRepository::reclamation_progress(db.pool())
        .await
        .unwrap();
    let first = GraphReclamationPhase::ALL
        .into_iter()
        .find(|phase| {
            statuses
                .iter()
                .any(|status| status.phase == phase.name() && status.last_attempt.is_none())
        })
        .unwrap();
    sqlx::query("UPDATE graph_reclamation_progress SET next_attempt=clock_timestamp()")
        .execute(db.pool())
        .await
        .unwrap();
    let resumed = run(db.pool(), &budgets).await;
    assert_eq!(resumed.phases[0].phase, first);
    assert_eq!(epoch(&resumed).cursor, 0);
    assert_eq!(epoch(&run(db.pool(), &budgets).await).changed, 1);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_authority_admission WHERE owner_id=990081",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        count, 0,
        "previously budget-limited phases must resume on a fresh collection"
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn discovery_timeout_does_not_shrink_or_advance_visit_progress() {
    let db = database().await;
    seed(db.pool(), &[91, 92]).await;
    let mut blocker = db.pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE runtime_cache_iteration_epoch IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let budgets = GraphReclamationOptions {
        statement_timeout: Duration::from_secs(2),
        ..options()
    };
    let pool = db.pool().clone();
    let attempt_budgets = budgets.clone();
    let mut collector = tokio::spawn(async move { run(&pool, &attempt_budgets).await });
    let end = Instant::now() + Duration::from_secs(10);
    let barrier_wait = async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND relation='runtime_cache_iteration_epoch'::regclass AND mode='AccessShareLock' AND NOT granted)")
            .fetch_one(db.pool()).await.unwrap();
            if waiting {
                break;
            }
            assert!(
                Instant::now() < end,
                "raw discovery never reached its owned relation barrier"
            );
            tokio::task::yield_now().await;
        }
    };
    tokio::select! {
        ()=barrier_wait=>{},
        result=&mut collector=>panic!("collector completed before discovery barrier: {result:?}"),
    }
    let result = collector.await.unwrap();
    assert_eq!(
        (epoch(&result).outcome, epoch(&result).candidates),
        (GraphReclamationOutcome::DeadlineDeferred, 0)
    );
    let progress = epoch_progress(db.pool()).await;
    assert_eq!(
        (progress.cursor, progress.high_water, progress.probe_limit),
        (0, 0, 256)
    );
    assert!(!progress.pass_deferred && progress.oversized_since.is_none());
    graph_available(db.pool()).await;
    blocker.rollback().await.unwrap();
    make_due(db.pool(), false).await;
    assert_eq!(epoch(&run(db.pool(), &budgets).await).changed, 2);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn truncated_clean_looking_page_cannot_clear_an_untried_oversized_leaf_age() {
    let db = database().await;
    seed(db.pool(), &[1, 2, 3]).await;
    sqlx::query("UPDATE graph_reclamation_progress SET oversized_since=clock_timestamp()-interval '1 hour' WHERE phase='cache_epochs'")
        .execute(db.pool()).await.unwrap();
    let oldest = epoch_progress(db.pool()).await.oversized_since;
    let budgets = GraphReclamationOptions {
        candidate_limit: 256,
        delete_limit: 1,
        ..options()
    };
    let result = run(db.pool(), &budgets).await;
    assert_eq!(
        (
            epoch(&result).candidates,
            epoch(&result).changed,
            epoch(&result).cursor
        ),
        (3, 1, 0)
    );
    assert_eq!(
        epoch_progress(db.pool()).await.oversized_since,
        oldest,
        "DELETE LIMIT 1 cannot prove anything about its untried siblings"
    );
    assert_eq!(epoch(&run(db.pool(), &budgets).await).changed, 1);
    assert_eq!(epoch_progress(db.pool()).await.oversized_since, oldest);
    assert_eq!(epoch(&run(db.pool(), &budgets).await).changed, 1);
    assert!(epoch_progress(db.pool()).await.oversized_since.is_none());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn stale_post_rollback_retry_cannot_change_a_newer_phase_cursor() {
    let db = database().await;
    sqlx::query("UPDATE graph_reclamation_progress SET revision=7 WHERE phase='cache_epochs'")
        .execute(db.pool())
        .await
        .unwrap();
    let result = defer(
        db.pool(),
        GraphReclamationPhase::CacheEpochs,
        6,
        &options(),
        Some(DeleteRetry {
            probe_limit: 1,
            cursor: Some(1),
            high_water: 257,
        }),
    )
    .await
    .unwrap();
    assert!(matches!(result, DeferredProgress::Committed(None)));
    let progress = epoch_progress(db.pool()).await;
    assert_eq!(
        (progress.cursor, progress.high_water, progress.probe_limit),
        (0, 0, 256)
    );
    assert!(!progress.pass_deferred && progress.oversized_since.is_none());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn production_defaults_cross_ten_thousand_pins_and_persist_fast_continuation() {
    use super::super::PackRetentionRepository;

    let db = database().await;
    let ids: Vec<i64> = (1..=10_540).collect();
    seed(db.pool(), &ids).await;
    let mut tx = db.pool().begin().await.unwrap();
    SchemaTypeRepository::lock_selection(&mut tx).await.unwrap();
    pin(&mut tx, 1).await;
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) SELECT 990001,'runtime_cache_iteration_epoch',id FROM generate_series(2,10240) id")
        .execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    let budgets = GraphReclamationOptions::default();
    let mut saw_progress = false;
    let mut saw_multiple_pages = false;
    // Each invocation has the unchanged production cycle budget. Advance only
    // the owned durable deadline between calls, modelling scheduler/restart time.
    for _ in 0..6 {
        let result = PackRetentionRepository::collect_bounded(db.pool(), Utc::now(), 0, &budgets)
            .await
            .unwrap();
        saw_multiple_pages |= epoch(&result).pages > 1;
        let progress = epoch_progress(db.pool()).await;
        if progress.pending {
            let delay = progress.next_attempt.unwrap() - progress.last_attempt.unwrap();
            assert!(delay >= chrono::Duration::seconds(1) && delay < chrono::Duration::seconds(2));
            assert!(result.backlog_pending() && !result.complete(), "{result:?}");
        }
        if epoch(&result).changed > 0 {
            saw_progress = true;
        }
        if remaining(db.pool()).await.len() == 10_240 && !progress.pending {
            let delay = progress.next_attempt.unwrap() - progress.last_attempt.unwrap();
            assert!(
                delay >= chrono::Duration::hours(1)
                    && delay < chrono::Duration::hours(1) + chrono::Duration::seconds(1)
            );
            break;
        }
        make_due(db.pool(), false).await;
    }
    assert!(
        saw_multiple_pages && saw_progress,
        "bounded cycles must cross the pinned prefix, not wait forty hourly ticks"
    );
    assert_eq!(remaining(db.pool()).await, (1..=10_240).collect::<Vec<_>>());
    assert!(!epoch_progress(db.pool()).await.pending);
    assert!(!PackRetentionRepository::reclamation_due(db.pool())
        .await
        .unwrap());
    assert!(PackRetentionRepository::next_reclamation_attempt(db.pool())
        .await
        .unwrap()
        .is_some());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn restart_keeps_scan_pending_and_finishes_only_the_original_high_water_pass() {
    use super::super::PackRetentionRepository;

    let db = database().await;
    assert!(PackRetentionRepository::reclamation_due(db.pool())
        .await
        .unwrap());
    let next = PackRetentionRepository::next_reclamation_attempt(db.pool())
        .await
        .unwrap()
        .unwrap();
    assert!(next <= Utc::now());
    seed(db.pool(), &[11, 12]).await;
    let mut tx = db.pool().begin().await.unwrap();
    SchemaTypeRepository::lock_selection(&mut tx).await.unwrap();
    pin(&mut tx, 11).await;
    tx.commit().await.unwrap();
    let budgets = GraphReclamationOptions {
        candidate_limit: 1,
        delete_limit: 1,
        continuation_delay: Duration::from_secs(30),
        ..options()
    };
    let first = run(db.pool(), &budgets).await;
    assert_eq!(epoch(&first).cursor, 11);
    assert!(first.backlog_pending() && !first.complete());
    let progress = epoch_progress(db.pool()).await;
    assert_eq!(progress.high_water, 12);
    assert!(progress.pending);
    assert_eq!(
        PackRetentionRepository::next_reclamation_attempt(db.pool())
            .await
            .unwrap(),
        progress.next_attempt
    );
    let not_due = PackRetentionRepository::collect_bounded(db.pool(), Utc::now(), 0, &budgets)
        .await
        .unwrap();
    assert_eq!(epoch(&not_due).outcome, GraphReclamationOutcome::NotDue);
    assert!(not_due.backlog_pending() && !not_due.complete());

    seed(db.pool(), &[13]).await;
    make_due(db.pool(), false).await;
    let resumed = PackRetentionRepository::collect_bounded(db.pool(), Utc::now(), 0, &budgets)
        .await
        .unwrap();
    assert_eq!(
        (
            epoch(&resumed).changed,
            epoch(&resumed).cursor,
            epoch(&resumed).pages
        ),
        (1, 0, 2)
    );
    assert!(resumed.complete(), "{resumed:?}");
    assert_eq!(remaining(db.pool()).await, vec![11, 13]);
    unpin(db.pool()).await;
    make_due(db.pool(), false).await;
    let wrapped = PackRetentionRepository::collect_bounded(db.pool(), Utc::now(), 0, &budgets)
        .await
        .unwrap();
    assert_eq!(epoch(&wrapped).changed, 2);
    assert!(remaining(db.pool()).await.is_empty());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn failed_phase_is_not_hot_retried_while_other_phases_take_additional_pages() {
    use super::super::PackRetentionRepository;

    let db = database().await;
    seed(db.pool(), &[1, 2, 3, 4, 5]).await;
    sqlx::query("INSERT INTO runtime_authority_admission(owner_kind,owner_id,policy_revision,authority,authority_digest) SELECT 'execution',id,990012,'{}',repeat('a',64) FROM generate_series(990100,990104) id")
        .execute(db.pool()).await.unwrap();
    sqlx::query("CREATE FUNCTION collector_test_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned phase failure'; END; $$")
        .execute(db.pool()).await.unwrap();
    sqlx::query("CREATE TRIGGER z_collector_test_failure BEFORE DELETE ON runtime_cache_iteration_epoch FOR EACH ROW EXECUTE FUNCTION collector_test_failure()")
        .execute(db.pool()).await.unwrap();
    let budgets = GraphReclamationOptions {
        candidate_limit: 2,
        delete_limit: 2,
        ..options()
    };
    let result = PackRetentionRepository::collect_bounded(db.pool(), Utc::now(), 0, &budgets)
        .await
        .unwrap();
    assert_eq!(
        (epoch(&result).outcome, epoch(&result).pages),
        (GraphReclamationOutcome::Failed, 1)
    );
    let admissions = result
        .phases
        .iter()
        .find(|phase| phase.phase == GraphReclamationPhase::Admissions)
        .unwrap();
    assert_eq!((admissions.changed, admissions.pages), (5, 3));
    let before = epoch_progress(db.pool()).await;
    let restarted = PackRetentionRepository::collect_bounded(db.pool(), Utc::now(), 0, &budgets)
        .await
        .unwrap();
    assert_eq!(
        (epoch(&restarted).outcome, epoch(&restarted).pages),
        (GraphReclamationOutcome::NotDue, 0)
    );
    let after = epoch_progress(db.pool()).await;
    assert_eq!(
        (before.deferred_since, before.next_attempt),
        (after.deferred_since, after.next_attempt)
    );
    assert!(restarted.backlog_pending() && !restarted.complete());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn scheduled_aged_queued_deferral_preserves_age_and_is_not_retried_in_cycle() {
    let db = database().await;
    seed(db.pool(), &[1, 2, 3]).await;
    make_due(db.pool(), true).await;
    let oldest = epoch_progress(db.pool()).await.deferred_since;
    let mut capture = db.pool().begin().await.unwrap();
    SchemaTypeRepository::lock_selection(&mut capture)
        .await
        .unwrap();
    let pool = db.pool().clone();
    let mut collector = tokio::spawn(async move {
        super::super::PackRetentionRepository::collect_bounded(&pool, Utc::now(), 0, &options())
            .await
            .unwrap()
    });
    waiting_for_collector(db.pool(), "ExclusiveLock", None, &mut collector).await;
    let result = collector.await.unwrap();
    assert_eq!(
        (epoch(&result).outcome, epoch(&result).pages),
        (GraphReclamationOutcome::LockDeferred, 1)
    );
    let progress = epoch_progress(db.pool()).await;
    assert_eq!(progress.deferred_since, oldest);
    assert!(progress.pending && progress.next_attempt.unwrap() > Utc::now());
    let restarted = super::super::PackRetentionRepository::collect_bounded(
        db.pool(),
        Utc::now(),
        0,
        &options(),
    )
    .await
    .unwrap();
    assert_eq!(
        (epoch(&restarted).outcome, epoch(&restarted).pages),
        (GraphReclamationOutcome::NotDue, 0)
    );
    assert_eq!(epoch_progress(db.pool()).await.deferred_since, oldest);
    capture.rollback().await.unwrap();
    graph_available(db.pool()).await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn lost_commit_ack_does_not_report_deletes_and_restart_reads_atomic_progress() {
    let db = database().await;
    seed(db.pool(), &[1, 2]).await;
    let budgets = options();
    let policy = PagePolicy {
        cutoff: Utc::now(),
        newest: 0,
        options: &budgets,
        compatibility_wait: false,
        lose_commit_ack: true,
    };
    let mut state = Progress {
        phase: "cache_epochs".into(),
        cursor_id: 0,
        high_water: 0,
        probe_limit: 256,
        revision: 0,
        last_attempt: None,
        due: true,
        aged: false,
        pending: false,
    };
    let mut report = GraphReclamationPhaseResult {
        phase: GraphReclamationPhase::CacheEpochs,
        outcome: GraphReclamationOutcome::NotDue,
        candidates: 0,
        changed: 0,
        cursor: 0,
        pending: true,
        pages: 1,
        elapsed: Duration::ZERO,
        error: None,
    };
    let content = page(
        db.pool(),
        GraphReclamationPhase::CacheEpochs,
        &mut state,
        &policy,
        Instant::now() + budgets.page_timeout,
        &mut report,
        &mut None,
    )
    .await
    .unwrap();
    assert_eq!(
        (report.outcome, report.changed),
        (GraphReclamationOutcome::CommitUnknown, 0)
    );
    assert!(content.is_empty());
    assert!(remaining(db.pool()).await.is_empty());
    let progress = epoch_progress(db.pool()).await;
    assert_eq!(
        (progress.cursor, progress.high_water, progress.pending),
        (0, 0, false)
    );
    assert!(progress.next_attempt.is_some() && progress.last_progress.is_some());
    let restarted =
        super::super::PackRetentionRepository::collect_bounded(db.pool(), Utc::now(), 0, &budgets)
            .await
            .unwrap();
    assert_eq!(
        (epoch(&restarted).outcome, epoch(&restarted).changed),
        (GraphReclamationOutcome::NotDue, 0)
    );
    graph_available(db.pool()).await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn release_commit_ack_loss_withholds_cleanup_payload_but_keeps_confirmed_pages() {
    use super::super::PackRetentionRepository;
    use crate::repositories::pack_release::{CreatePackReleaseInput, PackReleaseRepository};
    use serde_json::json;

    let db = database().await;
    let pack = release_test_pack(db.pool()).await;
    let packs = tempfile::tempdir().unwrap();
    let cutoff = Utc::now();
    let mut releases = Vec::new();
    let mut tx = db.pool().begin().await.unwrap();
    for index in 0..3 {
        // The third release retains the uncertain page's content by digest/path.
        let digest = format!("{:064x}", if index == 0 { 1 } else { 2 });
        let path = packs
            .path()
            .join(".releases/sha256")
            .join(&digest)
            .join("pack");
        std::fs::create_dir_all(&path).unwrap();
        let release = PackReleaseRepository::create_or_get(
            &mut tx,
            CreatePackReleaseInput {
                pack: pack.id,
                pack_ref: pack.r#ref.clone(),
                version: format!("1.0.{index}"),
                digest,
                object_key: format!("collector/{index}.tar.gz"),
                provider_version: "e:owned-fixture".into(),
                content_path: path.to_str().unwrap().into(),
                archive_size: 1,
                manifest: json!({}),
            },
        )
        .await
        .unwrap();
        let inactive = if index == 2 {
            cutoff
        } else {
            cutoff - chrono::Duration::days(1)
        };
        sqlx::query("UPDATE pack_release SET inactive_since=$2 WHERE id=$1")
            .bind(release.id)
            .bind(inactive)
            .execute(&mut *tx)
            .await
            .unwrap();
        releases.push(release);
    }
    tx.commit().await.unwrap();

    let budgets = GraphReclamationOptions {
        candidate_limit: 1,
        delete_limit: 1,
        ..options()
    };
    let GraphReclamationResult {
        phases,
        deleted_content: mut confirmed,
    } = collect(
        db.pool(),
        cutoff,
        0,
        &budgets,
        CollectionMode::SinglePage { force: true },
    )
    .await
    .unwrap();
    let mut report = phases
        .into_iter()
        .find(|phase| phase.phase == GraphReclamationPhase::Releases)
        .unwrap();
    assert_eq!(
        (report.outcome, report.changed, confirmed.len()),
        (GraphReclamationOutcome::Progress, 1, 1)
    );
    assert_eq!(confirmed[0].digest, releases[0].digest);
    let mut state: Progress = sqlx::query_as(
        "SELECT phase,cursor_id,high_water,probe_limit,revision,last_attempt,
            true AS due,false AS aged,true AS pending
         FROM graph_reclamation_progress WHERE phase='releases'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    let policy = PagePolicy {
        cutoff,
        newest: 0,
        options: &budgets,
        compatibility_wait: false,
        lose_commit_ack: true,
    };
    let uncertain = page(
        db.pool(),
        GraphReclamationPhase::Releases,
        &mut state,
        &policy,
        Instant::now() + budgets.page_timeout,
        &mut report,
        &mut None,
    )
    .await
    .unwrap();
    assert_eq!(
        (report.outcome, report.changed),
        (GraphReclamationOutcome::CommitUnknown, 0)
    );
    assert!(
        uncertain.is_empty(),
        "a committed release DELETE must not leak its unacknowledged RETURNING payload"
    );
    assert_eq!(
        PackRetentionRepository::cleanup_unreferenced(db.pool(), packs.path(), &uncertain)
            .await
            .unwrap(),
        0
    );
    confirmed.extend(uncertain);
    assert_eq!(
        confirmed.len(),
        1,
        "earlier acknowledged page descriptors survive"
    );
    assert_eq!(
        state.cursor_id, releases[0].id,
        "in-memory cursor remains last acknowledged"
    );
    let progress = PackRetentionRepository::reclamation_progress(db.pool())
        .await
        .unwrap()
        .into_iter()
        .find(|phase| phase.phase == "releases")
        .unwrap();
    assert_eq!(
        (progress.cursor, progress.high_water),
        (releases[1].id, releases[2].id)
    );
    assert!(
        progress.pending && progress.next_attempt.is_some() && progress.last_progress.is_some()
    );
    for release in &releases[..2] {
        assert!(PackReleaseRepository::find_by_id(db.pool(), release.id)
            .await
            .unwrap()
            .is_none());
    }
    assert_eq!(
        PackRetentionRepository::retained_count(db.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        PackRetentionRepository::cleanup_unreferenced(db.pool(), packs.path(), &confirmed)
            .await
            .unwrap(),
        1
    );
    assert!(!std::path::Path::new(&releases[0].content_path).exists());
    assert!(std::path::Path::new(&releases[1].content_path).is_dir());

    // Diagnostic reconstruction is not a deletion receipt. Even a reconstructed
    // descriptor cannot bypass the cleanup fence while another release retains
    // those bytes. This is an acknowledged-COMMIT response-loss seam, not a new
    // crash/orphan reconciliation protocol; unreferenced local-tree recovery
    // after an unknown commit remains an inherited limitation outside this test.
    let mut inspection = db.pool().begin().await.unwrap();
    let retained = PackRetentionRepository::content_for_pack(&mut inspection, pack.id)
        .await
        .unwrap();
    inspection.rollback().await.unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].digest, releases[1].digest);
    assert_eq!(
        PackRetentionRepository::cleanup_unreferenced(db.pool(), packs.path(), &retained)
            .await
            .unwrap(),
        0
    );
    assert!(std::path::Path::new(&releases[1].content_path).is_dir());
    graph_available(db.pool()).await;
    packs.close().unwrap();
    db.cleanup().await.unwrap();
}

async fn release_test_pack(pool: &PgPool) -> crate::models::Pack {
    use crate::repositories::pack::{CreatePackInput, PackRepository};
    use serde_json::json;

    PackRepository::create(
        pool,
        CreatePackInput {
            r#ref: "collector_cutoff".into(),
            label: "Collector cutoff".into(),
            description: None,
            version: "1.0.0".into(),
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            tags: vec![],
            runtime_deps: vec![],
            dependencies: vec![],
            is_standard: false,
            installers: json!({}),
        },
    )
    .await
    .unwrap()
}

async fn seed_inactive_releases(pool: &PgPool, count: i64) -> Vec<i64> {
    let pack = release_test_pack(pool).await;
    let mut tx = pool.begin().await.unwrap();
    SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let mut ids: Vec<i64> = sqlx::query_scalar(
        "INSERT INTO pack_release(pack,pack_ref,version,digest,object_key,provider_version,content_path,archive_size,manifest,inactive_since)
         SELECT $1,$2,'1.0.'||n,lpad(to_hex(n),64,'0'),'collector/'||n,'e:owned-fixture',
             '/packs/.releases/collector-'||n,1,'{}',clock_timestamp()-interval '1 day'
         FROM generate_series(1,$3::bigint) n ORDER BY n RETURNING id",
    )
    .bind(pack.id)
    .bind(&pack.r#ref)
    .bind(count)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    ids.sort_unstable();
    ids
}

#[tokio::test]
async fn graph_release_cycle_cap() {
    use super::super::PackRetentionRepository;

    let db = database().await;
    let releases = seed_inactive_releases(db.pool(), 257).await;
    seed(db.pool(), &(1..=257).collect::<Vec<_>>()).await;
    let budgets = GraphReclamationOptions {
        delete_limit: 1,
        ..options()
    };
    let first = PackRetentionRepository::collect_bounded(db.pool(), Utc::now(), 0, &budgets)
        .await
        .unwrap();
    let release = first
        .phases
        .iter()
        .find(|phase| phase.phase == GraphReclamationPhase::Releases)
        .unwrap();
    assert_eq!(
        (release.changed, first.deleted_content.len()),
        (1, 1),
        "257 eligible releases must not receive a fresh DELETE allowance on the second page"
    );
    assert_eq!((release.pages, release.cursor), (1, releases[255]));
    assert!(release.pending && first.backlog_pending() && !first.complete());
    assert_eq!(
        PackRetentionRepository::retained_count(db.pool())
            .await
            .unwrap(),
        256
    );
    assert_eq!(
        epoch(&first).changed,
        2,
        "the release cap is not a global graph deletion cap"
    );
    let progress = PackRetentionRepository::reclamation_progress(db.pool())
        .await
        .unwrap()
        .into_iter()
        .find(|phase| phase.phase == "releases")
        .unwrap();
    assert_eq!(
        (progress.cursor, progress.high_water),
        (releases[255], releases[256])
    );
    assert!(progress.pending && progress.pass_deferred);
    let delay = progress.next_attempt.unwrap() - progress.last_attempt.unwrap();
    assert_eq!(delay, chrono::Duration::seconds(1));

    sqlx::query("UPDATE graph_reclamation_progress SET next_attempt=clock_timestamp() WHERE phase='releases'")
        .execute(db.pool()).await.unwrap();
    let resumed = PackRetentionRepository::collect_bounded(db.pool(), Utc::now(), 0, &budgets)
        .await
        .unwrap();
    let release = resumed
        .phases
        .iter()
        .find(|phase| phase.phase == GraphReclamationPhase::Releases)
        .unwrap();
    assert_eq!((release.changed, release.pages, release.cursor), (1, 1, 0));
    assert_eq!(
        resumed.deleted_content.len(),
        1,
        "a fresh invocation resets the allowance"
    );
    assert_eq!(
        PackRetentionRepository::retained_count(db.pool())
            .await
            .unwrap(),
        255
    );
    assert!(release.pending && resumed.backlog_pending() && !resumed.complete());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn graph_release_cycle_cap_remaining_allowance() {
    use super::super::PackRetentionRepository;

    let db = database().await;
    let releases = seed_inactive_releases(db.pool(), 5).await;
    let budgets = GraphReclamationOptions {
        candidate_limit: 2,
        delete_limit: 3,
        ..options()
    };
    let result = PackRetentionRepository::collect_bounded(db.pool(), Utc::now(), 0, &budgets)
        .await
        .unwrap();
    let release = result
        .phases
        .iter()
        .find(|phase| phase.phase == GraphReclamationPhase::Releases)
        .unwrap();
    assert_eq!(
        (release.changed, release.pages, release.candidates),
        (3, 2, 4)
    );
    assert_eq!(result.deleted_content.len(), 3);
    assert_eq!(release.cursor, releases[3]);
    let progress = PackRetentionRepository::reclamation_progress(db.pool())
        .await
        .unwrap()
        .into_iter()
        .find(|phase| phase.phase == "releases")
        .unwrap();
    assert!(progress.pending && progress.pass_deferred && progress.oversized_since.is_some());
    assert_eq!(progress.high_water, releases[4]);
    assert_eq!(
        PackRetentionRepository::retained_count(db.pool())
            .await
            .unwrap(),
        2
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn graph_release_cycle_cap_crosses_ten_thousand_pins() {
    use super::super::PackRetentionRepository;

    let db = database().await;
    let releases = seed_inactive_releases(db.pool(), 10_241).await;
    let mut tx = db.pool().begin().await.unwrap();
    SchemaTypeRepository::lock_selection(&mut tx).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_done,staging_done) VALUES(990001,990002,990003,true,true)")
        .execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) SELECT 990001,'pack_release',id FROM unnest($1::bigint[]) id")
        .bind(&releases[..10_240]).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let budgets = GraphReclamationOptions {
        delete_limit: 1,
        ..options()
    };
    let result = PackRetentionRepository::collect_bounded(db.pool(), Utc::now(), 0, &budgets)
        .await
        .unwrap();
    let release = result
        .phases
        .iter()
        .find(|phase| phase.phase == GraphReclamationPhase::Releases)
        .unwrap();
    assert_eq!(
        (release.changed, release.candidates, release.pages),
        (1, 10_241, 41)
    );
    assert_eq!(result.deleted_content.len(), 1);
    assert_eq!(
        PackRetentionRepository::retained_count(db.pool())
            .await
            .unwrap(),
        10_240
    );
    assert!(
        result.complete(),
        "pinned-only pages must not spend the release allowance: {result:?}"
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn release_retention_preserves_strict_cutoff_and_newest_inactive_id_ties() {
    use crate::repositories::pack_release::{CreatePackReleaseInput, PackReleaseRepository};
    use serde_json::json;

    let db = database().await;
    let pack = release_test_pack(db.pool()).await;
    // Fixed microsecond precision survives PostgreSQL's timestamptz round trip.
    let cutoff = DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut releases = Vec::new();
    let mut tx = db.pool().begin().await.unwrap();
    for index in 0..5 {
        let release = PackReleaseRepository::create_or_get(
            &mut tx,
            CreatePackReleaseInput {
                pack: pack.id,
                pack_ref: pack.r#ref.clone(),
                version: format!("1.0.{index}"),
                digest: format!("{:064x}", index + 1),
                object_key: format!("collector/{index}.tar.gz"),
                provider_version: "e:owned-fixture".into(),
                content_path: format!("/packs/.releases/collector-{index}"),
                archive_size: 1,
                manifest: json!({}),
            },
        )
        .await
        .unwrap();
        let inactive = match index {
            0..=2 => cutoff - chrono::Duration::days(1),
            3 => cutoff,
            _ => cutoff + chrono::Duration::seconds(1),
        };
        sqlx::query("UPDATE pack_release SET inactive_since=$2 WHERE id=$1")
            .bind(release.id)
            .bind(inactive)
            .execute(&mut *tx)
            .await
            .unwrap();
        releases.push(release.id);
    }
    tx.commit().await.unwrap();
    let budgets = options();
    let first = collect(db.pool(), cutoff, 3, &budgets, CollectionMode::Scheduled)
        .await
        .unwrap();
    assert_eq!(first.deleted_content.len(), 2);
    let kept = PackReleaseRepository::list_by_pack(db.pool(), pack.id)
        .await
        .unwrap();
    assert_eq!(
        kept.iter().map(|release| release.id).collect::<Vec<_>>(),
        releases[2..]
    );

    let second = collect(
        db.pool(),
        cutoff,
        0,
        &budgets,
        CollectionMode::SinglePage { force: true },
    )
    .await
    .unwrap();
    assert_eq!(second.deleted_content.len(), 1);
    let kept = PackReleaseRepository::list_by_pack(db.pool(), pack.id)
        .await
        .unwrap();
    assert_eq!(
        kept.iter().map(|release| release.id).collect::<Vec<_>>(),
        releases[3..]
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn bounded_budget_jit_restores_on_commit_rollback_and_pool_reuse() {
    let db = database().await;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(db.database_url())
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET jit=on").execute(&mut *conn).await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    for fail in [false, true] {
        let mut tx = conn.begin().await.unwrap();
        budget_entry(
            &mut tx,
            Instant::now() + Duration::from_secs(5),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        budget(
            &mut tx,
            Instant::now() + Duration::from_secs(5),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SHOW jit")
                .fetch_one(&mut *tx)
                .await
                .unwrap(),
            "off"
        );
        if fail {
            assert!(sqlx::query("SELECT 1/0").execute(&mut *tx).await.is_err());
            rollback(tx, Duration::from_secs(2)).await.unwrap();
        } else {
            tx.commit().await.unwrap();
        }
        assert_eq!(
            sqlx::query_scalar::<_, String>("SHOW jit")
                .fetch_one(&mut *conn)
                .await
                .unwrap(),
            "on"
        );
    }
    drop(conn);
    let (reused_pid, jit): (i32, String) =
        sqlx::query_as("SELECT pg_backend_pid(), current_setting('jit')")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(reused_pid, pid);
    assert_eq!(jit, "on");
    pool.close().await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn release_json_pins_match_reference_and_recheck_discovered_identities() {
    use serde_json::json;
    let db = database().await;
    let ids = seed_inactive_releases(db.pool(), 10).await;
    // Capture the exact production raw identity before installing pins. The
    // authoritative DELETE must evaluate the new bindings, not discovery state.
    let versions: Vec<String> =
        sqlx::query_scalar("SELECT xmin::text FROM pack_release ORDER BY id")
            .fetch_all(db.pool())
            .await
            .unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    for (entity, contract) in [
        (
            1_i64,
            json!({"static_actions":{"direct":{"executable_snapshot":{"release":{"id":ids[0]}}}}}),
        ),
        (
            2,
            json!({"static_actions":{"outer":{"executable_snapshot":{"pack_executables":{"inner":{"release":{"id":ids[1]}}}}}}}),
        ),
        (3, json!({"static_actions":{}})),
        (4, json!({})),
    ] {
        sqlx::query("INSERT INTO runtime_schema_binding(entity_kind,entity_id,contract,binding_digest) VALUES('execution',$1,$2,repeat('a',64))")
            .bind(entity).bind(contract).execute(&mut *tx).await.unwrap();
    }
    sqlx::query("INSERT INTO sensor_generation_schema_binding(workload,generation,binding,binding_digest,config) VALUES(1,1,$1,repeat('b',64),'{}')")
        .bind(json!({"executable":{"release":{"id":ids[2]}}})).execute(&mut *tx).await.unwrap();
    let snapshot = |id| json!({"pack_executables":{"child":{"release":{"id":id}}}});
    sqlx::query("INSERT INTO execution(action_ref,pack_release_digest,executable_snapshot) VALUES('test.action',repeat('c',64),$1)")
        .bind(snapshot(ids[3])).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO enforcement(rule_ref,trigger_ref,payload,pack_release_digest,executable_snapshot) VALUES('test.rule','test.trigger','{}',repeat('c',64),$1)")
        .bind(snapshot(ids[4])).execute(&mut *tx).await.unwrap();
    let queue: i64 = sqlx::query_scalar("INSERT INTO work_queue(ref,label,dispatch_action_ref) VALUES('test.queue','Test','test.action') RETURNING id")
        .fetch_one(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO work_queue_item(queue,queue_ref,payload,enqueue_source,pack_release_digest,executable_snapshot) VALUES($1,'test.queue','{}','test',repeat('c',64),$2)")
        .bind(queue).bind(snapshot(ids[5])).execute(&mut *tx).await.unwrap();
    // Independent oracle expands the complete JSON pin set once, rather than
    // repeating the correlated production predicates for each release.
    let expected: Vec<i64> = sqlx::query_scalar("WITH pins AS (
        SELECT (child.value->'executable_snapshot'->'release'->>'id')::bigint AS id
        FROM runtime_schema_binding b, LATERAL jsonb_each(COALESCE(b.contract->'static_actions','{}')) child
        UNION SELECT (nested.value->'release'->>'id')::bigint
        FROM runtime_schema_binding b, LATERAL jsonb_each(COALESCE(b.contract->'static_actions','{}')) child,
          LATERAL jsonb_each(COALESCE(child.value->'executable_snapshot'->'pack_executables','{}')) nested
        UNION SELECT (child.value->'release'->>'id')::bigint FROM (
            SELECT executable_snapshot FROM execution UNION ALL SELECT executable_snapshot FROM enforcement
            UNION ALL SELECT executable_snapshot FROM work_queue_item
        ) snapshots, LATERAL jsonb_each(COALESCE(executable_snapshot->'pack_executables','{}')) child
        UNION SELECT (binding->'executable'->'release'->>'id')::bigint FROM sensor_generation_schema_binding
    ) SELECT r.id FROM pack_release r WHERE NOT EXISTS(SELECT 1 FROM pins WHERE pins.id=r.id) ORDER BY r.id")
        .fetch_all(&mut *tx).await.unwrap();
    assert_eq!(expected, ids[6..]);
    let sql = GraphReclamationPhase::Releases.delete_sql(false, false);
    sqlx::query("SAVEPOINT plan_probe")
        .execute(&mut *tx)
        .await
        .unwrap();
    let plan: serde_json::Value =
        sqlx::query_scalar(&format!("EXPLAIN (ANALYZE, FORMAT JSON, TIMING OFF) {sql}"))
            .bind(&ids)
            .bind(&versions)
            .bind(2_i64)
            .bind(Utc::now())
            .bind(0_i64)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    eprintln!(
        "owned positive release fixture planning_ms={} execution_ms={}",
        plan[0]["Planning Time"], plan[0]["Execution Time"]
    );
    sqlx::query("ROLLBACK TO SAVEPOINT plan_probe")
        .execute(&mut *tx)
        .await
        .unwrap();
    let removed = sqlx::query(&sql)
        .bind(&ids)
        .bind(&versions)
        .bind(2_i64)
        .bind(Utc::now())
        .bind(0_i64)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(removed.len(), 2, "delete allowance remains authoritative");
    let remaining: Vec<i64> = sqlx::query_scalar("SELECT id FROM pack_release ORDER BY id")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        remaining,
        [ids[..6].to_vec(), expected[2..].to_vec()].concat()
    );
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}
