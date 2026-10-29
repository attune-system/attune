//! Synthetic legacy seeding through the operator's exact-row maintenance gate.
//! This reverse fixture is TEST CODE ONLY. The shipped converter never writes
//! legacy verifiers. Only named metadata fields are visited, never user payloads.
use attune_common::{crypto, repositories::encryption_conversion::convert_offline};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

fn digest(value: &Value) -> String {
    Sha256::digest(serde_jcs::to_vec(value).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn fingerprint(value: &mut Value, current: &str, legacy: &str) {
    if value.is_null() {
        return;
    }
    assert!(
        value.as_str() == Some(current),
        "Fixture must start with the current verifier"
    );
    *value = json!(legacy);
}
fn fields(value: &mut Value, current: &str, legacy: &str) {
    for field in value.as_array_mut().expect("typed archived fields") {
        fingerprint(
            field
                .get_mut("encryption_key_hash")
                .expect("typed verifier"),
            current,
            legacy,
        );
    }
}
fn result(source: &mut Value, current: &str, legacy: &str) {
    fields(&mut source["fields"], current, legacy);
    source["version"]["ciphertext_digest"] = json!(digest(&source["fields"]));
}
fn body(value: &mut Value, current: &str, legacy: &str) {
    let kind = value["source"]["kind"].as_str().unwrap().to_string();
    source(&mut value["source"], &kind, current, legacy);
}
fn source(value: &mut Value, kind: &str, current: &str, legacy: &str) {
    match kind {
        "cache" | "empty_iteration" => {}
        "wait" => {
            fields(&mut value["fields"], current, legacy);
            value["source_version"]["ciphertext_digest"] = json!(digest(&value["fields"]));
        }
        "result_view" => result(value, current, legacy),
        "completion" => {
            fields(&mut value["fields"], current, legacy);
            for child in value["children"].as_array_mut().unwrap() {
                result(child, current, legacy);
            }
        }
        "wait_completion" => {
            body(&mut value["original"], current, legacy);
            value["original_digest"] = json!(digest(&value["original"]));
            for name in ["config_fields", "result_fields", "variable_fields"] {
                fields(&mut value[name], current, legacy);
            }
        }
        "iteration_outcome" => {
            for item in value["members"].as_array_mut().unwrap() {
                member(item, current, legacy);
            }
            value["members_digest"] = json!(digest(&value["members"]));
        }
        "success_metadata" => {
            for name in ["config_fields", "result_fields", "variable_fields"] {
                fields(&mut value[name], current, legacy);
            }
            for pair in value["child_input_fields"].as_array_mut().unwrap() {
                fields(&mut pair[1], current, legacy);
            }
            for child in value["children"].as_array_mut().unwrap() {
                result(child, current, legacy);
            }
            for witness in value["empty_sources"].as_array_mut().unwrap() {
                body(&mut witness["original"], current, legacy);
                witness["digest"] = json!(digest(&witness["original"]));
            }
            for leaf in value["leaves"].as_array_mut().unwrap() {
                fingerprint(&mut leaf["key_hash"], current, legacy);
            }
        }
        _ => panic!("Unsupported fixture source kind"),
    }
}
fn member(value: &mut Value, current: &str, legacy: &str) {
    for name in ["original", "latest"] {
        fields(&mut value[name]["input_fields"], current, legacy);
    }
}

pub async fn operator(pool: &PgPool) {
    sqlx::raw_sql("DO $$ BEGIN BEGIN IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='attune_encryption_maintenance') THEN CREATE ROLE attune_encryption_maintenance NOLOGIN; END IF; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END; BEGIN GRANT attune_encryption_maintenance TO CURRENT_USER; EXCEPTION WHEN unique_violation THEN NULL; END; END $$")
        .execute(pool).await.unwrap();
}

async fn state(pool: &PgPool) -> String {
    let mut hash = Sha256::new();
    for table in [
        "key",
        "execution_secret_value",
        "runtime_input_receipt",
        "runtime_accepted_input",
        "runtime_native_input_snapshot",
        "event_origin_snapshot",
        "runtime_native_success_metadata_observation",
        "runtime_native_iteration_outcome_member",
        "file_evidence_owner",
        "file_evidence_pin",
        "artifact_file_origin",
        "artifact_file_body",
    ] {
        let rows: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT encryption_conversion_row_digest(to_jsonb(t)) FROM {table} t ORDER BY 1"
        ))
        .fetch_all(pool)
        .await
        .unwrap();
        hash.update(table.as_bytes());
        for row in rows {
            hash.update(row.as_bytes());
        }
    }
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub async fn legacy_upgrade_roundtrip(pool: &PgPool, master: &str) {
    operator(pool).await;
    let current = crypto::hash_encryption_key(master);
    let legacy: String = Sha256::digest(master.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let initial = convert_offline(pool, master, false).await.unwrap();
    assert_eq!(initial.graphs_converted, 0);
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT encryption_conversion_lock()")
        .execute(&mut *tx)
        .await
        .unwrap();
    let run: i64 = sqlx::query_scalar("SELECT encryption_conversion_begin($1)")
        .bind(uuid::Uuid::new_v4())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    for (table, order) in [
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
    ] {
        let rows:Vec<(Value,String)>=sqlx::query_as(&format!("SELECT to_jsonb(t),encryption_conversion_row_digest(to_jsonb(t)) FROM {table} t ORDER BY {order}"))
            .fetch_all(&mut *tx).await.unwrap();
        for (row, before) in rows {
            let identity = if table == "runtime_native_iteration_outcome_member" {
                json!({"snapshot":row["snapshot"],"ordinal":row["ordinal"]})
            } else if table == "runtime_native_success_metadata_observation" {
                json!({"snapshot":row["snapshot"]})
            } else {
                json!({"id":row["id"]})
            };
            let values = match table {
                "key" => {
                    if row["encrypted"] != true {
                        continue;
                    }
                    let id = row["id"].as_i64().unwrap();
                    let plain = crypto::keystore::decrypt_json(&row["value"], master, id).unwrap();
                    json!({"value":crypto::encrypt_json(&plain,master).unwrap(),"encryption_key_hash":legacy})
                }
                "execution_secret_value" | "runtime_input_receipt" => {
                    let mut fp = row["encryption_key_hash"].clone();
                    fingerprint(&mut fp, &current, &legacy);
                    json!({"encryption_key_hash":fp})
                }
                "runtime_accepted_input" | "runtime_native_input_snapshot" => {
                    let mut value = row["body"].clone();
                    if table == "runtime_accepted_input" {
                        fields(&mut value["fields"], &current, &legacy);
                    } else {
                        body(&mut value, &current, &legacy);
                    }
                    json!({"body_digest":digest(&value),"body":value,"encryption_key_hash":legacy})
                }
                "event_origin_snapshot" => {
                    let mut value = row["snapshot"].clone();
                    fields(&mut value["payload"], &current, &legacy);
                    fields(&mut value["config"], &current, &legacy);
                    json!({"snapshot_digest":digest(&value),"snapshot":value})
                }
                "runtime_native_success_metadata_observation" => {
                    let mut value = row["observation"].clone();
                    source(&mut value, "success_metadata", &current, &legacy);
                    json!({"observation":value})
                }
                "runtime_native_iteration_outcome_member" => {
                    let mut value = row["member"].clone();
                    member(&mut value, &current, &legacy);
                    json!({"member":value})
                }
                _ => unreachable!(),
            };
            if values
                .as_object()
                .unwrap()
                .iter()
                .all(|(key, value)| row.get(key) == Some(value))
            {
                continue;
            }
            sqlx::query("SELECT encryption_conversion_apply($1,$2,$3,$4,$5)")
                .bind(run)
                .bind(table)
                .bind(identity)
                .bind(before)
                .bind(values)
                .execute(&mut *tx)
                .await
                .unwrap();
        }
    }
    sqlx::query("SELECT encryption_conversion_finish($1,$2)")
        .bind(run)
        .bind(json!({"applied":true}))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let source: Option<(String,i64)> = sqlx::query_as("SELECT entity_type,entity_id FROM execution_secret_value WHERE encryption_key_hash=$1 ORDER BY id LIMIT 1")
        .bind(&legacy).fetch_optional(pool).await.unwrap();
    let (kind, id) = source.expect("Legacy fixture must contain a recorded encrypted field");
    let fields = attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(pool,&kind,id).await.unwrap();
    assert!(
        attune_common::secret_values::restore_secret_values(Value::Null, &fields, master).is_err(),
        "Current runtime must reject the seeded legacy verifier before conversion"
    );
    let before = state(pool).await;
    let plan = convert_offline(pool, master, false).await.unwrap();
    assert!(
        plan.graphs_converted > 0,
        "Fixture must contain a retained graph needing conversion"
    );
    assert_eq!(state(pool).await, before, "Dry-run changed stored data");
    let report = convert_offline(pool, master, true).await.unwrap();
    assert!(report.graphs_converted > 0);
    let after = state(pool).await;
    let rerun = convert_offline(pool, master, true).await.unwrap();
    assert_eq!(rerun.graphs_converted, 0);
    assert_eq!(rerun.keys_converted, 0);
    assert_eq!(rerun.field_verifiers_converted, 0);
    assert_eq!(
        state(pool).await,
        after,
        "Rerun rewrote already-current data"
    );
}
