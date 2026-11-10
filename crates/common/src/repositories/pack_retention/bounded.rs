use super::PackReleaseContent;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use sqlx::{Connection, PgConnection, PgPool, Postgres, Transaction};
use std::collections::VecDeque;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tracing::Instrument;

/// Separate raw probe, successful-delete, statement, page and cycle budgets.
/// Durations must be nonzero and fit a PostgreSQL millisecond setting.
#[derive(Debug, Clone)]
pub struct GraphReclamationOptions {
    pub candidate_limit: i64,
    /// Per-page delete cap for graph leaves, and per-invocation cap for releases.
    pub delete_limit: i64,
    pub statement_timeout: Duration,
    /// Page work deadline. Cleanup/retry bookkeeping can use three further
    /// statement budgets, reserved within cycle_timeout, without new graph work.
    pub page_timeout: Duration,
    /// Must exceed three statement budgets so cleanup has bounded reserved time.
    pub cycle_timeout: Duration,
    pub queued_lock_timeout: Duration,
    pub max_deferral: Duration,
    pub retry_delay: Duration,
    /// Delay between budget-limited scan continuations, not lock/error retries.
    pub continuation_delay: Duration,
    /// Delay before revisiting a completed clean pass's fixed high-water range.
    pub rescan_delay: Duration,
}

