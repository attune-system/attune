//! Secret Management Module
//!
//! Handles fetching, decrypting, and injecting secrets into execution environments.
//! Secrets are stored encrypted in the database and decrypted on-demand for execution.
//!
//! Key values are stored as JSONB — they can be plain strings, objects, arrays,
//! numbers, or booleans. When encrypted, the JSON value is serialised to a
//! compact string, encrypted, and stored as a JSON string. Decryption reverses
//! this process, recovering the original structured value.
//!
//! Encryption and decryption use the shared `attune_common::crypto` module
//! (`encrypt_json` / `decrypt_json`) which stores ciphertext in the format
//! `BASE64(nonce ++ ciphertext)`. This is the same format used by the API
//! service, so keys encrypted by the API can be decrypted by the worker and
//! vice versa.

use attune_common::error::{Error, Result};
use attune_common::models::{key::Key, Action, OwnerType};
use attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository;
use attune_common::repositories::key::KeyRepository;
use attune_common::secret_values::{restore_secret_values, ENTITY_EXECUTION_CONFIG};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use std::collections::HashMap;
use tracing::{debug, warn};

/// Secret manager for handling secret operations.
///
/// Holds the database connection pool and the raw encryption key string.
/// The encryption key is passed through to `attune_common::crypto` which
/// derives the AES-256 key internally via SHA-256.
pub struct SecretManager {
    pool: PgPool,
    encryption_key: Option<String>,
}

impl SecretManager {
    /// Create a new secret manager.
    ///
    /// `encryption_key` is the raw key string (≥ 32 characters) used for
    /// AES-256-GCM encryption/decryption via `attune_common::crypto`.
    pub fn new(pool: PgPool, encryption_key: Option<String>) -> Result<Self> {
        if encryption_key.is_none() {
            warn!("No encryption key configured - encrypted secrets will fail to decrypt");
        }

        Ok(Self {
            pool,
            encryption_key,
        })
    }

    /// Fetch all secrets relevant to an action execution.
    ///
    /// Secrets are fetched in order of precedence:
    /// 1. System-level secrets (owner_type='system')
    /// 2. Pack-level secrets (owner_type='pack')
    /// 3. Action-level secrets (owner_type='action')
    ///
    /// More specific secrets override less specific ones with the same name.
    /// Values are returned as [`JsonValue`] — they may be strings, objects,
    /// arrays, numbers, or booleans.
    pub async fn fetch_secrets_for_action(
        &self,
        action: &Action,
    ) -> Result<HashMap<String, JsonValue>> {
        debug!("Fetching secrets for action: {}", action.r#ref);

        let mut secrets = HashMap::new();

        // 1. Fetch system-level secrets
        let system_secrets = self.fetch_secrets_by_owner_type(OwnerType::System).await?;
        for secret in system_secrets {
            let value = self.decrypt_if_needed(&secret)?;
            secrets.insert(secret.name.clone(), value);
        }
        debug!("Loaded {} system secrets", secrets.len());

        // 2. Fetch pack-level secrets
        let pack_secrets = self.fetch_secrets_by_pack(action.pack).await?;
        for secret in pack_secrets {
            let value = self.decrypt_if_needed(&secret)?;
            secrets.insert(secret.name.clone(), value);
        }
        debug!("Loaded {} pack secrets", secrets.len());

        // 3. Fetch action-level secrets
        let action_secrets = self.fetch_secrets_by_action(action.id).await?;
        for secret in action_secrets {
            let value = self.decrypt_if_needed(&secret)?;
            secrets.insert(secret.name.clone(), value);
        }
        debug!("Total secrets loaded: {}", secrets.len());

        Ok(secrets)
    }

    pub async fn restore_execution_parameters(
        &self,
        execution_id: i64,
        redacted_config: JsonValue,
    ) -> Result<JsonValue> {
        let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
            &self.pool,
            ENTITY_EXECUTION_CONFIG,
            execution_id,
        )
        .await?;
        if secrets.is_empty() {
            return Ok(redacted_config);
        }

        let encryption_key = self
            .encryption_key
            .as_ref()
            .ok_or_else(|| Error::Internal("No encryption key configured".to_string()))?;

        restore_secret_values(redacted_config, &secrets, encryption_key).map_err(|e| {
            Error::Internal(format!(
                "Failed to restore secret parameters for execution {}: {}",
                execution_id, e
            ))
        })
    }

