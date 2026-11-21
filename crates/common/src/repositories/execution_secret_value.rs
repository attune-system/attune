//! Repository for encrypted execution/enforcement secret values.

use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::{Executor, PgConnection, Postgres, Transaction};

use crate::secret_values::{
    restore_secret_values, PreparedSecretValue, StoredSecretValue, ENTITY_EXECUTION_CONFIG,
};
use crate::{models::Id, Error, Result};

use super::{inherited_input::ReceiptContext, FindById};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExecutionSecretValue {
    pub id: Id,
    pub entity_type: String,
    pub entity_id: Id,
    pub json_path: String,
    pub source_kind: String,
    pub source_ref: Option<String>,
    pub encrypted_value: JsonValue,
    pub encryption_key_hash: Option<String>,
    pub created: DateTime<Utc>,
}

pub struct ExecutionSecretValueRepository;

/// Authorization reads recorded origins, never ciphertext or encryption keys.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SecretProvenanceRow {
    pub entity_type: String,
    pub entity_id: Id,
    pub json_path: String,
    pub source_kind: String,
    pub source_ref: Option<String>,
}

impl ExecutionSecretValueRepository {
    /// Restore accepted execution inputs within the transaction that owns
    /// delivery. Check the returned receipts again after commit and before
    /// handing plaintext to a caller.
    pub async fn restore_execution_parameters(
        transaction: &mut Transaction<'_, Postgres>,
        execution_id: Id,
        redacted_config: JsonValue,
        encryption_key: Option<&str>,
    ) -> Result<(JsonValue, Vec<String>, Vec<ReceiptContext>)> {
        let connection = &mut **transaction;
        super::schema_type::SchemaTypeRepository::lock_selection(connection).await?;
        let secrets =
            Self::find_stored_by_entity(&mut *connection, ENTITY_EXECUTION_CONFIG, execution_id)
                .await?;
        let paths = secrets
            .iter()
            .map(|secret| secret.json_path.clone())
            .collect();
        let execution =
            super::execution::ExecutionRepository::find_by_id(&mut *connection, execution_id)
                .await?
                .ok_or_else(|| Error::invalid_state("Execution secret owner is unavailable"))?;
        if secrets.is_empty() {
            crate::key_access::authorize_execution_parameter_key_origins_with_conn(
                connection,
                execution_id,
                execution.executor,
                &secrets,
            )
            .await?;
            return Ok((redacted_config, paths, Vec::new()));
        }

        let encryption_key =
            encryption_key.ok_or_else(|| Error::internal("No encryption key configured"))?;

        let (ordinary, receipts) = super::immutable_input_origin::prepare_input_receipts(
            connection,
            ENTITY_EXECUTION_CONFIG,
            execution_id,
            &secrets,
            encryption_key,
        )
        .await?;
        if !ordinary.is_empty() {
            crate::key_access::authorize_execution_parameter_key_origins_with_conn(
                connection,
                execution_id,
                execution.executor,
                &ordinary,
            )
            .await?;
        }
        super::immutable_input_origin::bind_input_receipt_buffers(
            connection,
            ENTITY_EXECUTION_CONFIG,
            execution_id,
            &secrets,
        )
        .await?;

        let restored =
            restore_secret_values(redacted_config, &secrets, encryption_key).map_err(|e| {
                Error::Internal(format!(
                    "Failed to restore secret parameters for execution {}: {}",
                    execution_id, e
                ))
            })?;
        for receipt in &receipts {
            receipt.ensure_fresh_on(connection).await?;
        }
        Ok((restored, paths, receipts))
    }

