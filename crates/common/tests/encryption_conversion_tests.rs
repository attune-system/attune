//! Synthetic populated-store conversion. Every database is owned by this test.
mod helpers;
use attune_common::{
    crypto,
    models::Key,
    repositories::{
        encryption_conversion::convert_offline,
        execution_secret_value::ExecutionSecretValueRepository, key::KeyRepository, FindById,
    },
    secret_values::{
        prepare_secret_values, redaction_marker, restore_secret_values, SecretValueInput,
        ENTITY_EXECUTION_CONFIG,
    },
};
use helpers::KeyFixture;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

const MASTER: &str = "synthetic-offline-conversion-master-32-bytes";

async fn conversion_database() -> attune_common::test_database::TestDatabase {
    helpers::init_test_env();
    let mut config = attune_common::config::Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))
    .unwrap();
    config.database.max_connections = 1;
    config.database.min_connections = 1;
    let db = attune_common::test_database::TestDatabase::create(&config.database)
        .await
        .unwrap()
        .with_cleanup_on_drop();
    // This suite runs only on a disposable test engine. The reserved capability
    // is a cluster prerequisite, never a grant to an application/runtime role.
    sqlx::raw_sql("DO $$ BEGIN BEGIN IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='attune_encryption_maintenance') THEN CREATE ROLE attune_encryption_maintenance NOLOGIN; END IF; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END; BEGIN GRANT attune_encryption_maintenance TO CURRENT_USER; EXCEPTION WHEN unique_violation THEN NULL; END; END $$")
        .execute(&*db).await.unwrap();
    db
}
fn old_verifier() -> String {
    Sha256::digest(MASTER.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
async fn legacy_key(db: &PgPool) -> Key {
    let encrypted = crypto::encrypt_json(&json!({"credential":"synthetic-only"}), MASTER).unwrap();
    KeyFixture::new_system_unique("legacy", encrypted.as_str().unwrap())
        .with_encrypted(true)
        .with_encryption_key_hash(&old_verifier())
        .create(db)
        .await
        .unwrap()
}
async fn legacy_field(db: &PgPool, id: i64) {
    let mut fields = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/credential".into(),
            value: json!("synthetic-recorded-only"),
            source_kind: "literal".into(),
            source_ref: None,
        }],
        MASTER,
    )
    .unwrap();
    fields[0].encryption_key_hash = old_verifier();
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut db.acquire().await.unwrap(),
        ENTITY_EXECUTION_CONFIG,
        id,
        &fields,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn converts_legacy_keys_and_direct_fields_without_changing_origins_and_is_idempotent() {
    let db = conversion_database().await;
    let before = legacy_key(&db).await;
    let mut public = KeyFixture::new_system_unique("user_json", "unused");
    public.value =
        json!({"encryption_key_hash":old_verifier(),"nested":{"key_hash":old_verifier()}});
    let public = public.create(&db).await.unwrap();
    legacy_field(&db, before.id).await;
    let fields = ExecutionSecretValueRepository::find_stored_by_entity(
        &db,
        ENTITY_EXECUTION_CONFIG,
        before.id,
    )
    .await
    .unwrap();
    let plan = convert_offline(&db, MASTER, false).await.unwrap();
    assert!(!plan.applied);
    assert_eq!(plan.keys_converted, 1);
    assert_eq!(plan.field_verifiers_converted, 1);
    let unchanged = KeyRepository::find_by_id(&db, before.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.value, before.value);
    assert!(unchanged.encryption_key_hash == before.encryption_key_hash);
    assert!(crypto::keystore::decrypt_json(&before.value, MASTER, before.id).is_err());
    let report = convert_offline(&db, MASTER, true).await.unwrap();
    assert!(report.applied);
    assert_eq!(report.keys_converted, 1);
    let after = KeyRepository::find_by_id(&db, before.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.r#ref, before.r#ref);
    assert_eq!(after.name, before.name);
    assert_eq!(after.owner_type, before.owner_type);
    assert_eq!(after.created, before.created);
    assert_eq!(
        crypto::keystore::decrypt_json(&after.value, MASTER, after.id).unwrap(),
        json!({"credential":"synthetic-only"})
    );
    assert!(
        after.encryption_key_hash.as_deref()
            == Some(
                crypto::keystore::fingerprint(MASTER, after.id)
                    .unwrap()
                    .as_str()
            )
    );
    let stored = ExecutionSecretValueRepository::find_stored_by_entity(
        &db,
        ENTITY_EXECUTION_CONFIG,
        before.id,
    )
    .await
    .unwrap();
    assert_eq!(fields[0].encrypted_value, stored[0].encrypted_value);
    assert_eq!(fields[0].source_kind, stored[0].source_kind);
    assert_eq!(fields[0].source_ref, stored[0].source_ref);
    assert_eq!(
        restore_secret_values(json!({"credential":redaction_marker()}), &stored, MASTER).unwrap(),
        json!({"credential":"synthetic-recorded-only"})
    );
    let rerun = convert_offline(&db, MASTER, true).await.unwrap();
    assert_eq!(rerun.keys_converted, 0);
    assert_eq!(rerun.field_verifiers_converted, 0);
    assert_eq!(rerun.keys_already_current, 1);
    assert_eq!(rerun.fields_already_current, 1);
    assert!(
        KeyRepository::find_by_id(&db, public.id)
            .await
            .unwrap()
            .unwrap()
            .value
            == public.value,
        "User JSON resembling fingerprint metadata must remain unchanged"
    );
    assert_eq!(
        KeyRepository::find_by_id(&db, before.id)
            .await
            .unwrap()
            .unwrap()
            .value,
        after.value
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn ordinary_database_identity_cannot_acquire_conversion_or_modify_private_permits() {
    let db = conversion_database().await;
    let name = format!("fp_runtime_{}", uuid::Uuid::new_v4().simple());
    let mut conn = db.acquire().await.unwrap();
    sqlx::query(&format!("CREATE ROLE {name} NOLOGIN"))
        .execute(&mut *conn)
        .await
        .unwrap();
    let schema: String = sqlx::query_scalar("SELECT current_schema()")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query(&format!("GRANT USAGE ON SCHEMA \"{schema}\" TO {name}"))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(&format!("SET SESSION AUTHORIZATION {name}"))
        .execute(&mut *conn)
        .await
        .unwrap();
    let allowed: bool = sqlx::query_scalar("SELECT encryption_conversion_operator()")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let begin = sqlx::query("SELECT encryption_conversion_begin($1)")
        .bind(uuid::Uuid::new_v4())
        .execute(&mut *conn)
        .await;
    let permits = sqlx::query("SELECT count(*) FROM encryption_conversion_change")
        .execute(&mut *conn)
        .await;
    // Restore the owned connection before asserting, even on an unexpected result.
    sqlx::query("RESET SESSION AUTHORIZATION")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(&format!("DROP OWNED BY {name}"))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(&format!("DROP ROLE {name}"))
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(!allowed);
    assert!(begin.is_err());
    assert!(permits.is_err());
    drop(conn);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn wrong_master_and_database_failure_roll_back_every_key_and_allow_retry() {
    let db = conversion_database().await;
    let before = legacy_key(&db).await;
    legacy_field(&db, before.id).await;
    assert!(
        convert_offline(&db, "different-synthetic-conversion-secret-32", true)
            .await
            .is_err()
    );
    sqlx::raw_sql("CREATE FUNCTION reject_conversion_field() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic conversion failure'; END $$; CREATE TRIGGER reject_conversion_field BEFORE UPDATE ON execution_secret_value FOR EACH ROW EXECUTE FUNCTION reject_conversion_field()")
        .execute(&*db).await.unwrap();
    assert!(convert_offline(&db, MASTER, true).await.is_err());
    let unchanged = KeyRepository::find_by_id(&db, before.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.value, before.value);
    assert!(unchanged.encryption_key_hash == before.encryption_key_hash);
    let stored = ExecutionSecretValueRepository::find_stored_by_entity(
        &db,
        ENTITY_EXECUTION_CONFIG,
        before.id,
    )
    .await
    .unwrap();
    assert!(stored[0].encryption_key_hash.as_deref() == Some(old_verifier().as_str()));
    sqlx::query("DROP TRIGGER reject_conversion_field ON execution_secret_value")
        .execute(&*db)
        .await
        .unwrap();
    assert_eq!(
        convert_offline(&db, MASTER, true)
            .await
            .unwrap()
            .keys_converted,
        1
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn retained_graphs_block_conversion_without_rewriting_nested_metadata_or_user_json() {
    let db = conversion_database().await;
    let before = legacy_key(&db).await;
    // An adversarial retained snapshot sentinel, including a lookalike user value.
    let body =
        json!({"payload":[{"encryption_key_hash":old_verifier()}],"user_value":old_verifier()});
    let digest: String = Sha256::digest(serde_jcs::to_vec(&body).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    sqlx::query("INSERT INTO event_origin_snapshot(event,event_created,snapshot,snapshot_digest,evidence,evidence_digest) VALUES(1,NOW(),$1,$2,'{}','sentinel')")
        .bind(&body).bind(&digest).execute(&*db).await.unwrap();
    let error = convert_offline(&db, MASTER, true).await.unwrap_err();
    assert!(error.to_string().contains("event_origin_snapshot"));
    let (stored, stored_digest): (serde_json::Value, String) =
        sqlx::query_as("SELECT snapshot,snapshot_digest FROM event_origin_snapshot")
            .fetch_one(&*db)
            .await
            .unwrap();
    assert!(
        stored == body,
        "Blocked conversion must preserve all nested content"
    );
    assert_eq!(stored_digest, digest);
    assert_eq!(
        KeyRepository::find_by_id(&db, before.id)
            .await
            .unwrap()
            .unwrap()
            .value,
        before.value
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn operator_tool_requires_explicit_environment_and_offline_acknowledgement() {
    let binary = env!("CARGO_BIN_EXE_attune-encryption-convert");
    let help = tokio::process::Command::new(binary)
        .env_clear()
        .arg("--help")
        .output()
        .await
        .unwrap();
    assert!(help.status.success());
    let missing = tokio::process::Command::new(binary)
        .env_clear()
        .arg("--apply")
        .output()
        .await
        .unwrap();
    assert!(!missing.status.success());
    let db = conversion_database().await;
    let before = legacy_key(&db).await;
    for args in [
        vec!["--dry-run"],
        vec!["--apply", "--offline-writers-stopped"],
    ] {
        let output = tokio::process::Command::new(binary)
            .env_clear()
            .args(&args)
            .env(
                "ATTUNE_ENCRYPTION_CONVERSION_DATABASE_URL",
                db.database_url(),
            )
            .env("ATTUNE_ENCRYPTION_CONVERSION_SECRET", MASTER)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "Operator tool failed on an owned disposable database"
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["keys_converted"], 1);
        assert_eq!(report["applied"], args[0] == "--apply");
    }
    let stored = KeyRepository::find_by_id(&db, before.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crypto::keystore::decrypt_json(&stored.value, MASTER, stored.id).unwrap(),
        json!({"credential":"synthetic-only"})
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn interrupted_operator_process_leaves_rows_unchanged_and_conversion_can_retry() {
    use std::time::Duration;
    let db = conversion_database().await;
    let before = legacy_key(&db).await;
    legacy_field(&db, before.id).await;
    let mut blocker = db.begin().await.unwrap();
    sqlx::query("LOCK TABLE key IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_attune-encryption-convert"))
        .env_clear()
        .args(["--apply", "--offline-writers-stopped"])
        .env(
            "ATTUNE_ENCRYPTION_CONVERSION_DATABASE_URL",
            db.database_url(),
        )
        .env("ATTUNE_ENCRYPTION_CONVERSION_SECRET", MASTER)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let readiness = tokio::time::timeout(Duration::from_secs(10), async {
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        loop {
            // The blocker is one long transaction; refresh activity rather than
            // repeatedly reading its first statistics snapshot.
            sqlx::query("SELECT pg_stat_clear_snapshot()")
                .execute(&mut *blocker).await.unwrap();
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query='SELECT encryption_conversion_lock()')")
                .fetch_one(&mut *blocker).await.unwrap();
            if waiting { break; }
            tick.tick().await;
        }
    }).await;
    // Join the exact owned process even when readiness fails.
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    blocker.rollback().await.unwrap();
    assert!(
        readiness.is_ok(),
        "Converter did not reach its observed lock boundary"
    );
    let unchanged = KeyRepository::find_by_id(&db, before.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.value, before.value);
    assert!(unchanged.encryption_key_hash == before.encryption_key_hash);
    assert_eq!(
        convert_offline(&db, MASTER, true)
            .await
            .unwrap()
            .keys_converted,
        1
    );
    db.cleanup().await.unwrap();
}
