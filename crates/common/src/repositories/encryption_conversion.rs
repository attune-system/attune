//! Bounded offline compatibility conversion, not a root-key rotation service.
//! Generic ciphertext, user JSON, origins and external evidence bytes stay intact.
use crate::{crypto, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};
mod graph;
mod integrity;

/// Old verifier bytes are AES material. Keep them private and never print them.
pub(in crate::repositories) struct VerifierConversion<'a> {
    master: &'a str,
    legacy: String,
    current: String,
}
impl<'a> VerifierConversion<'a> {
    pub(in crate::repositories) fn new(master: &'a str) -> Result<Self> {
        if master.len() < 32 {
            return Err(Error::encryption(
                "Encryption key must be at least 32 characters",
            ));
        }
        Ok(Self {
            master,
            legacy: Sha256::digest(master.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            current: crypto::hash_encryption_key(master),
        })
    }
    pub(in crate::repositories) fn verifier(&self, value: &mut String) -> Result<()> {
        require_known_verifier(Some(value), &self.legacy, &self.current)?;
        *value = self.current.clone();
        Ok(())
    }
    pub(in crate::repositories) fn field(
        &self,
        value: &Value,
        fingerprint: &mut Option<String>,
    ) -> Result<()> {
        require_known_verifier(fingerprint.as_deref(), &self.legacy, &self.current)?;
        crypto::decrypt_json(value, self.master)?;
        if let Some(fingerprint) = fingerprint {
            self.verifier(fingerprint)?;
        }
        Ok(())
    }
    pub(in crate::repositories) fn prepared(
        &self,
        field: &mut crate::secret_values::PreparedSecretValue,
    ) -> Result<()> {
        crypto::decrypt_json(&field.encrypted_value, self.master)?;
        self.verifier(&mut field.encryption_key_hash)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionReport {
    pub keys_converted: usize,
    pub keys_already_current: usize,
    pub field_verifiers_converted: usize,
    pub fields_already_current: usize,
    pub graphs_converted: usize,
    pub applied: bool,
}

/// All service writers must be stopped. Dedicated maintenance membership and
/// exclusive graph/table locks are required; ordinary runtime writes cannot use
/// the exact-row maintenance permits. Dry-run authenticates without mutations.
pub async fn convert_offline(pool: &PgPool, master: &str, apply: bool) -> Result<ConversionReport> {
    VerifierConversion::new(master)?;
    let mut tx = pool.begin().await?;
    match convert_locked(&mut tx, master, apply).await {
        Ok(report) => {
            if apply {
                tx.commit().await?;
            } else {
                tx.rollback().await?;
            }
            Ok(report)
        }
        Err(error) => {
            tx.rollback().await?;
            Err(error)
        }
    }
}

async fn convert_locked(
    conn: &mut PgConnection,
    master: &str,
    apply: bool,
) -> Result<ConversionReport> {
    sqlx::query("SET LOCAL TIME ZONE 'UTC'")
        .execute(&mut *conn)
        .await?;
    sqlx::query("SET LOCAL lock_timeout = '5s'")
        .execute(&mut *conn)
        .await?;
    sqlx::query("SET LOCAL statement_timeout = '30min'")
        .execute(&mut *conn)
        .await?;
    sqlx::query("SELECT encryption_conversion_lock()")
        .execute(&mut *conn)
        .await?;
    let before = integrity::validate(conn).await?;
    let (changes, mut report) = graph::changes(conn, master).await?;
    if !apply {
        return Ok(report);
    }
    report.applied = true;
    if changes.is_empty() {
        return Ok(report);
    }
    let owned: i64 = sqlx::query_scalar("SELECT encryption_conversion_begin($1)")
        .bind(uuid::Uuid::new_v4())
        .fetch_one(&mut *conn)
        .await?;
    for change in changes {
        sqlx::query("SELECT encryption_conversion_apply($1,$2,$3,$4,$5)")
            .bind(owned)
            .bind(change.table)
            .bind(change.identity)
            .bind(change.before)
            .bind(change.values)
            .execute(&mut *conn)
            .await?;
    }
    if integrity::validate(conn).await? != before {
        return Err(Error::validation(
            "Encryption conversion changed retained origins, pins or byte accounting",
        ));
    }
    if !graph::changes(conn, master).await?.0.is_empty() {
        return Err(graph::corrupt());
    }
    sqlx::query("SELECT encryption_conversion_finish($1,$2)")
        .bind(owned)
        .bind(serde_json::to_value(&report)?)
        .execute(conn)
        .await?;
    Ok(report)
}

fn require_known_verifier(value: Option<&str>, legacy: &str, shared: &str) -> Result<()> {
    if value.is_none_or(|value| value == legacy || value == shared) {
        return Ok(());
    }
    Err(Error::encryption(
        "Unknown stored verifier; conversion rolled back without changing data",
    ))
}
