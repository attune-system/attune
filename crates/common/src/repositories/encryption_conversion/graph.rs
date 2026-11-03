//! Conversion plans contain typed metadata changes, never arbitrary JSON walks.
use super::*;
use crate::models::Key;
use crate::repositories::{accepted_input, event_origin, native_input_snapshot};
use serde_json::json;

pub(super) struct Change {
    pub table: &'static str,
    pub identity: Value,
    pub before: String,
    pub values: Value,
}

pub(super) const STORES: &[(&str, &str)] = &[
    ("key", "id"),
    ("execution_secret_value", "id"),
    ("runtime_input_receipt", "id"),
    ("runtime_accepted_input", "id"),
    ("runtime_native_input_snapshot", "id"),
    ("event_origin_snapshot", "id"),
    ("runtime_native_success_metadata_observation", "snapshot"),
    (
        "runtime_native_iteration_outcome_member",
        "snapshot,ordinal",
    ),
];

pub(super) async fn changes(
    conn: &mut PgConnection,
    master: &str,
) -> Result<(Vec<Change>, ConversionReport)> {
    let conversion = VerifierConversion::new(master)?;
    let mut changes = Vec::new();
    let mut report = ConversionReport::default();
    let mut total_bytes = 0usize;
    for &(table, order) in STORES {
        let size_sql = format!(
            "SELECT COALESCE(sum(octet_length(to_jsonb(t)::TEXT)),0)::BIGINT FROM {table} t"
        );
        let bytes: i64 = sqlx::query_scalar(&size_sql).fetch_one(&mut *conn).await?;
        if bytes < 0 || total_bytes as u64 + bytes as u64 > 256 * 1024 * 1024 {
            return Err(Error::validation(
                "Encryption conversion exceeds the 256-MiB metadata planning bound",
            ));
        }
        // Names are a closed source-owned list; row values are always bound.
        let sql = format!("SELECT to_jsonb(t),encryption_conversion_row_digest(to_jsonb(t)) FROM {table} t ORDER BY {order}");
        let rows: Vec<(Value, String)> = sqlx::query_as(&sql).fetch_all(&mut *conn).await?;
        for (row, before) in rows {
            total_bytes = total_bytes
                .checked_add(serde_json::to_vec(&row)?.len())
                .ok_or_else(corrupt)?;
            if total_bytes > 256 * 1024 * 1024 {
                return Err(Error::validation(
                    "Encryption conversion exceeds the 256-MiB metadata planning bound",
                ));
            }
            let id = |column: &str| -> Result<i64> {
                row.get(column).and_then(Value::as_i64).ok_or_else(corrupt)
            };
            let identity = if table == "runtime_native_iteration_outcome_member" {
                json!({"snapshot":id("snapshot")?,"ordinal":id("ordinal")?})
            } else if table == "runtime_native_success_metadata_observation" {
                json!({"snapshot":id("snapshot")?})
            } else {
                json!({"id":id("id")?})
            };
            let values;
            match table {
                "key" => {
                    let key: Key = serde_json::from_value(row.clone())?;
                    if !key.encrypted {
                        continue;
                    }
                    let current = crypto::keystore::fingerprint(master, key.id)?;
                    if key.encryption_key_hash.as_deref() == Some(current.as_str()) {
                        crypto::keystore::decrypt_json(&key.value, master, key.id)?;
                        report.keys_already_current += 1;
                        continue;
                    }
                    require_known_verifier(
                        key.encryption_key_hash.as_deref(),
                        &conversion.legacy,
                        &conversion.current,
                    )?;
                    let plaintext = crypto::decrypt_json(&key.value, master)?;
                    values = json!({"value":crypto::keystore::encrypt_json(&plaintext, master, key.id)?,"encryption_key_hash":current});
                    report.keys_converted += 1;
                }
                "execution_secret_value" | "runtime_input_receipt" => {
                    let encrypted = row.get("encrypted_value").ok_or_else(corrupt)?;
                    let original: Option<String> = serde_json::from_value(
                        row.get("encryption_key_hash")
                            .cloned()
                            .ok_or_else(corrupt)?,
                    )?;
                    let mut fingerprint = original.clone();
                    conversion.field(encrypted, &mut fingerprint)?;
                    if original == fingerprint {
                        report.fields_already_current += 1;
                        continue;
                    }
                    values = json!({"encryption_key_hash":fingerprint});
                    report.field_verifiers_converted += 1;
                }
                "runtime_accepted_input" | "runtime_native_input_snapshot" => {
                    let body = row.get("body").ok_or_else(corrupt)?;
                    if row.get("body_digest").and_then(Value::as_str)
                        != Some(digest(body)?.as_str())
                    {
                        return Err(corrupt());
                    }
                    let converted = if table == "runtime_accepted_input" {
                        accepted_input::convert_encryption_metadata(body, &conversion)?
                    } else {
                        native_input_snapshot::convert_encryption_metadata(body, &conversion)?
                    };
                    if let Some(encrypted) = row.get("encrypted_value") {
                        crypto::decrypt_json(encrypted, master)?;
                    }
                    let mut fingerprint: String = serde_json::from_value(
                        row.get("encryption_key_hash")
                            .cloned()
                            .ok_or_else(corrupt)?,
                    )?;
                    conversion.verifier(&mut fingerprint)?;
                    values = json!({"body_digest":digest(&converted)?,"body":converted,"encryption_key_hash":fingerprint});
                }
                "event_origin_snapshot" => {
                    let body = row.get("snapshot").ok_or_else(corrupt)?;
                    if row.get("snapshot_digest").and_then(Value::as_str)
                        != Some(digest(body)?.as_str())
                    {
                        return Err(corrupt());
                    }
                    let converted = event_origin::convert_encryption_metadata(body, &conversion)?;
                    values = json!({"snapshot_digest":digest(&converted)?,"snapshot":converted});
                }
                // These normalized copies are replaced from the converted owner,
                // not interpreted using a second independent metadata schema.
                "runtime_native_success_metadata_observation"
                | "runtime_native_iteration_outcome_member" => {
                    let snapshot = id("snapshot")?;
                    let owner: Value = sqlx::query_scalar(
                        "SELECT body FROM runtime_native_input_snapshot WHERE id=$1",
                    )
                    .bind(snapshot)
                    .fetch_one(&mut *conn)
                    .await?;
                    let converted =
                        native_input_snapshot::convert_encryption_metadata(&owner, &conversion)?;
                    if table == "runtime_native_success_metadata_observation" {
                        let mut source = owner.get("source").cloned().ok_or_else(corrupt)?;
                        source.as_object_mut().ok_or_else(corrupt)?.remove("kind");
                        if row.get("observation") != Some(&source) {
                            return Err(corrupt());
                        }
                        let mut source = converted.get("source").cloned().ok_or_else(corrupt)?;
                        source.as_object_mut().ok_or_else(corrupt)?.remove("kind");
                        values = json!({"observation":source});
                    } else {
                        let ordinal: usize = id("ordinal")?.try_into().map_err(|_| corrupt())?;
                        if owner
                            .pointer("/source/members")
                            .and_then(Value::as_array)
                            .and_then(|members| members.get(ordinal))
                            != row.get("member")
                        {
                            return Err(corrupt());
                        }
                        values = json!({"member":converted.pointer("/source/members").and_then(Value::as_array).and_then(|members| members.get(ordinal)).cloned().ok_or_else(corrupt)?});
                    }
                }
                _ => return Err(corrupt()),
            }
            if values
                .as_object()
                .ok_or_else(corrupt)?
                .iter()
                .all(|(name, value)| row.get(name) == Some(value))
            {
                continue;
            }
            if table != "key"
                && table != "execution_secret_value"
                && table != "runtime_input_receipt"
            {
                report.graphs_converted += 1;
            }
            changes.push(Change {
                table,
                identity,
                before,
                values,
            });
        }
    }
    Ok((changes, report))
}

pub(super) fn corrupt() -> Error {
    Error::validation(
        "Corrupt or unsupported retained encryption metadata; no conversion was published",
    )
}

fn digest(value: &impl Serialize) -> Result<String> {
    crate::value_schema::digest(value).map_err(|_| corrupt())
}
