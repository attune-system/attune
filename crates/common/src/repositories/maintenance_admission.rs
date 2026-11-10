//! Bounded metadata maintenance. `PhysicalCleanup` is the explicit exception for
//! legacy filesystem cleanup which has no durable physical-work claim.
use crate::{Error, Result};
use futures::future::BoxFuture;
use sqlx::{Connection, PgConnection, PgPool, Postgres, Transaction};
use std::{future::Future, time::Duration};
use tokio::time::{timeout, timeout_at, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct MaintenanceBudget {
    pub statement_timeout: Duration,
    /// Work only. Acknowledged rollback may use one further statement budget.
    pub page_timeout: Duration,
    pub queued_lock_timeout: Duration,
    /// The scheduler may enable one short queued attempt for aged deferrals.
    /// Ordinary maintenance must leave this false.
    pub queued_admission: bool,
}

impl Default for MaintenanceBudget {
    fn default() -> Self {
        Self {
            statement_timeout: Duration::from_millis(250),
            page_timeout: Duration::from_secs(1),
            queued_lock_timeout: Duration::from_millis(50),
            queued_admission: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceOutcome {
    Progress,
    NoWork,
    LockDeferred,
    DeadlineDeferred,
    Failed,
    CommitUnknown,
}

#[derive(Debug)]
pub struct MaintenanceResult<T> {
    pub outcome: MaintenanceOutcome,
    /// Only acknowledged commits can return work for physical cleanup.
    pub value: Option<T>,
    pub candidates: usize,
    pub changed: u64,
    pub error: Option<Error>,
    /// The protected work transaction ended with acknowledged rollback. This
    /// permits advisory failed-visit bookkeeping, never physical cleanup.
    pub rollback_acknowledged: bool,
}

impl<T> MaintenanceResult<T> {
    /// Housekeeping callers must not turn a deferred or unknown transaction into
    /// an empty success. In particular, no physical work follows CommitUnknown.
    pub fn into_result(self) -> Result<Option<T>> {
        match self.outcome {
            MaintenanceOutcome::Progress | MaintenanceOutcome::NoWork => Ok(self.value),
            outcome => Err(self.error.unwrap_or_else(|| {
                Error::invalid_state(format!("Metadata maintenance deferred: {outcome:?}"))
            })),
        }
    }

    pub(crate) fn empty(outcome: MaintenanceOutcome) -> Self {
        Self {
            outcome,
            value: None,
            candidates: 0,
            changed: 0,
            error: None,
            rollback_acknowledged: false,
        }
    }

    pub(crate) fn fail(&mut self, error: Error, end: Instant, cancel: &CancellationToken) {
        self.outcome = match &error {
            Error::Database(sqlx::Error::Database(db)) if db.code().as_deref() == Some("55P03") => {
                MaintenanceOutcome::LockDeferred
            }
            Error::Database(sqlx::Error::Database(db)) if db.code().as_deref() == Some("57014") => {
                MaintenanceOutcome::DeadlineDeferred
            }
            _ if cancel.is_cancelled() || Instant::now() >= end => {
                MaintenanceOutcome::DeadlineDeferred
            }
            _ => MaintenanceOutcome::Failed,
        };
        self.error = Some(error);
    }
}

fn deadline_error() -> Error {
    Error::invalid_state("Metadata maintenance work deadline or cancellation reached")
}

fn milliseconds(duration: Duration) -> String {
    format!("{}ms", duration.as_millis().clamp(1, i32::MAX as u128))
}

async fn limited<T>(
    end: Instant,
    cancel: &CancellationToken,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(deadline_error()),
        result = timeout_at(end, work) => result.unwrap_or_else(|_| Err(deadline_error())),
    }
}

async fn configure(
    conn: &mut PgConnection,
    end: Instant,
    budget: &MaintenanceBudget,
) -> Result<()> {
    let remaining = end
        .checked_duration_since(Instant::now())
        .ok_or_else(deadline_error)?;
    sqlx::query("SELECT set_config('statement_timeout',$1,true)")
        .bind(milliseconds(remaining.min(budget.statement_timeout)))
        .execute(conn)
        .await?;
    Ok(())
}

// Only at owned transaction entry; later budget refreshes need no JIT setter.
async fn configure_entry(
    conn: &mut PgConnection,
    end: Instant,
    budget: &MaintenanceBudget,
) -> Result<()> {
    let remaining = end
        .checked_duration_since(Instant::now())
        .ok_or_else(deadline_error)?;
    sqlx::query("SELECT set_config('statement_timeout',$1,true), set_config('jit','off',true)")
        .bind(milliseconds(remaining.min(budget.statement_timeout)))
        .execute(conn)
        .await?;
    Ok(())
}

async fn rollback(mut tx: Transaction<'_, Postgres>, cap: Duration) -> Result<()> {
    timeout(cap, async {
        // A dropped query future may leave ErrorResponse before ReadyForQuery.
        // Drain it before requesting ROLLBACK, then await acknowledgement.
        if let Err(error) = (*tx).flush().await {
            match error {
                sqlx::Error::Database(ref db)
                    if matches!(db.code().as_deref(), Some("57014" | "55P03")) =>
                {
                    (*tx).flush().await?
                }
                error => return Err(error),
            }
        }
        tx.rollback().await
    })
    .await
    .map_err(|_| {
        Error::invalid_state("Metadata maintenance rollback unconfirmed; owned socket disposed")
    })??;
    Ok(())
}

/// Admission and each metadata phase are bounded. Physical I/O deliberately is
/// not: cancelling an unlink future can leave a blocking thread still unlinking.
/// The caller must await physical work without a timeout and keep this owner
/// alive through completion. Production housekeeping runs in an owned task so
/// cancellation of its waiter cannot drop this fence during filesystem work.
pub struct PhysicalCleanup {
    tx: Transaction<'static, Postgres>,
    budget: MaintenanceBudget,
}

impl PhysicalCleanup {
    pub async fn begin(pool: &PgPool, budget: &MaintenanceBudget) -> Result<Self> {
        if [
            budget.statement_timeout,
            budget.page_timeout,
            budget.queued_lock_timeout,
        ]
        .iter()
        .any(|duration| duration.as_millis() == 0 || duration.as_millis() > i32::MAX as u128)
        {
            return Err(Error::validation("Invalid metadata maintenance budget"));
        }
        let cancel = CancellationToken::new();
        let end = Instant::now() + budget.page_timeout;
        let mut conn = limited(end, &cancel, async { Ok(pool.acquire().await?) }).await?;
        // Never return a cancelled/in-flight transaction to the pool.
        conn.close_on_drop();
        let mut tx = limited(end, &cancel, async {
            Ok(Transaction::begin(conn, None).await?)
        })
        .await?;
        let admitted = limited(end, &cancel, async {
            sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").execute(&mut *tx).await?;
            configure_entry(&mut tx, end, budget).await?;
            if budget.queued_admission {
                let previous: String = sqlx::query_scalar("SELECT current_setting('lock_timeout')").fetch_one(&mut *tx).await?;
                sqlx::query("SELECT set_config('lock_timeout',$1,true)")
                    .bind(milliseconds(budget.queued_lock_timeout)).execute(&mut *tx).await?;
                super::file_evidence_retention::FileEvidenceRetentionRepository::lock_reclamation(&mut tx).await?;
                sqlx::query("SELECT set_config('lock_timeout',$1,true)").bind(previous).execute(&mut *tx).await?;
                Ok(true)
            } else {
                Ok(sqlx::query_scalar::<_,bool>("SELECT pg_try_advisory_xact_lock(hashtextextended(current_schema() || ':named-type-graph',0))")
                    .fetch_one(&mut *tx).await?)
            }
        }).await;
        match admitted {
            Ok(true) => Ok(Self {
                tx,
                budget: budget.clone(),
            }),
            other => {
                rollback(tx, budget.statement_timeout).await?;
                match other {
                    Ok(false) => Err(Error::invalid_state("Physical cleanup admission deferred")),
                    Err(error) => Err(error),
                    Ok(true) => unreachable!(),
                }
            }
        }
    }

    /// Start a fresh metadata deadline after physical I/O. Never wrap physical
    /// work in this method; its timeout would release a still-needed fence.
    pub async fn metadata<T>(
        &mut self,
        work: impl for<'c> FnOnce(&'c mut PgConnection) -> BoxFuture<'c, Result<T>>,
    ) -> Result<T> {
        let end = Instant::now() + self.budget.page_timeout;
        limited(end, &CancellationToken::new(), async {
            configure(&mut self.tx, end, &self.budget).await?;
            work(&mut self.tx).await
        })
        .await
    }

    pub async fn rollback(self) -> Result<()> {
        rollback(self.tx, self.budget.statement_timeout).await
    }

    pub async fn commit(mut self) -> Result<()> {
        let end = Instant::now() + self.budget.page_timeout;
        limited(end, &CancellationToken::new(), async {
            configure(&mut self.tx, end, &self.budget).await?;
            self.tx.commit().await.map_err(|_| {
                Error::invalid_state("Physical cleanup commit unconfirmed; retry required")
            })
        })
        .await
    }
}

/// Read-only candidate discovery with no graph admission. A result is exposed
/// only after acknowledged rollback; cancellation owns the detached socket.
pub(crate) async fn probe<T>(
    pool: &PgPool,
    budget: &MaintenanceBudget,
    deadline: Instant,
    cancel: &CancellationToken,
    read: impl for<'c> FnOnce(&'c mut PgConnection) -> BoxFuture<'c, Result<T>>,
) -> Result<T> {
    if [
        budget.statement_timeout,
        budget.page_timeout,
        budget.queued_lock_timeout,
    ]
    .iter()
    .any(|duration| duration.as_millis() == 0 || duration.as_millis() > i32::MAX as u128)
    {
        return Err(Error::validation("Invalid metadata maintenance budget"));
    }
    let end = deadline.min(Instant::now() + budget.page_timeout);
    let mut conn = limited(end, cancel, async { Ok(pool.acquire().await?.detach()) }).await?;
    let mut tx = limited(end, cancel, async { Ok(conn.begin().await?) }).await?;
    let result = limited(end, cancel, async {
        configure_entry(&mut tx, end, budget).await?;
        read(&mut tx).await
    })
    .await;
    rollback(tx, budget.statement_timeout).await?;
    result
}

/// Bounded discovery progress transaction. Returns data only after acknowledged
/// COMMIT; cancellation/drop disposes the detached session just like `probe`.
pub(crate) async fn discovery<T>(
    pool: &PgPool,
    budget: &MaintenanceBudget,
    deadline: Instant,
    work: impl for<'c> FnOnce(&'c mut PgConnection) -> BoxFuture<'c, Result<T>>,
) -> Result<T> {
    let end = deadline.min(Instant::now() + budget.page_timeout);
    let cancel = CancellationToken::new();
    let mut conn = limited(end, &cancel, async { Ok(pool.acquire().await?.detach()) }).await?;
    let mut tx = limited(end, &cancel, async { Ok(conn.begin().await?) }).await?;
    let result = limited(end, &cancel, async {
        configure_entry(&mut tx, end, budget).await?;
        work(&mut tx).await
    })
    .await;
    match result {
        Ok(value) => {
            limited(end, &cancel, async { Ok(tx.commit().await?) }).await?;
            Ok(value)
        }
        Err(error) => {
            rollback(tx, budget.statement_timeout).await?;
            Err(error)
        }
    }
}

