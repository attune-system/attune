//! A pooled leader socket is reusable only after acknowledged advisory unlock.
use anyhow::{bail, Result};
use attune_common::repositories::retention::RetentionRepository;
use sqlx::{pool::PoolConnection, PgPool, Postgres};

pub(super) struct LeaderSession {
    connection: Option<PoolConnection<Postgres>>,
    key: i64,
}

impl LeaderSession {
    pub(super) async fn acquire(pool: &PgPool, key: i64) -> Result<Option<Self>> {
        let mut session = Self {
            connection: Some(pool.acquire().await?),
            key,
        };
        if RetentionRepository::try_advisory_lock(
            session.connection.as_mut().unwrap().as_mut(),
            key,
        )
        .await?
        {
            Ok(Some(session))
        } else {
            // The failed try acquired no lock and changed no session settings.
            session.return_clean().await;
            Ok(None)
        }
    }

    pub(super) async fn release(mut self) -> Result<()> {
        if !RetentionRepository::advisory_unlock(
            self.connection.as_mut().unwrap().as_mut(),
            self.key,
        )
        .await?
        {
            bail!("Supervisor advisory unlock was not acknowledged as held; socket discarded");
        }
        // This owner exposes no mutable session access. Only the two repository
        // advisory operations run on it: no open transaction or changed settings.
        self.return_clean().await;
        Ok(())
    }

    async fn return_clean(mut self) {
        if let Some(mut connection) = self.connection.take() {
            // Await SQLx's normal readiness/health check and pool return. The
            // socket is already proven unlocked; cancellation here can discard it.
            connection.return_to_pool().await;
        }
    }
}

impl Drop for LeaderSession {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            // Error, panic or cancellation before acknowledged unlock: close
            // directly instead of returning an unknown session to the pool.
            drop(connection.detach());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::{config::Config, test_database::TestDatabase};
    use sqlx::postgres::PgPoolOptions;
    use std::time::Duration;

    async fn fixture() -> (TestDatabase, PgPool, i64) {
        let config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let db = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .min_connections(0)
            .connect_with((*db.pool().connect_options()).clone())
            .await
            .unwrap();
        let key = (uuid::Uuid::new_v4().as_u128() & i64::MAX as u128) as i64;
        (db, pool, key)
    }

    async fn pid(session: &mut LeaderSession) -> i32 {
        sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(session.connection.as_mut().unwrap().as_mut())
            .await
            .unwrap()
    }

    async fn gone(db: &PgPool, id: i32) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1)",
                )
                .bind(id)
                .fetch_one(db)
                .await
                .unwrap();
                if !exists {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("discarded leader backend must exit");
    }

    #[tokio::test]
    async fn acknowledged_unlock_reuses_session_and_allows_another_leader() {
        let (db, pool, key) = fixture().await;
        let mut first = LeaderSession::acquire(&pool, key).await.unwrap().unwrap();
        let first_pid = pid(&mut first).await;
        first.release().await.unwrap();
        let mut contender = db.acquire().await.unwrap();
        assert!(RetentionRepository::try_advisory_lock(&mut contender, key)
            .await
            .unwrap());
        assert!(LeaderSession::acquire(&pool, key).await.unwrap().is_none());
        assert!(RetentionRepository::advisory_unlock(&mut contender, key)
            .await
            .unwrap());
        drop(contender);
        let mut second = LeaderSession::acquire(&pool, key).await.unwrap().unwrap();
        assert_eq!(first_pid, pid(&mut second).await);
        second.release().await.unwrap();
        pool.close().await;
        db.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn dropped_owner_discards_socket_instead_of_pooling_its_lock() {
        let (db, pool, key) = fixture().await;
        let mut first = LeaderSession::acquire(&pool, key).await.unwrap().unwrap();
        let first_pid = pid(&mut first).await;
        drop(first);
        gone(db.pool(), first_pid).await;
        let mut second = LeaderSession::acquire(&pool, key).await.unwrap().unwrap();
        assert_ne!(first_pid, pid(&mut second).await);
        second.release().await.unwrap();
        pool.close().await;
        db.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_owner_releases_lock_and_discards_session() {
        let (db, pool, key) = fixture().await;
        let owned_pool = pool.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let owner = tokio::spawn(async move {
            let mut session = LeaderSession::acquire(&owned_pool, key)
                .await
                .unwrap()
                .unwrap();
            ready_tx.send(pid(&mut session).await).unwrap();
            std::future::pending::<()>().await;
            session.release().await.unwrap();
        });
        let first_pid = ready_rx.await.unwrap();
        owner.abort();
        assert!(owner.await.unwrap_err().is_cancelled());
        gone(db.pool(), first_pid).await;
        let mut replacement = LeaderSession::acquire(&pool, key).await.unwrap().unwrap();
        assert_ne!(first_pid, pid(&mut replacement).await);
        replacement.release().await.unwrap();
        pool.close().await;
        db.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn unacknowledged_unlock_discards_session() {
        let (db, pool, key) = fixture().await;
        let mut session = LeaderSession::acquire(&pool, key).await.unwrap().unwrap();
        let first_pid = pid(&mut session).await;
        // Simulate a broken ownership invariant: even a successful SQL response
        // is insufficient unless it confirms that this session held the lock.
        assert!(RetentionRepository::advisory_unlock(
            session.connection.as_mut().unwrap().as_mut(),
            key
        )
        .await
        .unwrap());
        assert!(session.release().await.is_err());
        gone(db.pool(), first_pid).await;
        let mut replacement = LeaderSession::acquire(&pool, key).await.unwrap().unwrap();
        assert_ne!(first_pid, pid(&mut replacement).await);
        replacement.release().await.unwrap();
        pool.close().await;
        db.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn failed_unlock_never_reuses_the_unknown_socket() {
        let (db, pool, key) = fixture().await;
        let mut first = LeaderSession::acquire(&pool, key).await.unwrap().unwrap();
        let first_pid = pid(&mut first).await;
        assert!(
            sqlx::query_scalar::<_, bool>("SELECT pg_terminate_backend($1)")
                .bind(first_pid)
                .fetch_one(db.pool())
                .await
                .unwrap()
        );
        assert!(first.release().await.is_err());
        gone(db.pool(), first_pid).await;
        let mut second = LeaderSession::acquire(&pool, key).await.unwrap().unwrap();
        assert_ne!(first_pid, pid(&mut second).await);
        second.release().await.unwrap();
        pool.close().await;
        db.cleanup().await.unwrap();
    }
}
