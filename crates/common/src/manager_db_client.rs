//! Lifecycle of the optional manager-only PostgreSQL connection.

use std::{num::NonZeroU32, path::PathBuf, sync::Arc, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine};
use ring::{
    digest, hmac, pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use sqlx::{
    postgres::{PgConnectOptions, PgSslMode},
    ConnectOptions, Connection, PgConnection,
};
use tokio::sync::watch;
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

pub struct ManagerDatabaseConfig {
    pod_token: PathBuf,
    database: PgConnectOptions,
    role: WorkerRole,
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

        Ok(Some(Self {
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
        let mut active: Option<PgConnection> = None;
        let mut generation = 1;
        while !stop.is_cancelled() {
            let operation = self.obtain(&api, &sessions, generation);
            let result = tokio::select! {
                biased;
                _ = stop.cancelled() => break,
                result = operation => result,
            };
            match result {
                Ok((connection, rotate_in)) => {
                    if let Some(old) = active.replace(connection) {
                        let _ = old.close().await;
                    }
                    tracing::info!(
                        generation,
                        "Restricted manager database credential connected"
                    );
                    generation += 1;
                    if wait(&stop, rotate_in).await {
                        break;
                    }
                }
                Err(_) => {
                    tracing::warn!(
                        "Restricted manager database credential loop stopped; restart required"
                    );
                    break;
                }
            }
        }
        if let Some(connection) = active {
            let _ = connection.close().await;
        }
    }

    async fn obtain(
        &self,
        api: &ManagerControlClient,
        sessions: &watch::Receiver<Arc<ManagerSession>>,
        generation: i64,
    ) -> Result<(PgConnection, Duration)> {
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
                                return Ok((db, delay));
                            }
                            let _ = db.close().await;
                        }
                    }
                }
            }
            tokio::time::sleep(RETRY).await;
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

async fn wait(stop: &CancellationToken, delay: Duration) -> bool {
    tokio::select! {
        _ = stop.cancelled() => true,
        _ = tokio::time::sleep(delay) => false,
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
