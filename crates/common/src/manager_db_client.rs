//! Lifecycle of the optional manager-only PostgreSQL connection.

use std::{
    io,
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Utc};
use ring::{
    digest, hmac, pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use sqlx::{
    postgres::{PgConnectOptions, PgSslMode},
    ConnectOptions, Connection, PgConnection,
};
use tokio::{sync::watch, time::Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    db::ConnectionIdentity,
    manager_control::{ManagerControlClient, ManagerDatabaseCredentialRequest, ManagerSession},
    models::WorkerRole,
    Error, Result,
};

const ROUNDS: NonZeroU32 = NonZeroU32::new(4096).unwrap();
const RETRY: Duration = Duration::from_secs(2);
const HEALTH_INTERVAL: Duration = Duration::from_secs(5);
const READY_MAX_AGE: Duration = Duration::from_secs(15);
const READY_FILE_ENV: &str = "ATTUNE_MANAGER_POSTGRES_READY_FILE";

/// Kubernetes uses this local check without opening another database connection.
pub fn check_database_ready() -> Result<()> {
    let path = std::env::var_os(READY_FILE_ENV)
        .map(PathBuf::from)
        .ok_or_else(|| Error::configuration("Missing manager database readiness file"))?;
    if recent_readiness(&path, SystemTime::now()) {
        Ok(())
    } else {
        Err(Error::invalid_state(
            "Restricted manager database is unavailable",
        ))
    }
}

fn recent_readiness(path: &Path, now: SystemTime) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|updated| now.duration_since(updated).ok())
        .is_some_and(|age| age <= READY_MAX_AGE)
}

fn clear_readiness(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn refresh_readiness(path: &Path) -> io::Result<()> {
    let staging = path.with_extension("tmp");
    std::fs::write(&staging, [])?;
    std::fs::rename(staging, path)
}

struct ScopedConnection {
    db: PgConnection,
    expires_at: DateTime<Utc>,
    rotate_at: Instant,
}

pub struct ManagerDatabaseConfig {
    pod_token: PathBuf,
    database: PgConnectOptions,
    role: WorkerRole,
    readiness: Option<PathBuf>,
}

impl ManagerDatabaseConfig {
    pub fn from_environment(role: WorkerRole) -> Result<Option<Self>> {
        let Some(location) = std::env::var_os("ATTUNE_MANAGER_POD_TOKEN_FILE") else {
            return Ok(None);
        };
        let url = std::env::var("ATTUNE_MANAGER_POSTGRES_URL")
            .map_err(|_| Error::configuration("Missing manager PostgreSQL URL"))?;
        let schema =
            std::env::var("ATTUNE_MANAGER_POSTGRES_SCHEMA").unwrap_or_else(|_| "attune".to_owned());
        if schema.is_empty()
            || schema.len() > 63
            || !schema
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        {
            return Err(Error::configuration("Invalid manager PostgreSQL schema"));
        }
        let database = ConnectionIdentity::for_process("manager", "scoped-postgres")
            .connect_options(&url)
            .map_err(|_| Error::configuration("Invalid manager PostgreSQL URL"))?;
        if !matches!(database.get_ssl_mode(), PgSslMode::VerifyFull) {
            return Err(Error::configuration(
                "Manager PostgreSQL requires sslmode=verify-full",
            ));
        }

        let readiness = std::env::var_os(READY_FILE_ENV).map(PathBuf::from);
        if let Some(path) = &readiness {
            clear_readiness(path)
                .map_err(|_| Error::configuration("Could not clear manager database readiness"))?;
        }
        Ok(Some(Self {
            readiness,
            pod_token: location.into(),
            database: database
                .options([
                    ("search_path", schema.as_str()),
                    ("statement_timeout", "5000"),
                ])
                .disable_statement_logging(),
            role,
        }))
    }

    pub async fn run(
        self,
        api: ManagerControlClient,
        sessions: watch::Receiver<Arc<ManagerSession>>,
        stop: CancellationToken,
    ) {
        let mut active: Option<ScopedConnection> = None;
        let mut health = tokio::time::interval(HEALTH_INTERVAL);
        health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut generation = 1;
        while !stop.is_cancelled() {
            // Keep checking the old connection while the next credential is
            // being verified, issued, and used to open its replacement.
            let rotate_at = active.as_ref().map(|connection| connection.rotate_at);
            let next_generation = generation;
            let operation = async {
                if let Some(rotate_at) = rotate_at {
                    tokio::time::sleep_until(rotate_at).await;
                }
                self.obtain(&api, &sessions, next_generation).await
            };
            tokio::pin!(operation);
            let result = loop {
                tokio::select! {
                    biased;
                    _ = stop.cancelled() => break None,
                    result = &mut operation => break Some(result),
                    _ = health.tick(), if active.is_some() => {
                        let session = sessions.borrow().clone();
                        let connection = active.as_mut().expect("health branch requires a connection");
                        let healthy = connection.expires_at > Utc::now()
                            && tokio::time::timeout(
                                HEALTH_INTERVAL,
                                self.inspect(&mut connection.db, &session),
                            ).await.is_ok_and(|result| result.unwrap_or(false));
                        self.set_ready(healthy);
                    }
                }
            };
            let Some(result) = result else { break };
            match result {
                Ok(connection) => {
                    self.set_ready(connection.expires_at > Utc::now());
                    if let Some(old) = active.replace(connection) {
                        let _ = old.db.close().await;
                    }
                    tracing::info!(
                        generation,
                        "Restricted manager database credential connected"
                    );
                    generation += 1;
                }
                Err(_) => {
                    tracing::warn!(
                        "Restricted manager database credential loop stopped; restart required"
                    );
                    break;
                }
            }
        }
        self.set_ready(false);
        if let Some(connection) = active {
            let _ = connection.db.close().await;
        }
    }

    async fn obtain(
        &self,
        api: &ManagerControlClient,
        sessions: &watch::Receiver<Arc<ManagerSession>>,
        generation: i64,
    ) -> Result<ScopedConnection> {
        let random = SystemRandom::new();
        let mut password = [0_u8; 32];
        let mut salt = [0_u8; 16];
        random
            .fill(&mut password)
            .and_then(|_| random.fill(&mut salt))
            .map_err(|_| Error::internal("Manager password randomness unavailable"))?;
        let password = STANDARD.encode(password);
        let mut request = ManagerDatabaseCredentialRequest {
            request: Uuid::new_v4(),
            generation,
            verifier: scram_verifier(&password, &salt),
            pod_token: String::new(),
        };
        let mut submitted = None;
        loop {
            let session = sessions.borrow().clone();
            if submitted
                .is_none_or(|time: tokio::time::Instant| time.elapsed() >= Duration::from_secs(30))
            {
                if let Ok(token) = tokio::fs::read_to_string(&self.pod_token).await {
                    request.pod_token = token.trim().to_owned();
                    match api.request_database_credential(&session, &request).await {
                        Ok(()) => submitted = Some(tokio::time::Instant::now()),
                        Err(Error::InvalidState(_)) => {
                            return Err(Error::invalid_state("Manager credential was retired"))
                        }
                        Err(_) => {}
                    }
                    request.pod_token.clear();
                }
            }
            if submitted.is_some() {
                if let Ok(Some(lease)) = api.database_credential(&session).await {
                    if lease.request == request.request && lease.generation == generation {
                        let options = self
                            .database
                            .clone()
                            .username(&lease.username)
                            .password(&password);
                        if let Ok(Ok(mut db)) = tokio::time::timeout(
                            Duration::from_secs(5),
                            PgConnection::connect_with(&options),
                        )
                        .await
                        {
                            if self.inspect(&mut db, &session).await.unwrap_or(false) {
                                let delay = Duration::from_secs(
                                    u64::try_from(lease.rotate_after_seconds).unwrap_or(2),
                                );
                                return Ok(ScopedConnection {
                                    db,
                                    expires_at: lease.expires_at,
                                    rotate_at: Instant::now() + delay,
                                });
                            }
                            let _ = db.close().await;
                        }
                    }
                }
            }
            tokio::time::sleep(RETRY).await;
        }
    }

    fn set_ready(&self, ready: bool) {
        if let Some(path) = &self.readiness {
            let result = if ready {
                refresh_readiness(path)
            } else {
                clear_readiness(path)
            };
            if result.is_err() {
                tracing::warn!("Could not update restricted manager database readiness");
            }
        }
    }

    async fn inspect(&self, db: &mut PgConnection, session: &ManagerSession) -> Result<bool> {
        crate::repositories::manager_db_login::ManagerDatabaseLoginRepository::inspect(
            db,
            self.role,
            session.instance,
            session.id,
        )
        .await
    }
}

fn scram_verifier(password: &str, salt: &[u8; 16]) -> String {
    let mut salted = [0; 32];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        ROUNDS,
        salt,
        password.as_bytes(),
        &mut salted,
    );
    let key = hmac::Key::new(hmac::HMAC_SHA256, &salted);
    let client = hmac::sign(&key, b"Client Key");
    let stored = digest::digest(&digest::SHA256, client.as_ref());
    let server = hmac::sign(&key, b"Server Key");
    format!(
        "SCRAM-SHA-256${}:{}${}:{}",
        ROUNDS,
        STANDARD.encode(salt),
        STANDARD.encode(stored.as_ref()),
        STANDARD.encode(server.as_ref())
    )
}