impl Default for GraphReclamationOptions {
    fn default() -> Self {
        Self {
            candidate_limit: 256,
            delete_limit: 100,
            statement_timeout: Duration::from_millis(250),
            page_timeout: Duration::from_secs(1),
            cycle_timeout: Duration::from_secs(10),
            queued_lock_timeout: Duration::from_millis(50),
            max_deferral: Duration::from_secs(300),
            retry_delay: Duration::from_secs(30),
            continuation_delay: Duration::from_secs(1),
            rescan_delay: Duration::from_secs(3600),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphReclamationPhase {
    Receipts,
    AcceptedInputs,
    NativeSnapshots,
    CacheEpochs,
    Admissions,
    PolicyRevisions,
    Releases,
}

impl GraphReclamationPhase {
    pub(super) const ALL: [Self; 7] = [
        Self::Receipts,
        Self::AcceptedInputs,
        Self::NativeSnapshots,
        Self::CacheEpochs,
        Self::Admissions,
        Self::PolicyRevisions,
        Self::Releases,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Receipts => "receipts",
            Self::AcceptedInputs => "accepted_inputs",
            Self::NativeSnapshots => "native_snapshots",
            Self::CacheEpochs => "cache_epochs",
            Self::Admissions => "admissions",
            Self::PolicyRevisions => "policy_revisions",
            Self::Releases => "releases",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphReclamationOutcome {
    Progress,
    NoWork,
    NotDue,
    Superseded,
    LockDeferred,
    DeadlineDeferred,
    Failed,
    CommitUnknown,
}

#[derive(Debug)]
pub struct GraphReclamationPhaseResult {
    pub phase: GraphReclamationPhase,
    pub outcome: GraphReclamationOutcome,
    pub candidates: usize,
    pub changed: u64,
    /// Last confirmed cursor. Zero means the next page starts a new pass.
    pub cursor: i64,
    /// Unvisited pages, a truncated DELETE, or an unresolved deferral remain.
    pub pending: bool,
    pub pages: usize,
    pub elapsed: Duration,
    pub error: Option<Error>,
}

/// Seven durable rows for operator status, independent of outer job success.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GraphReclamationProgress {
    pub phase: String,
    pub cursor: i64,
    pub high_water: i64,
    pub probe_limit: i64,
    /// This pass encountered over-budget work or a possibly truncated DELETE.
    pub pass_deferred: bool,
    pub oversized_since: Option<DateTime<Utc>>,
    pub next_attempt: Option<DateTime<Utc>>,
    pub last_attempt: Option<DateTime<Utc>>,
    pub last_progress: Option<DateTime<Utc>>,
    /// Oldest unresolved admission/deadline deferral or incomplete DELETE visit.
    pub deferred_since: Option<DateTime<Utc>>,
    pub pending: bool,
}

#[derive(Debug, Default)]
pub struct GraphReclamationResult {
    pub phases: Vec<GraphReclamationPhaseResult>,
    /// Only confirmed commits. Unknown commits never authorize physical cleanup.
    pub deleted_content: Vec<PackReleaseContent>,
}

impl GraphReclamationResult {
    /// Every phase completed its current pass without unresolved scan work.
    /// This does not prove that retained rows cannot become eligible later.
    pub fn complete(&self) -> bool {
        self.phases.len() == GraphReclamationPhase::ALL.len()
            && !self.backlog_pending()
            && self.phases.iter().all(|phase| {
                matches!(
                    phase.outcome,
                    GraphReclamationOutcome::Progress
                        | GraphReclamationOutcome::NoWork
                        | GraphReclamationOutcome::NotDue
                )
            })
    }

    pub fn backlog_pending(&self) -> bool {
        self.phases.len() != GraphReclamationPhase::ALL.len()
            || self.phases.iter().any(|phase| phase.pending)
    }

    /// Compatibility callers requested one bounded page, not a complete scan.
    pub(super) fn pages_succeeded(&self) -> bool {
        self.phases.iter().all(|phase| {
            matches!(
                phase.outcome,
                GraphReclamationOutcome::Progress | GraphReclamationOutcome::NoWork
            )
        })
    }
}

#[derive(Clone, sqlx::FromRow)]
struct Progress {
    phase: String,
    cursor_id: i64,
    high_water: i64,
    probe_limit: i64,
    revision: i64,
    last_attempt: Option<DateTime<Utc>>,
    due: bool,
    aged: bool,
    pending: bool,
}

#[derive(Clone, Copy)]
pub(super) enum CollectionMode {
    Scheduled,
    /// Existing direct collection is explicitly one forced page per phase.
    Compatibility,
    #[cfg(test)]
    SinglePage {
        force: bool,
    },
}

impl CollectionMode {
    fn force(self) -> bool {
        match self {
            Self::Scheduled => false,
            Self::Compatibility => true,
            #[cfg(test)]
            Self::SinglePage { force } => force,
        }
    }

    fn single_page(self) -> bool {
        !matches!(self, Self::Scheduled)
    }
}

struct PagePolicy<'a> {
    cutoff: DateTime<Utc>,
    newest: i64,
    options: &'a GraphReclamationOptions,
    compatibility_wait: bool,
    #[cfg(test)]
    lose_commit_ack: bool,
}

// Advisory visit progress only, recorded after a timed-out DELETE's rollback
// was acknowledged. A deferred leaf is revisited on the next bounded pass.
struct DeleteRetry {
    probe_limit: i64,
    cursor: Option<i64>,
    high_water: i64,
}

enum DeferredProgress {
    Committed(Option<i64>),
    CommitUnknown,
}

// One outer admission per page, not the reentrant calls made by DELETE guards.
// Drop records only uncertainty. It cannot acknowledge rollback/backend exit.
struct AdmissionTrace {
    span: tracing::Span,
    requested_at: Option<Instant>,
    acquired_at: Option<Instant>,
}

impl AdmissionTrace {
    fn new() -> Self {
        Self {
            span: tracing::Span::current(),
            requested_at: None,
            acquired_at: None,
        }
    }

    fn request(&mut self, method: &'static str) {
        self.requested_at = Some(Instant::now());
        tracing::debug!(parent: &self.span, event = "requested", mode = "exclusive", method,
            "Graph reclamation admission");
    }

    fn acquired(&mut self) {
        self.acquired_at = Some(Instant::now());
        if let Some(requested) = self.requested_at.take() {
            tracing::debug!(parent: &self.span, event = "acquired", mode = "exclusive",
                wait_us = requested.elapsed().as_micros() as u64, "Graph reclamation admission");
        }
    }

    fn deferred(&mut self) {
        if let Some(requested) = self.requested_at.take() {
            tracing::debug!(parent: &self.span, event = "deferred", mode = "exclusive",
                wait_us = requested.elapsed().as_micros() as u64, "Graph reclamation admission");
        }
    }

    fn acquisition_unconfirmed(&self) {
        if let Some(requested) = self.requested_at {
            tracing::debug!(parent: &self.span, event = "acquisition_unconfirmed", mode = "exclusive",
                wait_us = requested.elapsed().as_micros() as u64, "Graph reclamation admission");
        }
    }

    fn completed(&mut self, completion: &'static str) {
        if let Some(acquired) = self.acquired_at.take() {
            tracing::debug!(parent: &self.span, event = "transaction_completed", completion,
                mode = "exclusive", hold_us = acquired.elapsed().as_micros() as u64,
                "Graph reclamation admission");
        }
        if let Some(requested) = self.requested_at.take() {
            // A cancelled/failed acquisition statement did not acknowledge a
            // holder, but the subsequent rollback did acknowledge transaction end.
            tracing::debug!(parent: &self.span, event = "transaction_completed", completion,
                acquisition = "unconfirmed", request_to_end_us = requested.elapsed().as_micros() as u64,
                "Graph reclamation admission");
        }
    }

    fn unconfirmed(&mut self, disposition: &'static str) {
        if let Some(acquired) = self.acquired_at.take() {
            // This is time observed while ownership was unresolved, NOT a
            // completed hold duration. Socket disposal is not backend-gone proof.
            tracing::debug!(parent: &self.span, event = "outcome_unknown", disposition,
                mode = "exclusive", observed_hold_us = acquired.elapsed().as_micros() as u64,
                "Graph reclamation admission");
        }
        if let Some(requested) = self.requested_at.take() {
            tracing::debug!(parent: &self.span, event = "outcome_unknown", disposition,
                acquisition = "unconfirmed", request_observed_us = requested.elapsed().as_micros() as u64,
                "Graph reclamation admission");
        }
    }
}

impl Drop for AdmissionTrace {
    fn drop(&mut self) {
        self.unconfirmed("owner_dropped_cleanup_unacknowledged");
    }
}

fn deadline_error() -> Error {
    Error::invalid_state("Graph reclamation operation deadline reached")
}

fn milliseconds(duration: Duration) -> u64 {
    duration.as_millis().clamp(1, i32::MAX as u128) as u64
}

async fn budget(conn: &mut PgConnection, end: Instant, cap: Duration) -> Result<()> {
    budget_with_lock_timeout(conn, end, cap, None, false).await
}

async fn budget_entry(conn: &mut PgConnection, end: Instant, cap: Duration) -> Result<()> {
    budget_with_lock_timeout(conn, end, cap, None, true).await
}

async fn budget_with_lock_timeout(
    conn: &mut PgConnection,
    end: Instant,
    cap: Duration,
    lock_timeout: Option<&str>,
    entry: bool,
) -> Result<()> {
    let remaining = end
        .checked_duration_since(Instant::now())
        .ok_or_else(deadline_error)?;
    sqlx::query("SELECT set_config('statement_timeout',$1,true),CASE WHEN $2::text IS NOT NULL THEN set_config('lock_timeout',$2,true) END, CASE WHEN $3 THEN set_config('jit','off',true) END")
        .bind(format!("{}ms", milliseconds(remaining.min(cap))))
        .bind(lock_timeout)
        .bind(entry)
        .execute(conn)
        .await?;
    Ok(())
}

// Work deadlines do not bypass transaction cleanup. Allow one statement budget
// for acknowledged rollback, then dispose the owned socket on an unconfirmed end.
async fn rollback(mut tx: Transaction<'_, Postgres>, cap: Duration) -> Result<()> {
    timeout_at(Instant::now() + cap, async {
        // As in native_maintenance::summaries, a dropped SQLx future may leave
        // ErrorResponse ahead of ReadyForQuery. Consume a pending cancellation
        // before requesting rollback, or SQLx returns it without sending ROLLBACK.
        // Do not suppress unrelated SQL/protocol failures or authorize visit
        // progress from an unconfirmed end.
        if let Err(error) = (*tx).flush().await {
            match error {
                sqlx::Error::Database(ref db) if db.code().as_deref() == Some("57014") => {
                    (*tx).flush().await?
                }
                other => return Err(other),
            }
        }
        tx.rollback().await
    })
    .await
    .map_err(|_| {
        Error::invalid_state("Graph reclamation rollback unconfirmed; owned connection disposed")
    })??;
    Ok(())
}

#[tracing::instrument(name = "graph_reclamation_collection", level = "debug", skip_all,
    fields(collection_id = %uuid::Uuid::new_v4()))]
pub(super) async fn collect(
    pool: &PgPool,
    cutoff: DateTime<Utc>,
    newest: i64,
    options: &GraphReclamationOptions,
    mode: CollectionMode,
) -> Result<GraphReclamationResult> {
    if !(1..=256).contains(&options.candidate_limit)
        || !(1..=256).contains(&options.delete_limit)
        || [
            options.statement_timeout,
            options.page_timeout,
            options.cycle_timeout,
            options.queued_lock_timeout,
            options.max_deferral,
            options.retry_delay,
            options.continuation_delay,
            options.rescan_delay,
        ]
        .iter()
        .any(|value| value.as_millis() == 0 || value.as_millis() > i32::MAX as u128)
        || options.cycle_timeout <= options.statement_timeout * 3
    {
        return Err(Error::validation("Invalid graph reclamation budget"));
    }
    // A timed-out final page still owns rollback and durable deferral. Reserve
    // their worst-case three statement budgets instead of starting fresh work
    // at the advertised cycle deadline and extending it with bookkeeping.
    let cycle_end = Instant::now() + options.cycle_timeout - options.statement_timeout * 3;
    let mut conn = timeout_at(cycle_end, pool.acquire())
        .await
        .map_err(|_| deadline_error())??
        .detach();
    // Cancellation must never return an in-flight graph holder to the pool.
    // Detach owns the socket, so Drop closes it without a pool-return task.
    // This conservative collector-local disposal does not extend the specialized
    // API reclamation CancellationOwner to unrelated maintenance transactions.
    let mut tx = timeout_at(cycle_end, conn.begin())
        .await
        .map_err(|_| deadline_error())??;
    let states = timeout_at(cycle_end, async {
        budget_entry(&mut tx, cycle_end, options.statement_timeout).await?;
        Ok::<_,Error>(sqlx::query_as::<_, Progress>(
         "SELECT phase,cursor_id,high_water,probe_limit,revision,last_attempt,next_attempt<=clock_timestamp() AS due,
                 COALESCE(deferred_since<=clock_timestamp()-($1::bigint*interval '1 millisecond'),false) AS aged,
                 cursor_id<>0 OR high_water<>0 OR pass_deferred OR deferred_since IS NOT NULL OR oversized_since IS NOT NULL AS pending
         FROM graph_reclamation_progress",
    ).bind(milliseconds(options.max_deferral) as i64).fetch_all(&mut *tx).await?)
    }).await.unwrap_or_else(|_| Err(deadline_error()));
    let states = match states {
        Ok(states) => {
            rollback(tx, options.statement_timeout).await?;
            states
        }
        Err(error) => {
            rollback(tx, options.statement_timeout).await?;
            return Err(error);
        }
    };
    drop(conn);
    let mut phases: Vec<_> = GraphReclamationPhase::ALL
        .into_iter()
        .map(|phase| {
            states
                .iter()
                .find(|state| state.phase == phase.name())
                .map(|state| (phase, state.clone()))
                .ok_or_else(|| Error::invalid_state("Graph reclamation progress row missing"))
        })
        .collect::<Result<_>>()?;
    // A short cycle resumes at the least recently attempted phase after restart.
    // Stable sorting preserves the original dependency order on the first pass.
    phases.sort_by_key(|(_, state)| state.last_attempt);
    let mut result = GraphReclamationResult::default();
    // Release cleanup is capped across the entire invocation. Raw pinned pages
    // spend no allowance; other leaf phases retain their independent page caps.
    let mut release_options = options.clone();
    let mut queue: VecDeque<_> = phases.into();
    while let Some((phase, mut state)) = queue.pop_front() {
        let phase_span = tracing::debug_span!("graph_reclamation_phase", phase = phase.name());
        let started = Instant::now();
        let mut report = GraphReclamationPhaseResult {
            phase,
            outcome: GraphReclamationOutcome::NotDue,
            candidates: 0,
            changed: 0,
            cursor: state.cursor_id,
            pending: state.pending || state.due,
            pages: 0,
            elapsed: Duration::ZERO,
            error: None,
        };
        if (state.due || mode.force()) && started < cycle_end {
            report.pages = 1;
            report.pending = true;
            let end = cycle_end.min(started + options.page_timeout);
            let mut retry = None;
            let policy = PagePolicy {
                cutoff,
                newest,
                options: if phase == GraphReclamationPhase::Releases {
                    &release_options
                } else {
                    options
                },
                compatibility_wait: matches!(mode, CollectionMode::Compatibility),
                #[cfg(test)]
                lose_commit_ack: false,
            };
            match page(
                pool,
                phase,
                &mut state,
                &policy,
                end,
                &mut report,
                &mut retry,
            )
            .instrument(phase_span.clone())
            .await
            {
                Ok(content) => {
                    if phase == GraphReclamationPhase::Releases {
                        // page reports changes and content only after COMMIT is
                        // acknowledged. An unknown commit stops the collection.
                        release_options.delete_limit -= report.changed as i64;
                    }
                    result.deleted_content.extend(content);
                }
                Err(error) => {
                    report.outcome = match &error {
                        _ if report.outcome == GraphReclamationOutcome::Failed => {
                            GraphReclamationOutcome::Failed
                        }
                        Error::Database(sqlx::Error::Database(db))
                            if db.code().as_deref() == Some("55P03") =>
                        {
                            GraphReclamationOutcome::LockDeferred
                        }
                        Error::Database(sqlx::Error::Database(db))
                            if db.code().as_deref() == Some("57014") =>
                        {
                            GraphReclamationOutcome::DeadlineDeferred
                        }
                        _ if Instant::now() >= end => GraphReclamationOutcome::DeadlineDeferred,
                        _ => GraphReclamationOutcome::Failed,
                    };
                    report.error = Some(error);
                }
            }
            if matches!(
                report.outcome,
                GraphReclamationOutcome::LockDeferred
                    | GraphReclamationOutcome::DeadlineDeferred
                    | GraphReclamationOutcome::Failed
            ) {
                // Separate, non-graph transaction AFTER awaited rollback. Retry
                // deadlines use completion-time DB clock and bounded jitter.
                match defer(pool, phase, state.revision, options, retry)
                    .instrument(phase_span.clone())
                    .await
                {
                    Ok(DeferredProgress::Committed(Some(cursor))) => report.cursor = cursor,
                    Ok(DeferredProgress::Committed(None)) => {
                        report.outcome = GraphReclamationOutcome::Superseded
                    }
                    Ok(DeferredProgress::CommitUnknown) => {
                        report.outcome = GraphReclamationOutcome::CommitUnknown
                    }
                    Err(error) => {
                        report.outcome = GraphReclamationOutcome::Failed;
                        report.error = Some(error);
                    }
                }
            }
        } else if state.due {
            report.outcome = GraphReclamationOutcome::DeadlineDeferred;
        }
        report.elapsed = started.elapsed();
        tracing::debug!(parent: &phase_span, event = "phase_result", outcome = ?report.outcome,
            candidates = report.candidates as u64, changed = report.changed,
            elapsed_us = report.elapsed.as_micros() as u64, has_error = report.error.is_some(),
            "Graph reclamation phase completed");
        // Identifiers stay in restricted traces, never fixed-category counters.
        tracing::trace!(parent: &phase_span, cursor = report.cursor, "Graph reclamation cursor");
        let unknown = report.outcome == GraphReclamationOutcome::CommitUnknown;
        // Only a confirmed successful page can continue in this invocation.
        // Deferrals leave the queue until their durable retry deadline, even if
        // another phase or a queued acquisition has consumed very little time.
        let continue_pass = !mode.single_page()
            && (phase != GraphReclamationPhase::Releases || release_options.delete_limit > 0)
            && report.cursor != 0
            && matches!(
                report.outcome,
                GraphReclamationOutcome::Progress | GraphReclamationOutcome::NoWork
            );
        if let Some(previous) = result.phases.iter_mut().find(|entry| entry.phase == phase) {
            report.candidates += previous.candidates;
            report.changed += previous.changed;
            report.pages += previous.pages;
            report.elapsed += previous.elapsed;
            if report.outcome == GraphReclamationOutcome::NoWork && previous.changed > 0 {
                report.outcome = GraphReclamationOutcome::Progress;
            }
            *previous = report;
        } else {
            result.phases.push(report);
        }
        if unknown {
            break;
        }
        if continue_pass && Instant::now() < cycle_end {
            // Round-robin one independent transaction per page. Never start a
            // second pass here, even when a truncated DELETE requests a rescan.
            state.due = true;
            queue.push_back((phase, state));
        }
        tokio::task::yield_now().await;
    }
    Ok(result)
}

async fn page(
    pool: &PgPool,
    phase: GraphReclamationPhase,
    state: &mut Progress,
    policy: &PagePolicy<'_>,
    end: Instant,
    report: &mut GraphReclamationPhaseResult,
    retry: &mut Option<DeleteRetry>,
) -> Result<Vec<PackReleaseContent>> {
    let (cutoff, newest, options, compatibility_wait) = (
        policy.cutoff,
        policy.newest,
        policy.options,
        policy.compatibility_wait,
    );
    let mut conn = timeout_at(end, pool.acquire())
        .await
        .map_err(|_| deadline_error())??
        .detach();
    let (table, id, _) = phase.identity();
    let probe_limit = state.probe_limit.min(options.candidate_limit);
    // Raw primary-key page, with NO pin/eligibility predicates before LIMIT.
    // Capture xmin as well as PK so replacement/changed tuples are only revisited
    // on a later pass, never deleted under a stale discovery identity.
    let mut discovery = timeout_at(end, conn.begin())
        .await
        .map_err(|_| deadline_error())??;
    let candidates = timeout_at(end, async {
        budget_entry(&mut discovery, end, options.statement_timeout).await?;
        // Freeze each pass's upper bound. Continuous insertion must not stop
        // wrapping and hide previously pinned or late-committing older IDs.
        let high_water = if state.high_water == 0 {
            sqlx::query_scalar::<_, Option<i64>>(&format!("SELECT max({id}) FROM {table}"))
                .fetch_one(&mut *discovery)
                .await?
                .unwrap_or(0)
        } else {
            state.high_water
        };
        budget(&mut discovery, end, options.statement_timeout).await?;
        let rows: Vec<(i64, String)> = sqlx::query_as(&format!(
            "SELECT {id},xmin::text FROM {table} WHERE {id}>$1 AND {id}<=$3 ORDER BY {id} LIMIT $2"
        ))
        .bind(state.cursor_id)
        .bind(probe_limit)
        .bind(high_water)
        .fetch_all(&mut *discovery)
        .await?;
        Ok::<_, Error>((rows, high_water))
    })
    .await
    .unwrap_or_else(|_| Err(deadline_error()));
    let (candidates, high_water) = match candidates {
        Ok(rows) => {
            rollback(discovery, options.statement_timeout).await?;
            rows
        }
        Err(error) => {
            if let Err(cleanup) = rollback(discovery, options.statement_timeout).await {
                report.outcome = GraphReclamationOutcome::Failed;
                return Err(cleanup);
            }
            return Err(error);
        }
    };
    report.candidates = candidates.len();
    let cursor = if candidates.len() < probe_limit as usize {
        0
    } else {
        candidates.last().unwrap().0
    };
    let mut tx = timeout_at(end, conn.begin())
        .await
        .map_err(|_| deadline_error())??;
    let mut admission = AdmissionTrace::new();
    let mut delete_in_flight = false;
    let mut pending = true;
    let work = timeout_at(end, async {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").execute(&mut *tx).await?;
        budget_entry(&mut tx, end, options.statement_timeout).await?;
        let mut previous_lock_timeout=None;
        if !candidates.is_empty() {
            // The admission statement's snapshot is never used for deletion.
            // Empty pages need only cursor progress, not graph admission.
            if state.aged || compatibility_wait {
                previous_lock_timeout=Some(sqlx::query_scalar::<_,String>(
                    "WITH previous AS MATERIALIZED (SELECT current_setting('lock_timeout') AS value)
                     SELECT value FROM previous WHERE set_config('lock_timeout',$1,true) IS NOT NULL")
                    .bind(format!("{}ms",milliseconds(options.queued_lock_timeout.min(end.saturating_duration_since(Instant::now())))))
                    .fetch_one(&mut *tx).await?);
                admission.request("queued");
                super::super::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx).await?;
                admission.acquired();
            } else {
                admission.request("try");
                let admitted: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended(current_schema() || ':named-type-graph',0))")
                    .fetch_one(&mut *tx).await?;
                if !admitted {
                    admission.deferred();
                    return Ok(None);
                }
                admission.acquired();
            }
        }
        // Restore only our queued-acquisition override. Dependent work keeps the
        // original connection policy and the separate page/statement budgets.
        budget_with_lock_timeout(&mut tx, end, options.statement_timeout,previous_lock_timeout.as_deref(),false).await?;
        let revision: i64 = sqlx::query_scalar("SELECT revision FROM graph_reclamation_progress WHERE phase=$1 FOR UPDATE")
            .bind(phase.name()).fetch_one(&mut *tx).await?;
        if revision != state.revision {
            report.outcome = GraphReclamationOutcome::Superseded;
            return Ok(Some(Vec::new()));
        }
        let mut content = Vec::new();
        let changed = if candidates.is_empty() { 0 } else {
            budget(&mut tx, end, options.statement_timeout).await?;
            let (accepted, queued): (bool,bool) = sqlx::query_as("SELECT to_regclass('work_queue_item_ad_hoc_dispatch') IS NOT NULL,to_regclass('work_queue_item_runtime_admission') IS NOT NULL")
                .fetch_one(&mut *tx).await?;
            budget(&mut tx, end, options.statement_timeout).await?;
            let (ids, versions): (Vec<_>,Vec<_>) = candidates.iter().cloned().unzip();
            let sql = phase.delete_sql(accepted, queued);
            delete_in_flight = true;
            let changed = if phase == GraphReclamationPhase::Releases {
                content = sqlx::query_as(&sql).bind(&ids).bind(&versions).bind(options.delete_limit)
                    .bind(cutoff).bind(newest).fetch_all(&mut *tx).await?;
                content.len() as u64
            } else {
                sqlx::query(&sql).bind(&ids).bind(&versions).bind(options.delete_limit)
                    .bind(cutoff).bind(newest).execute(&mut *tx).await?.rows_affected()
            };
            delete_in_flight = false;
            changed
        };
        budget(&mut tx, end, options.statement_timeout).await?;
        pending = sqlx::query_scalar("WITH attempt AS MATERIALIZED (SELECT clock_timestamp() AS now)
            UPDATE graph_reclamation_progress SET cursor_id=$2,revision=revision+1,high_water=$4,
            last_attempt=attempt.now,
            next_attempt=attempt.now+(CASE WHEN $2=0 AND NOT pass_deferred AND NOT $6 THEN $8 ELSE $7 END::bigint*interval '1 millisecond'),
            deferred_since=CASE WHEN $2=0 AND NOT pass_deferred AND NOT $6 THEN NULL ELSE deferred_since END,
            probe_limit=$5,
            oversized_since=CASE WHEN $2=0 AND NOT pass_deferred AND NOT $6 THEN NULL
                WHEN $6 THEN COALESCE(oversized_since,attempt.now) ELSE oversized_since END,
            pass_deferred=CASE WHEN $2=0 THEN false ELSE pass_deferred OR $6 END,
            last_progress=CASE WHEN $3 THEN attempt.now ELSE last_progress END FROM attempt WHERE phase=$1
            RETURNING cursor_id<>0 OR deferred_since IS NOT NULL OR oversized_since IS NOT NULL")
            .bind(phase.name()).bind(cursor).bind(changed>0).bind(if cursor==0 {0} else {high_water})
            .bind(options.candidate_limit)
            // A full DELETE budget may leave an oversized raw candidate untried.
            // Such a pass cannot prove that earlier oversized visits are resolved.
            .bind(changed==options.delete_limit as u64 && candidates.len()>changed as usize)
            .bind(milliseconds(options.continuation_delay) as i64)
            .bind(milliseconds(options.rescan_delay) as i64)
            .fetch_one(&mut *tx).await?;
        report.changed = changed;
        Ok::<_,Error>(Some(content))
    }).await;
    let work_timed_out = work.is_err();
    let work = work.unwrap_or_else(|_| Err(deadline_error()));
    match work {
        Ok(Some(content)) => {
            // A commit transport error does not establish rollback. The metadata
            // and cursor are atomic/idempotent; the next call reads durable state.
            let committed = matches!(timeout_at(end, tx.commit()).await, Ok(Ok(())));
            // Owning-seam fault injection discards an acknowledged COMMIT. The
            // collector must treat this exactly like a lost transport response.
            #[cfg(test)]
            let committed = committed && !policy.lose_commit_ack;
            if !committed {
                drop(conn);
                admission.unconfirmed("commit_unknown_socket_disposed_unacknowledged");
                report.changed = 0;
                report.outcome = GraphReclamationOutcome::CommitUnknown;
                return Ok(Vec::new());
            }
            admission.completed("commit_acknowledged");
            if report.outcome != GraphReclamationOutcome::Superseded {
                report.cursor = cursor;
                report.pending = pending;
                state.cursor_id = cursor;
                state.high_water = if cursor == 0 { 0 } else { high_water };
                state.probe_limit = options.candidate_limit;
                state.revision += 1;
                state.pending = pending;
                report.outcome = if report.changed > 0 {
                    GraphReclamationOutcome::Progress
                } else {
                    GraphReclamationOutcome::NoWork
                };
            }
            Ok(content)
        }
        Ok(None) => {
            if let Err(cleanup) = rollback(tx, options.statement_timeout).await {
                drop(conn);
                admission.unconfirmed("rollback_unknown_socket_disposed_unacknowledged");
                report.outcome = GraphReclamationOutcome::Failed;
                return Err(cleanup);
            }
            admission.completed("rollback_acknowledged");
            report.outcome = GraphReclamationOutcome::LockDeferred;
            Ok(Vec::new())
        }
        Err(error) => {
            report.changed = 0;
            admission.acquisition_unconfirmed();
            if let Err(cleanup) = rollback(tx, options.statement_timeout).await {
                drop(conn);
                admission.unconfirmed("rollback_unknown_socket_disposed_unacknowledged");
                report.outcome = GraphReclamationOutcome::Failed;
                return Err(cleanup);
            }
            admission.completed("rollback_acknowledged");
            let statement_timed_out = matches!(&error,
                Error::Database(sqlx::Error::Database(db)) if db.code().as_deref()==Some("57014"));
            if delete_in_flight && (work_timed_out || statement_timed_out) {
                // Do not infer a bad leaf from probe, admission, bookkeeping, or
                // generic SQL failures. Only this confirmed rollback can authorize
                // adaptive visit progress in the separate non-graph transaction.
                let single = candidates.len() == 1;
                *retry = Some(DeleteRetry {
                    probe_limit: if single {
                        options.candidate_limit
                    } else {
                        (candidates.len() as i64 / 2).max(1)
                    },
                    cursor: single.then(|| candidates[0].0),
                    high_water,
                });
                tracing::debug!(
                    event = "delete_retry",
                    action = if single { "defer_leaf" } else { "shrink_probe" },
                    "Graph reclamation over-budget visit"
                );
            }
            Err(error)
        }
    }
}

async fn defer(
    pool: &PgPool,
    phase: GraphReclamationPhase,
    revision: i64,
    options: &GraphReclamationOptions,
    retry: Option<DeleteRetry>,
) -> Result<DeferredProgress> {
    let end = Instant::now() + options.statement_timeout;
    let mut conn = timeout_at(end, pool.acquire())
        .await
        .map_err(|_| deadline_error())??
        .detach();
    let mut tx = timeout_at(end, conn.begin())
        .await
        .map_err(|_| deadline_error())??;
    let update = timeout_at(end,async {
        budget_entry(&mut tx,end,options.statement_timeout).await?;
        let cursor=sqlx::query_scalar("UPDATE graph_reclamation_progress SET last_attempt=clock_timestamp(),
            next_attempt=clock_timestamp()+($3::bigint*(0.8+random()*0.4)*interval '1 millisecond'),
            deferred_since=COALESCE(deferred_since,clock_timestamp()),revision=revision+1,
            probe_limit=COALESCE($4::bigint,probe_limit),cursor_id=COALESCE($5::bigint,cursor_id),
            high_water=COALESCE($6::bigint,high_water),pass_deferred=pass_deferred OR $4 IS NOT NULL,
            oversized_since=CASE WHEN $4 IS NOT NULL THEN COALESCE(oversized_since,clock_timestamp()) ELSE oversized_since END
            WHERE phase=$1 AND revision=$2 RETURNING cursor_id")
            .bind(phase.name()).bind(revision).bind(milliseconds(options.retry_delay) as i64)
            .bind(retry.as_ref().map(|retry|retry.probe_limit)).bind(retry.as_ref().and_then(|retry|retry.cursor))
            .bind(retry.as_ref().map(|retry|retry.high_water)).fetch_optional(&mut *tx).await?;
        Ok::<_,Error>(cursor)
    }).await.unwrap_or_else(|_| Err(deadline_error()));
    match update {
        Ok(cursor) => {
            if !matches!(timeout_at(end, tx.commit()).await, Ok(Ok(()))) {
                return Ok(DeferredProgress::CommitUnknown);
            }
            Ok(DeferredProgress::Committed(cursor))
        }
        Err(error) => {
            rollback(tx, options.statement_timeout).await?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests;