/// Probe without graph admission, end that transaction, then recheck and mutate
/// under a fresh READ COMMITTED snapshot. `work` must recheck candidate identity,
/// eligibility and pins, and return its value plus the number of changed rows.
///
/// The absolute deadline bounds all work, including pool acquisition and COMMIT.
/// Explicit cancellation awaits drain/rollback. Dropping this future closes its
/// detached socket, never returning an in-flight holder to the pool. Socket
/// disposal is not an acknowledgement that the backend has exited.
pub async fn run<C, T>(
    pool: &PgPool,
    budget: &MaintenanceBudget,
    deadline: Instant,
    cancel: &CancellationToken,
    probe: impl for<'c> FnOnce(&'c mut PgConnection) -> BoxFuture<'c, Result<Vec<C>>>,
    work: impl for<'c> FnOnce(&'c mut PgConnection, Vec<C>) -> BoxFuture<'c, Result<(T, u64)>>,
) -> MaintenanceResult<T> {
    let mut report = MaintenanceResult::empty(MaintenanceOutcome::NoWork);
    if [
        budget.statement_timeout,
        budget.page_timeout,
        budget.queued_lock_timeout,
    ]
    .iter()
    .any(|duration| duration.as_millis() == 0 || duration.as_millis() > i32::MAX as u128)
    {
        report.outcome = MaintenanceOutcome::Failed;
        report.error = Some(Error::validation("Invalid metadata maintenance budget"));
        return report;
    }
    let end = deadline.min(Instant::now() + budget.page_timeout);
    let connection = limited(end, cancel, async { Ok(pool.acquire().await?.detach()) }).await;
    let mut conn = match connection {
        Ok(conn) => conn,
        Err(error) => {
            report.fail(error, end, cancel);
            return report;
        }
    };
    let mut discovery = match limited(end, cancel, async { Ok(conn.begin().await?) }).await {
        Ok(tx) => tx,
        Err(error) => {
            report.fail(error, end, cancel);
            return report;
        }
    };
    let candidates = limited(end, cancel, async {
        configure_entry(&mut discovery, end, budget).await?;
        probe(&mut discovery).await
    })
    .await;
    if let Err(error) = rollback(discovery, budget.statement_timeout).await {
        report.outcome = MaintenanceOutcome::Failed;
        report.error = Some(error);
        return report;
    }
    let candidates = match candidates {
        Ok(candidates) => candidates,
        Err(error) => {
            report.fail(error, end, cancel);
            return report;
        }
    };
    report.candidates = candidates.len();
    if candidates.is_empty() {
        return report;
    }
    let transaction = limited(end, cancel, async { Ok(conn.begin().await?) }).await;
    let mut tx = match transaction {
        Ok(tx) => tx,
        Err(error) => {
            report.fail(error, end, cancel);
            return report;
        }
    };
    let result = limited(end, cancel, async {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").execute(&mut *tx).await?;
        configure_entry(&mut tx, end, budget).await?;
        if budget.queued_admission {
            let previous: String = sqlx::query_scalar("SELECT current_setting('lock_timeout')").fetch_one(&mut *tx).await?;
            sqlx::query("SELECT set_config('lock_timeout',$1,true)")
                .bind(milliseconds(budget.queued_lock_timeout.min(end.saturating_duration_since(Instant::now()))))
                .execute(&mut *tx).await?;
            super::file_evidence_retention::FileEvidenceRetentionRepository::lock_reclamation(&mut tx).await?;
            sqlx::query("SELECT set_config('lock_timeout',$1,true)").bind(previous).execute(&mut *tx).await?;
        } else {
            let admitted: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended(current_schema() || ':named-type-graph',0))")
                .fetch_one(&mut *tx).await?;
            if !admitted { return Ok(None); }
        }
        configure(&mut tx, end, budget).await?;
        work(&mut tx, candidates).await.map(Some)
    }).await;
    match result {
        Ok(Some((value, changed))) => {
            if cancel.is_cancelled() || Instant::now() >= end {
                if let Err(error) = rollback(tx, budget.statement_timeout).await {
                    report.outcome = MaintenanceOutcome::Failed;
                    report.error = Some(error);
                } else {
                    report.rollback_acknowledged = true;
                    report.fail(deadline_error(), end, cancel);
                }
                return report;
            }
            // Once COMMIT is started, cancellation/error cannot prove rollback.
            // Never return claims or authorize physical work on an unknown commit.
            match limited(end, cancel, async { Ok(tx.commit().await?) }).await {
                Ok(()) => {
                    report.outcome = if changed == 0 {
                        MaintenanceOutcome::NoWork
                    } else {
                        MaintenanceOutcome::Progress
                    };
                    report.value = Some(value);
                    report.changed = changed;
                }
                Err(error) => {
                    report.outcome = MaintenanceOutcome::CommitUnknown;
                    report.error = Some(error);
                }
            }
        }
        other => {
            if let Err(error) = rollback(tx, budget.statement_timeout).await {
                report.outcome = MaintenanceOutcome::Failed;
                report.error = Some(error);
                return report;
            }
            report.rollback_acknowledged = true;
            match other {
                Ok(None) => report.outcome = MaintenanceOutcome::LockDeferred,
                Err(error) => report.fail(error, end, cancel),
                Ok(Some(_)) => unreachable!(),
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, test_database::TestDatabase};

    #[tokio::test]
    async fn jit_is_local_to_owned_metadata_transactions() {
        let config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let db = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let schema = db.schema().to_owned();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |conn, _| {
                let schema = schema.clone();
                Box::pin(async move {
                    sqlx::query("SELECT set_config('search_path',quote_ident($1)||', public',false), set_config('jit','on',false)")
                        .bind(schema).execute(conn).await?;
                    Ok(())
                })
            })
            .connect(db.database_url())
            .await
            .unwrap();
        let budget = MaintenanceBudget {
            statement_timeout: Duration::from_secs(2),
            page_timeout: Duration::from_secs(10),
            ..MaintenanceBudget::default()
        };
        // Exercise the actual discovery and protected-work owners, including SQL failure.
        for fail in [false, true] {
            let report = run(
                &pool,
                &budget,
                Instant::now() + budget.page_timeout,
                &CancellationToken::new(),
                |conn| {
                    Box::pin(async move {
                        let jit: String = sqlx::query_scalar("SHOW jit").fetch_one(conn).await?;
                        assert_eq!(jit, "off");
                        Ok(vec![1])
                    })
                },
                move |conn, _| {
                    Box::pin(async move {
                        let jit: String =
                            sqlx::query_scalar("SHOW jit").fetch_one(&mut *conn).await?;
                        assert_eq!(jit, "off");
                        if fail {
                            sqlx::query("SELECT 1/0").execute(conn).await?;
                        }
                        Ok(((), 1))
                    })
                },
            )
            .await;
            assert_eq!(
                report.outcome,
                if fail {
                    MaintenanceOutcome::Failed
                } else {
                    MaintenanceOutcome::Progress
                }
            );
            if fail {
                assert!(report.rollback_acknowledged);
            }
            let jit: String = sqlx::query_scalar("SHOW jit")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(jit, "on");
        }
        let discovered = probe(
            &pool,
            &budget,
            Instant::now() + budget.page_timeout,
            &CancellationToken::new(),
            |conn| {
                Box::pin(async move {
                    Ok(sqlx::query_scalar::<_, String>("SHOW jit")
                        .fetch_one(conn)
                        .await?)
                })
            },
        )
        .await
        .unwrap();
        assert_eq!(discovered, "off");
        for commit in [false, true] {
            let mut owner = PhysicalCleanup::begin(&pool, &budget).await.unwrap();
            let jit = owner
                .metadata(|conn| {
                    Box::pin(async move {
                        Ok(sqlx::query_scalar::<_, String>("SHOW jit")
                            .fetch_one(conn)
                            .await?)
                    })
                })
                .await
                .unwrap();
            assert_eq!(jit, "off");
            if commit {
                owner.commit().await.unwrap();
            } else {
                owner.rollback().await.unwrap();
            }
            assert_eq!(
                sqlx::query_scalar::<_, String>("SHOW jit")
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                "on"
            );
        }
        // Check restoration on the very same reusable backend after commit,
        // server timeout and rollback (production owners conservatively dispose).
        let mut conn = pool.acquire().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        for fail in [false, true] {
            let mut tx = conn.begin().await.unwrap();
            configure_entry(&mut tx, Instant::now() + budget.page_timeout, &budget)
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
                sqlx::query("SET LOCAL statement_timeout='20ms'")
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                let error = sqlx::query("SELECT pg_sleep(1)")
                    .execute(&mut *tx)
                    .await
                    .unwrap_err();
                assert_eq!(
                    error.as_database_error().unwrap().code().as_deref(),
                    Some("57014")
                );
                rollback(tx, budget.statement_timeout).await.unwrap();
            } else {
                tx.commit().await.unwrap();
            }
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
}