    /// Fetch secrets by owner type
    async fn fetch_secrets_by_owner_type(&self, owner_type: OwnerType) -> Result<Vec<Key>> {
        KeyRepository::find_by_owner_type(&self.pool, owner_type).await
    }

    /// Fetch secrets for a specific pack
    async fn fetch_secrets_by_pack(&self, pack_id: i64) -> Result<Vec<Key>> {
        sqlx::query_as::<_, Key>(
            "SELECT id, ref, local_ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref,
             owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted,
             encryption_key_hash, value, created, updated
             FROM key
             WHERE owner_type = $1 AND owner_pack = $2
             ORDER BY name ASC",
        )
        .bind(OwnerType::Pack)
        .bind(pack_id)
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// Fetch secrets for a specific action
    async fn fetch_secrets_by_action(&self, action_id: i64) -> Result<Vec<Key>> {
        sqlx::query_as::<_, Key>(
            "SELECT id, ref, local_ref, owner_type, owner, owner_identity, owner_pack, owner_pack_ref,
             owner_action, owner_action_ref, owner_sensor, owner_sensor_ref, name, encrypted,
             encryption_key_hash, value, created, updated
             FROM key
             WHERE owner_type = $1 AND owner_action = $2
             ORDER BY name ASC",
        )
        .bind(OwnerType::Action)
        .bind(action_id)
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// Decrypt a secret if it's encrypted, otherwise return the value as-is.
    ///
    /// For unencrypted keys the JSONB value is returned directly.
    /// For encrypted keys the value (a JSON string containing base64 ciphertext)
    /// is decrypted via `attune_common::crypto::decrypt_json` and parsed back
    /// into the original [`JsonValue`].
    fn decrypt_if_needed(&self, key: &Key) -> Result<JsonValue> {
        if !key.encrypted {
            return Ok(key.value.clone());
        }

        let encryption_key = self
            .encryption_key
            .as_ref()
            .ok_or_else(|| Error::Internal("No encryption key configured".to_string()))?;

        // Verify encryption key hash if present
        if let Some(expected_hash) = &key.encryption_key_hash {
            let actual_hash = attune_common::crypto::hash_encryption_key(encryption_key);
            if &actual_hash != expected_hash {
                return Err(Error::Internal(format!(
                    "Encryption key hash mismatch for secret '{}'",
                    key.name
                )));
            }
        }

        attune_common::crypto::decrypt_json(&key.value, encryption_key)
            .map_err(|e| Error::Internal(format!("Failed to decrypt key '{}': {}", key.name, e)))
    }

    /// Compute hash of the encryption key.
    ///
    /// Uses the shared `attune_common::crypto::hash_encryption_key` so the
    /// hash format is consistent with values stored by the API.
    pub fn compute_key_hash(&self) -> String {
        if let Some(key) = &self.encryption_key {
            attune_common::crypto::hash_encryption_key(key)
        } else {
            String::new()
        }
    }

    pub fn encryption_key(&self) -> Option<&str> {
        self.encryption_key.as_deref()
    }

    /// Prepare secrets as environment variables.
    ///
    /// **DEPRECATED - SECURITY VULNERABILITY**: This method exposes secrets in the process
    /// environment, making them visible in process listings (`ps auxe`) and `/proc/[pid]/environ`.
    ///
    /// Secrets should be passed via stdin instead. This method is kept only for backward
    /// compatibility and will be removed in a future version.
    ///
    /// Secret names are converted to uppercase and prefixed with "SECRET_"
    /// Example: "api_key" becomes "SECRET_API_KEY"
    ///
    /// String values are used directly; structured values are serialised to
    /// compact JSON.
    #[deprecated(
        since = "0.2.0",
        note = "Secrets in environment variables are insecure. Pass secrets via stdin instead."
    )]
    pub fn prepare_secret_env(
        &self,
        secrets: &HashMap<String, JsonValue>,
    ) -> HashMap<String, String> {
        secrets
            .iter()
            .map(|(name, value)| {
                let env_name = format!("SECRET_{}", name.to_uppercase().replace('-', "_"));
                let env_value = match value {
                    JsonValue::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (env_name, env_value)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::crypto;

    // ── encrypt / decrypt round-trip using shared crypto ───────────

    const TEST_KEY: &str = "this_is_a_test_key_that_is_32_chars_long!!!!";

    #[test]
    fn test_encrypt_decrypt_roundtrip_string() {
        let value = serde_json::json!("my-secret-value");
        let encrypted = crypto::encrypt_json(&value, TEST_KEY).unwrap();
        let decrypted = crypto::decrypt_json(&encrypted, TEST_KEY).unwrap();
        assert_eq!(value, decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip_object() {
        let value = serde_json::json!({"user": "admin", "password": "s3cret"});
        let encrypted = crypto::encrypt_json(&value, TEST_KEY).unwrap();
        let decrypted = crypto::decrypt_json(&encrypted, TEST_KEY).unwrap();
        assert_eq!(value, decrypted);
    }

    #[test]
    fn test_encrypt_produces_different_ciphertext() {
        let value = serde_json::json!("my-secret-value");
        let encrypted1 = crypto::encrypt_json(&value, TEST_KEY).unwrap();
        let encrypted2 = crypto::encrypt_json(&value, TEST_KEY).unwrap();

        // Different ciphertexts due to random nonces
        assert_ne!(encrypted1, encrypted2);

        // Both decrypt to the same value
        assert_eq!(crypto::decrypt_json(&encrypted1, TEST_KEY).unwrap(), value);
        assert_eq!(crypto::decrypt_json(&encrypted2, TEST_KEY).unwrap(), value);
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let value = serde_json::json!("secret");
        let encrypted = crypto::encrypt_json(&value, TEST_KEY).unwrap();

        let wrong_key = "wrong_key_that_is_also_32_chars_long!!!";
        assert!(crypto::decrypt_json(&encrypted, wrong_key).is_err());
    }

    // ── prepare_secret_env ────────────────────────────────────────

    #[test]
    fn test_prepare_secret_env() {
        let mut secrets: HashMap<String, JsonValue> = HashMap::new();
        secrets.insert(
            "api_key".to_string(),
            JsonValue::String("secret123".to_string()),
        );
        secrets.insert(
            "db-password".to_string(),
            JsonValue::String("pass456".to_string()),
        );
        secrets.insert(
            "oauth_token".to_string(),
            JsonValue::String("token789".to_string()),
        );

        // Replicate the logic without constructing a full SecretManager
        let env: HashMap<String, String> = secrets
            .iter()
            .map(|(name, value)| {
                let env_name = format!("SECRET_{}", name.to_uppercase().replace('-', "_"));
                let env_value = match value {
                    JsonValue::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (env_name, env_value)
            })
            .collect();

        assert_eq!(env.get("SECRET_API_KEY"), Some(&"secret123".to_string()));
        assert_eq!(env.get("SECRET_DB_PASSWORD"), Some(&"pass456".to_string()));
        assert_eq!(env.get("SECRET_OAUTH_TOKEN"), Some(&"token789".to_string()));
        assert_eq!(env.len(), 3);
    }

    #[test]
    fn test_prepare_secret_env_structured_value() {
        let mut secrets: HashMap<String, JsonValue> = HashMap::new();
        secrets.insert(
            "db_config".to_string(),
            serde_json::json!({"host": "db.example.com", "port": 5432}),
        );

        let env: HashMap<String, String> = secrets
            .iter()
            .map(|(name, value)| {
                let env_name = format!("SECRET_{}", name.to_uppercase().replace('-', "_"));
                let env_value = match value {
                    JsonValue::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (env_name, env_value)
            })
            .collect();

        // Structured values should be serialised to compact JSON
        let db_config = env.get("SECRET_DB_CONFIG").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(db_config).unwrap();
        assert_eq!(parsed["host"], "db.example.com");
        assert_eq!(parsed["port"], 5432);
    }

    // ── compute_key_hash ──────────────────────────────────────────

    #[test]
    fn test_compute_key_hash_consistent() {
        let hash1 = crypto::hash_encryption_key(TEST_KEY);
        let hash2 = crypto::hash_encryption_key(TEST_KEY);
        assert_eq!(hash1, hash2);
        // SHA-256 → 64 hex characters
        assert_eq!(hash1.len(), 64);
    }

    #[test]
    fn test_compute_key_hash_different_keys() {
        let hash1 = crypto::hash_encryption_key(TEST_KEY);
        let hash2 = crypto::hash_encryption_key("different_key_that_is_32_chars_long!!");
        assert_ne!(hash1, hash2);
    }
}