    /// Read only the exact source sites in one disclosure frontier. Zipped
    /// selectors must not turn into a cross product of entity kinds and IDs.
    pub async fn find_bounded_provenance_by_sites(
        conn: &mut PgConnection,
        sites: &[(String, Id, String)],
    ) -> Result<Vec<SecretProvenanceRow>> {
        if sites.len() > 256 {
            return Err(crate::Error::PermissionDenied(
                "Provenance frontier exceeds its bound".into(),
            ));
        }
        let kinds: Vec<_> = sites.iter().map(|site| site.0.as_str()).collect();
        let ids: Vec<_> = sites.iter().map(|site| site.1).collect();
        let paths: Vec<_> = sites.iter().map(|site| site.2.as_str()).collect();
        sqlx::query_as(
            "WITH bounded AS ( \
               SELECT p.entity_type, p.entity_id, p.json_path, p.source_kind, p.source_ref \
               FROM execution_secret_value p \
               JOIN UNNEST($1::TEXT[], $2::BIGINT[], $3::TEXT[]) AS wanted(kind, id, path) \
                 ON p.entity_type = wanted.kind AND p.entity_id = wanted.id AND p.json_path = wanted.path \
               ORDER BY p.entity_type, p.entity_id, p.json_path LIMIT 257 \
             ), budgeted AS ( \
               SELECT entity_type, entity_id, json_path, source_kind, source_ref, \
                 SUM(COALESCE(octet_length(source_ref), 0)) \
                 OVER (ORDER BY entity_type, entity_id, json_path) AS origin_bytes FROM bounded \
             ) SELECT entity_type, entity_id, json_path, source_kind, \
               CASE WHEN origin_bytes <= 1048576 THEN source_ref ELSE NULL END AS source_ref \
               FROM budgeted ORDER BY entity_type, entity_id, json_path",
        )
        .bind(kinds)
        .bind(ids)
        .bind(paths)
        .fetch_all(conn)
        .await
        .map_err(Into::into)
    }

