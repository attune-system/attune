use futures_core::future::BoxFuture;
use sqlx_core::database::Database;
use std::borrow::Cow;

use crate::error::Error;
use crate::executor::Executor;

use crate::{PgConnection, Postgres};

pub(crate) use sqlx_core::transaction::*;

/// Implementation of [`TransactionManager`] for PostgreSQL.
pub struct PgTransactionManager;

impl TransactionManager for PgTransactionManager {
    type Database = Postgres;

    fn begin<'conn>(
        conn: &'conn mut PgConnection,
        statement: Option<Cow<'static, str>>,
    ) -> BoxFuture<'conn, Result<(), Error>> {
        Box::pin(async move {
            let depth = conn.inner.transaction_depth;
            let statement = match statement {
                // custom `BEGIN` statements are not allowed if we're already in
                // a transaction (we need to issue a `SAVEPOINT` instead)
                Some(_) if depth > 0 => return Err(Error::InvalidSavePointStatement),
                Some(statement) => statement,
                None => begin_ansi_transaction_sql(depth),
            };

            // Drain earlier work before assuming ownership of a new boundary.
            conn.wait_until_ready().await?;
            if depth == 0 && !conn.is_transaction_idle() {
                // Raw transaction SQL is not tracked by SQLx. Do not adopt its
                // boundary or let a failed custom BEGIN roll back its owner.
                return Err(Error::Protocol(
                    "cannot begin inside an untracked transaction".into(),
                ));
            }
            let rollback = Rollback::new(conn);
            if let Err(error) = rollback.conn.queue_simple_query(&statement) {
                rollback.conn.inner.transaction_startup_uncertain = true;
                return Err(error);
            }
            rollback.conn.wait_until_ready().await?;
            if !rollback.conn.in_transaction() {
                return Err(Error::BeginFailed);
            }
            rollback.conn.inner.transaction_depth += 1;
            rollback.defuse();

            Ok(())
        })
    }

    fn commit(conn: &mut PgConnection) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            if conn.inner.transaction_depth > 0 {
                let completion = Completion::new(conn);
                completion
                    .conn
                    .execute(&*commit_ansi_transaction_sql(completion.depth))
                    .await?;
                completion.conn.inner.transaction_depth -= 1;
                completion.defuse();
            }

            Ok(())
        })
    }

    fn rollback(conn: &mut PgConnection) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            if conn.inner.transaction_depth > 0 {
                let completion = Completion::new(conn);
                completion
                    .conn
                    .execute(&*rollback_ansi_transaction_sql(completion.depth))
                    .await?;
                completion.conn.inner.transaction_depth -= 1;
                completion.defuse();
            }

            Ok(())
        })
    }

    fn start_rollback(conn: &mut PgConnection) {
        if std::mem::take(&mut conn.inner.suppress_rollback_once) {
            return;
        }
        if conn.inner.transaction_depth > 0 {
            conn.queue_simple_query(&rollback_ansi_transaction_sql(conn.inner.transaction_depth))
                .expect("BUG: Rollback query somehow too large for protocol");

            conn.inner.transaction_depth -= 1;
        }
    }

    fn get_transaction_depth(conn: &<Self::Database as Database>::Connection) -> usize {
        conn.inner.transaction_depth
    }
}

struct Rollback<'c> {
    conn: &'c mut PgConnection,
    defuse: bool,
}

impl Drop for Rollback<'_> {
    fn drop(&mut self) {
        if !self.defuse {
            static INTERRUPTED_STARTS: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let interrupted_starts = INTERRUPTED_STARTS
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .wrapping_add(1);
            tracing::warn!(
                operation = "begin",
                interrupted_starts,
                depth = self.conn.inner.transaction_depth,
                "transaction startup interrupted; cleanup or quarantine required"
            );
            if self.conn.inner.transaction_depth == 0 {
                // BEGIN may already have reached the server, even if no response
                // has reached the client. Queue cleanup independently of depth.
                if self.conn.queue_simple_query("ROLLBACK").is_err() {
                    self.conn.inner.transaction_startup_uncertain = true;
                }
            } else {
                // SAVEPOINT may not exist. Never roll back the caller's outer
                // transaction on uncertain nested startup; quarantine at reuse.
                self.conn.inner.transaction_startup_uncertain = true;
            }
        }
    }
}

impl<'c> Rollback<'c> {
    fn new(conn: &'c mut PgConnection) -> Self {
        Self {
            conn,
            defuse: false,
        }
    }
    fn defuse(mut self) {
        self.defuse = true;
    }
}

// A consumed nested Transaction will call start_rollback on cancellation/error.
// RELEASE may already have removed the savepoint: another rollback must not
// target a missing savepoint or consume the caller's outer boundary.
struct Completion<'c> {
    conn: &'c mut PgConnection,
    depth: usize,
    defused: bool,
}
impl<'c> Completion<'c> {
    fn new(conn: &'c mut PgConnection) -> Self {
        let depth = conn.inner.transaction_depth;
        Self {
            conn,
            depth,
            defused: false,
        }
    }
    fn defuse(mut self) {
        self.defused = true;
    }
}
impl Drop for Completion<'_> {
    fn drop(&mut self) {
        if !self.defused && self.depth > 1 {
            static INTERRUPTED_COMPLETIONS: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let interrupted_nested_completions = INTERRUPTED_COMPLETIONS
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .wrapping_add(1);
            tracing::warn!(operation = "nested_completion", interrupted_nested_completions, depth = self.depth,
                "nested completion interrupted; session quarantined without consuming caller boundary");
            self.conn.inner.transaction_startup_uncertain = true;
            self.conn.inner.transaction_depth = self.depth - 1;
            self.conn.inner.suppress_rollback_once = true;
        }
    }
}
