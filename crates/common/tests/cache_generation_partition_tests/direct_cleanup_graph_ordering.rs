use super::{fixture, limits};
use anyhow::{ensure, Result};
use attune_common::repositories::{
    cache::{
        CacheEntryRepository, CacheGenerationCleanupOutcome, CacheGenerationRepository,
        CacheTransactionMode,
    },
    schema_type::SchemaTypeRepository,
};
use std::time::Duration;

#[tokio::test]
async fn direct_cleanup_cache_wait_allows_shared_graph_admission() -> Result<()> {
    let (db, generation) = fixture().await;
    let evidence: Result<_> = async {
        let pool = db.pool();
        let before: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT u.record_count, u.physical_bytes, \
             (SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id=1), \
             (SELECT COALESCE(SUM(physical_bytes),0)::BIGINT FROM cache_owner_physical_byte_usage) \
             FROM cache_generation_entry_usage u WHERE u.generation=$1",
        )
        .bind(generation)
        .fetch_one(pool)
        .await?;
        // Reserve O before C starts. W, O, and C are the only active sessions.
        let mut observer = pool.begin().await?;
        sqlx::query("SELECT set_config('statement_timeout','2s',true)")
            .execute(&mut *observer)
            .await?;
        let mut holder = pool.begin().await?;
        sqlx::query(
            "SELECT set_config('lock_timeout','5s',true), \
             set_config('statement_timeout','5s',true)",
        )
        .execute(&mut *holder)
        .await?;
        // Real scheduler entry takes PinMutation before shared graph selection.
        CacheEntryRepository::protect_transaction(&mut holder, CacheTransactionMode::PinMutation)
            .await?;
        let (holder_pid, graph_already_held): (i32, bool) =
            sqlx::query_as("SELECT pg_backend_pid(), file_evidence_graph_held(false)")
                .fetch_one(&mut *holder)
                .await?;
        ensure!(!graph_already_held, "fixture acquired graph before the interleave");

        let config = limits(12_000);
        let cleanup = CacheGenerationRepository::drop_if_cleanup_eligible(pool, generation, &config);
        let interleave = async {
            let admission = tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let waiting: Option<(i32, bool, i64)> = sqlx::query_as(
                        "SELECT waiter.pid, \
                         EXISTS(SELECT 1 FROM pg_locks g WHERE g.pid=waiter.pid \
                           AND g.locktype='advisory' AND g.objsubid=1 AND g.granted \
                           AND g.mode='ExclusiveLock' AND g.database=waiter.database \
                           AND g.classid=((hashtextextended(current_schema() || ':named-type-graph',0)>>32)&4294967295)::OID \
                           AND g.objid=(hashtextextended(current_schema() || ':named-type-graph',0)&4294967295)::OID), \
                         (SELECT COUNT(*) FROM pg_locks source WHERE source.pid=waiter.pid \
                           AND source.relation IN ('cache_entry'::regclass,'cache_namespace'::regclass, \
                             'cache_generation'::regclass,'cache_generation_entry_usage'::regclass, \
                             'workflow_execution'::regclass,'workflow_cache_iteration'::regclass, \
                             'cache_deployment_physical_byte_usage'::regclass,'cache_owner_physical_byte_usage'::regclass)) \
                         FROM pg_locks waiter JOIN pg_locks holder \
                           ON holder.locktype=waiter.locktype AND holder.classid=waiter.classid \
                           AND holder.objid=waiter.objid AND holder.objsubid=waiter.objsubid \
                           AND holder.database=waiter.database \
                         WHERE waiter.locktype='advisory' AND waiter.classid=7821101 \
                           AND waiter.objid=0 AND waiter.objsubid=2 \
                           AND waiter.mode='ExclusiveLock' AND NOT waiter.granted \
                           AND holder.pid=$1 AND holder.mode='ExclusiveLock' AND holder.granted \
                           AND $1=ANY(pg_blocking_pids(waiter.pid))",
                    )
                    .bind(holder_pid)
                    .fetch_optional(&mut *observer)
                    .await?;
                    if let Some(waiting) = waiting {
                        return Ok::<_, sqlx::Error>(waiting);
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await;
            // Do not fail on retained G yet. Exercise the real conflicting call.
            // No cancelling timeout surrounds this SQL or the combined join.
            let graph = if matches!(&admission, Ok(Ok(_))) {
                Some(SchemaTypeRepository::lock_selection(&mut holder).await)
            } else {
                None
            };
            // A failed statement may have aborted W. Always release it before
            // draining C, including observer errors and readiness timeouts.
            let released = holder.rollback().await;
            (admission, graph, released)
        };
        let (outcome, (admission, graph, released)) = tokio::join!(cleanup, interleave);
        let observer_released = observer.rollback().await;
        let after = sqlx::query_as::<_, (bool, bool, i64, i64)>(
            "SELECT EXISTS(SELECT 1 FROM cache_generation WHERE id=$1), \
             EXISTS(SELECT 1 FROM cache_generation_entry_usage WHERE generation=$1), \
             (SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id=1), \
             (SELECT COALESCE(SUM(physical_bytes),0)::BIGINT FROM cache_owner_physical_byte_usage)",
        )
        .bind(generation)
        .fetch_one(pool)
        .await;
        Ok((before, admission, graph, released, outcome, observer_released, after))
    }
    .await;
    // Even an unexpected query/setup error is reported after owned DB cleanup.
    // Once C starts, no fallible escape precedes W resolution and C drain.
    let fixture_cleanup = db.cleanup().await;
    ensure!(
        fixture_cleanup.is_ok(),
        "fixture cleanup failed: {fixture_cleanup:?}; evidence={evidence:?}"
    );
    let (before, admission, graph, released, outcome, observer_released, after) = evidence?;
    ensure!(
        matches!(&admission, Ok(Ok((_, false, 0))))
            && matches!(&graph, Some(Ok(())))
            && released.is_ok()
            && observer_released.is_ok(),
        "direct cleanup blocked shared graph admission: admission={admission:?}, \
         graph={graph:?}, holder_release={released:?}, cleanup={outcome:?}, \
         observer_release={observer_released:?}, after={after:?}"
    );
    let records = u64::try_from(before.0)?;
    let bytes = u64::try_from(before.1)?;
    ensure!(
        matches!(&outcome, Ok(CacheGenerationCleanupOutcome::Dropped { records: r, bytes: b })
            if *r == records && *b == bytes),
        "cleanup failed after holder release: {outcome:?}"
    );
    ensure!(
        after? == (false, false, before.2 - before.1, before.3 - before.1),
        "cleanup did not release exact owned usage"
    );
    Ok(())
}