    /// Runtime disclosure returns one overflow row and at most 1 MiB of origin
    /// text. Oversized origins become unavailable, never silently authorized.
    /// Ciphertext and encryption metadata are not projected.
    pub async fn find_bounded_provenance<'e, E>(
        executor: E,
        entity_types: &[&str],
        entity_ids: &[Id],
    ) -> Result<Vec<SecretProvenanceRow>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as(
            "WITH bounded AS ( \
               SELECT entity_type, entity_id, json_path, source_kind, source_ref \
               FROM execution_secret_value WHERE entity_type = ANY($1) AND entity_id = ANY($2) \
               ORDER BY entity_type, entity_id, json_path LIMIT 257 \
             ), budgeted AS ( \
               SELECT entity_type, entity_id, json_path, source_kind, source_ref, \
                 SUM(COALESCE(octet_length(source_ref), 0)) OVER (ORDER BY entity_type, entity_id, json_path) AS origin_bytes \
               FROM bounded \
             ) SELECT entity_type, entity_id, json_path, source_kind, \
               CASE WHEN origin_bytes <= 1048576 THEN source_ref ELSE NULL END AS source_ref \
               FROM budgeted ORDER BY entity_type, entity_id, json_path",
        ).bind(entity_types).bind(entity_ids).fetch_all(executor).await.map_err(Into::into)
    }

    pub async fn find_provenance_by_entities<'e, E>(
        executor: E,
        entity_types: &[&str],
        entity_ids: &[Id],
    ) -> Result<Vec<SecretProvenanceRow>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as(
            "SELECT entity_type, entity_id, json_path, source_kind, source_ref \
             FROM execution_secret_value WHERE entity_type = ANY($1) AND entity_id = ANY($2) \
             ORDER BY entity_type, entity_id, json_path",
        )
        .bind(entity_types)
        .bind(entity_ids)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    /// Replace a mutable value's complete encrypted projection in its owning
    /// transaction. Removing a secret field must also remove its old ciphertext.
    pub async fn replace_entity_with_conn(
        connection: &mut PgConnection,
        entity_type: &str,
        entity_id: Id,
        values: &[PreparedSecretValue],
    ) -> Result<()> {
        sqlx::query("DELETE FROM execution_secret_value WHERE entity_type = $1 AND entity_id = $2")
            .bind(entity_type)
            .bind(entity_id)
            .execute(&mut *connection)
            .await?;
        Self::upsert_many_with_conn(connection, entity_type, entity_id, values).await
    }
    pub async fn upsert_many<'e, E>(
        executor: E,
        entity_type: &str,
        entity_id: Id,
        values: &[PreparedSecretValue],
    ) -> Result<()>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        for value in values {
            sqlx::query(
                r#"
                INSERT INTO execution_secret_value
                    (entity_type, entity_id, json_path, source_kind, source_ref,
                     encrypted_value, encryption_key_hash)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (entity_type, entity_id, json_path)
                DO UPDATE SET
                    source_kind = EXCLUDED.source_kind,
                    source_ref = EXCLUDED.source_ref,
                    encrypted_value = EXCLUDED.encrypted_value,
                    encryption_key_hash = EXCLUDED.encryption_key_hash
                "#,
            )
            .bind(entity_type)
            .bind(entity_id)
            .bind(&value.json_path)
            .bind(&value.source_kind)
            .bind(&value.source_ref)
            .bind(&value.encrypted_value)
            .bind(&value.encryption_key_hash)
            .execute(executor)
            .await?;
        }

        Ok(())
    }

    pub async fn upsert_many_with_conn(
        conn: &mut PgConnection,
        entity_type: &str,
        entity_id: Id,
        values: &[PreparedSecretValue],
    ) -> Result<()> {
        for value in values {
            sqlx::query(
                r#"
                INSERT INTO execution_secret_value
                    (entity_type, entity_id, json_path, source_kind, source_ref,
                     encrypted_value, encryption_key_hash)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (entity_type, entity_id, json_path)
                DO UPDATE SET
                    source_kind = EXCLUDED.source_kind,
                    source_ref = EXCLUDED.source_ref,
                    encrypted_value = EXCLUDED.encrypted_value,
                    encryption_key_hash = EXCLUDED.encryption_key_hash
                "#,
            )
            .bind(entity_type)
            .bind(entity_id)
            .bind(&value.json_path)
            .bind(&value.source_kind)
            .bind(&value.source_ref)
            .bind(&value.encrypted_value)
            .bind(&value.encryption_key_hash)
            .execute(&mut *conn)
            .await?;
        }

        Ok(())
    }

    pub async fn find_by_entity<'e, E>(
        executor: E,
        entity_type: &str,
        entity_id: Id,
    ) -> Result<Vec<ExecutionSecretValue>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, ExecutionSecretValue>(
            r#"
            SELECT id, entity_type, entity_id, json_path, source_kind, source_ref,
                   encrypted_value, encryption_key_hash, created
            FROM execution_secret_value
            WHERE entity_type = $1 AND entity_id = $2
            ORDER BY json_path ASC
            "#,
        )
        .bind(entity_type)
        .bind(entity_id)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn find_stored_by_entity<'e, E>(
        executor: E,
        entity_type: &str,
        entity_id: Id,
    ) -> Result<Vec<StoredSecretValue>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let rows = Self::find_by_entity(executor, entity_type, entity_id).await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredSecretValue {
                json_path: row.json_path,
                encrypted_value: row.encrypted_value,
                encryption_key_hash: row.encryption_key_hash,
                source_kind: row.source_kind,
                source_ref: row.source_ref,
            })
            .collect())
    }

    /// The queue completion consumer needs only the control projection. Keep
    /// unrelated structured outputs and output channels out of this query.
    pub(super) async fn find_queue_ack_projection(
        conn: &mut PgConnection,
        execution: Id,
    ) -> Result<Vec<StoredSecretValue>> {
        let rows: Vec<ExecutionSecretValue> = sqlx::query_as(
            "SELECT id, entity_type, entity_id, json_path, source_kind, source_ref, \
                    encrypted_value, encryption_key_hash, created \
             FROM execution_secret_value WHERE entity_type=$1 AND entity_id=$2 \
                AND (json_path IN ('','/queue_ack') OR json_path LIKE '/queue_ack/%') \
             ORDER BY json_path LIMIT 257 FOR SHARE",
        )
        .bind(crate::secret_values::ENTITY_EXECUTION_RESULT)
        .bind(execution)
        .fetch_all(conn)
        .await?;
        if rows.len() > 256 {
            return Err(crate::Error::PermissionDenied(
                "Queue acknowledgement projection exceeds its bound".into(),
            ));
        }
        Ok(rows
            .into_iter()
            .map(|row| StoredSecretValue {
                json_path: row.json_path,
                encrypted_value: row.encrypted_value,
                encryption_key_hash: row.encryption_key_hash,
                source_kind: row.source_kind,
                source_ref: row.source_ref,
            })
            .collect())
    }

    /// Bind a prepared input buffer to the actual typed source row through
    /// restoration. Call during source preparation, before destination locks.
    pub async fn find_stored_at(
        connection: &mut sqlx::PgConnection,
        entity_type: &str,
        entity_id: Id,
        path: &str,
    ) -> Result<Option<StoredSecretValue>> {
        let row = sqlx::query_as::<_, ExecutionSecretValue>(
            "SELECT id, entity_type, entity_id, json_path, source_kind, source_ref, encrypted_value, encryption_key_hash, created FROM execution_secret_value WHERE entity_type=$1 AND entity_id=$2 AND json_path=$3 FOR SHARE",
        )
        .bind(entity_type)
        .bind(entity_id)
        .bind(path)
        .fetch_optional(connection)
        .await?;
        Ok(row.map(|row| StoredSecretValue {
            json_path: row.json_path,
            encrypted_value: row.encrypted_value,
            encryption_key_hash: row.encryption_key_hash,
            source_kind: row.source_kind,
            source_ref: row.source_ref,
        }))
    }

    pub async fn copy_entity<'e, E>(
        executor: E,
        from_entity_type: &str,
        from_entity_id: Id,
        to_entity_type: &str,
        to_entity_id: Id,
    ) -> Result<()>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query(
            r#"
            INSERT INTO execution_secret_value
                (entity_type, entity_id, json_path, source_kind, source_ref,
                 encrypted_value, encryption_key_hash)
            SELECT $3, $4, json_path, source_kind, source_ref,
                   encrypted_value, encryption_key_hash
            FROM execution_secret_value
            WHERE entity_type = $1 AND entity_id = $2
            ON CONFLICT (entity_type, entity_id, json_path)
            DO UPDATE SET
                source_kind = EXCLUDED.source_kind,
                source_ref = EXCLUDED.source_ref,
                encrypted_value = EXCLUDED.encrypted_value,
                encryption_key_hash = EXCLUDED.encryption_key_hash
            "#,
        )
        .bind(from_entity_type)
        .bind(from_entity_id)
        .bind(to_entity_type)
        .bind(to_entity_id)
        .execute(executor)
        .await?;

        Ok(())
    }

    pub async fn copy_entity_path<'e, E>(
        executor: E,
        from_entity_type: &str,
        from_entity_id: Id,
        from_json_path: &str,
        to_entity_type: &str,
        to_entity_id: Id,
        to_json_path: &str,
    ) -> Result<()>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query(
            r#"
            INSERT INTO execution_secret_value
                (entity_type, entity_id, json_path, source_kind, source_ref,
                 encrypted_value, encryption_key_hash)
            SELECT $4, $5, $6, source_kind, source_ref,
                   encrypted_value, encryption_key_hash
            FROM execution_secret_value
            WHERE entity_type = $1 AND entity_id = $2 AND json_path = $3
            ON CONFLICT (entity_type, entity_id, json_path)
            DO UPDATE SET
                source_kind = EXCLUDED.source_kind,
                source_ref = EXCLUDED.source_ref,
                encrypted_value = EXCLUDED.encrypted_value,
                encryption_key_hash = EXCLUDED.encryption_key_hash
            "#,
        )
        .bind(from_entity_type)
        .bind(from_entity_id)
        .bind(from_json_path)
        .bind(to_entity_type)
        .bind(to_entity_id)
        .bind(to_json_path)
        .execute(executor)
        .await?;

        Ok(())
    }

    pub async fn delete_by_entity<'e, E>(
        executor: E,
        entity_type: &str,
        entity_id: Id,
    ) -> Result<u64>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query(
            "DELETE FROM execution_secret_value WHERE entity_type = $1 AND entity_id = $2",
        )
        .bind(entity_type)
        .bind(entity_id)
        .execute(executor)
        .await?;

        Ok(result.rows_affected())
    }
}