pub fn valid_scram_verifier(verifier: &str) -> bool {
    if verifier.len() != 133 {
        return false;
    }
    let Some((salt, keys)) = verifier
        .strip_prefix("SCRAM-SHA-256$4096:")
        .and_then(|parts| parts.split_once('$'))
    else {
        return false;
    };
    let Some((stored, server)) = keys.split_once(':') else {
        return false;
    };
    [(salt, 16), (stored, 32), (server, 32)]
        .into_iter()
        .all(|(value, expected)| {
            STANDARD
                .decode(value)
                .is_ok_and(|bytes| bytes.len() == expected)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_requires_a_recent_health_check() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ready");
        assert!(!recent_readiness(&path, SystemTime::now()));
        refresh_readiness(&path).unwrap();
        assert!(recent_readiness(&path, SystemTime::now()));
        assert!(!recent_readiness(
            &path,
            SystemTime::now() + READY_MAX_AGE + Duration::from_secs(1),
        ));
        clear_readiness(&path).unwrap();
        assert!(!recent_readiness(&path, SystemTime::now()));
    }

    #[test]
    fn scram_verifier_matches_postgres_format() {
        let verifier = scram_verifier("scram-fixture", &std::array::from_fn(|index| index as u8));
        assert_eq!(
            verifier,
            concat!(
                "SCRAM-SHA-256$4096:AAECAwQFBgcICQoLDA0ODw==$",
                "JHzPT0hucPWRqpnXVj/0KeNOra93TxgpsavZN1UL2IE=:",
                "oojK2u9iw0pP20tFnjfPP6DGIFxUpNxOXwiF9z42Sag=",
            )
        );
        assert!(valid_scram_verifier(&verifier));
        assert!(!valid_scram_verifier(&verifier.replace("$4096:", "$8192:")));
        assert!(!valid_scram_verifier("a plain password"));
    }
}
