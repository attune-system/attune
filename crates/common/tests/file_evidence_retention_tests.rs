//! Unexecuted PG18 retention/partition regression sources. Pins are evidence,
//! not IAM grants; disclosure is exercised by F's real API route fixture.
use attune_common::{
    config::{Config, NativeMaintenanceConfig, RetentionConfig},
    repositories::{
        file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        native_maintenance::{partitions::PartitionRepository, ManagedTable},
        retention::{RetentionRepository, RetentionTarget},
    },
    test_database::TestDatabase,
};
use chrono::{DateTime, Duration, Utc};

async fn database() -> TestDatabase {
    let config = Config::load_from_file(&format!(
        "{}/../../config.test.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    TestDatabase::create(&config.database)
        .await
        .unwrap()
        .with_cleanup_on_drop()
}

#[tokio::test]
async fn accepted_archive_ciphertext_survives_until_exact_object_and_staging_cleanup() {
    archive_ciphertext_retention("accepted_input", "accepted_input").await;
}

#[tokio::test]
async fn native_snapshot_archive_ciphertext_survives_until_exact_object_and_staging_cleanup() {
    archive_ciphertext_retention("native_input_snapshot", "runtime_native_input_snapshot").await;
}

async fn archive_ciphertext_retention(entity: &str, pin_kind: &str) {
    use attune_common::{
        blob_store::{body_from_bytes, sha256, BlobStore, FilesystemBlobStore, ObjectKey},
        repositories::execution_secret_value::ExecutionSecretValueRepository,
        secret_provenance::{SecretOrigin, SecretProvenance},
        secret_values::{prepare_secret_values, SecretValueInput},
    };
    use bytes::Bytes;
    use serde_json::json;
    let db = database().await;
    let directory = tempfile::tempdir().unwrap();
    let store = FilesystemBlobStore::new(directory.path().join("objects")).unwrap();
    let prepared = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/input".into(),
            value: json!("owned archive fixture"),
            source_kind: "provenance".into(),
            source_ref: Some(
                serde_json::to_string(&SecretProvenance {
                    origins: vec![SecretOrigin::Local {
                        source_kind: "parameter_schema".into(),
                        source_ref: Some("/input".into()),
                    }],
                    templates: vec![],
                })
                .unwrap(),
            ),
        }],
        "owned-accepted-archive-test-key-32-bytes-minimum",
    )
    .unwrap();
    let ciphertext = Bytes::from(serde_json::to_vec(&prepared[0].encrypted_value).unwrap());
    let key = ObjectKey::new("accepted-archive/owned-file").unwrap();
    let object = store
        .put(
            &key,
            body_from_bytes(ciphertext.clone()),
            sha256(&ciphertext),
        )
        .await
        .unwrap();
    let staging = directory.path().join("staged");
    tokio::fs::write(&staging, &ciphertext).await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    // Dormant internal storage-owner fixture, not an authority-valid receipt.
    // A's seal/retention_parents tests authenticate archives; this test exercises
    // F's independently owned ciphertext guard and physical release boundary.
    ExecutionSecretValueRepository::upsert_many_with_conn(&mut tx, entity, 97004, &prepared)
        .await
        .unwrap();
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_key,provider_version,staging_path) VALUES(97001,97002,97003,$1,$2,'staged')")
        .bind(key.as_str()).bind(object.provider_version.as_stored()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) VALUES(97001,$1,97004)")
        .bind(pin_kind)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query("DELETE FROM execution_secret_value WHERE entity_type=$1 AND entity_id=97004")
            .bind(entity)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected(),
        0
    );
    Evidence::request_cleanup(&mut tx, &[97001]).await.unwrap();
    sqlx::query("UPDATE file_evidence_owner SET metadata_gone=true WHERE version=97001")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    assert!(sqlx::query("UPDATE execution_secret_value SET encrypted_value='{}' WHERE entity_type=$1 AND entity_id=97004").bind(entity).execute(&mut *tx).await.is_err());
    tx.rollback().await.unwrap();
    store.delete(&key, &object.provider_version).await.unwrap();
    assert!(store
        .head_version(&key, &object.provider_version)
        .await
        .unwrap()
        .is_none());
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    Evidence::object_removed(&mut tx, key.as_str(), Some("v:not-the-recorded-provider"))
        .await
        .unwrap();
    let object_done: bool =
        sqlx::query_scalar("SELECT object_done FROM file_evidence_owner WHERE version=97001")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(
        !object_done,
        "even an absent object does not qualify a wrong provider identity"
    );
    Evidence::object_removed(
        &mut tx,
        key.as_str(),
        Some(object.provider_version.as_stored()),
    )
    .await
    .unwrap();
    Evidence::staging_removed(&mut tx, "wrong-staging-path", 97001)
        .await
        .unwrap();
    assert!(!Evidence::eligible(&mut tx, pin_kind, 97004).await.unwrap());
    assert_eq!(
        sqlx::query("DELETE FROM execution_secret_value WHERE entity_type=$1 AND entity_id=97004")
            .bind(entity)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected(),
        0
    );
    tx.commit().await.unwrap();
    assert!(staging.exists());
    tokio::fs::remove_file(staging).await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    Evidence::staging_removed(&mut tx, "staged", 97001)
        .await
        .unwrap();
    assert!(Evidence::eligible(&mut tx, pin_kind, 97004).await.unwrap());
    assert_eq!(
        sqlx::query("DELETE FROM execution_secret_value WHERE entity_type=$1 AND entity_id=97004")
            .bind(entity)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn pre_06005_upgrade_parks_and_restores_original_reservations_without_rewriting_history() {
    use attune_common::{
        models::{
            ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType,
            RetentionPolicyType,
        },
        repositories::{
            artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
            Create,
        },
    };
    let config = Config::load_from_file(&format!(
        "{}/../../config.test.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let db = TestDatabase::create_before_with_history(
        &config.database,
        "20261015006005_file_evidence_retention.sql",
    )
    .await
    .unwrap()
    .with_cleanup_on_drop();
    let original: i64 = sqlx::query_scalar("SELECT 'artifact_file_origin'::regclass::oid::bigint")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let checksums = db.migration_history().await.unwrap();
    assert!(
        !checksums.is_empty(),
        "upgrade assertions require real runner-created history"
    );
    let artifact = ArtifactRepository::create(
        db.pool(),
        CreateArtifactInput {
            r#ref: "upgrade.old-file".into(),
            scope: OwnerType::System,
            owner: String::new(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::General,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 1,
            name: None,
            description: None,
            content_type: Some("text/plain".into()),
            data: None,
        },
    )
    .await
    .unwrap();
    let version = ArtifactVersionRepository::create_object_pending(
        db.pool(),
        artifact.id,
        Some(9201),
        "text/plain".into(),
        None,
        None,
    )
    .await
    .unwrap();
    // Deliberately incomplete historical evidence must preserve physical ownership
    // and produce a located quarantine, not invented admission authority.
    sqlx::query("INSERT INTO artifact_file_origin(artifact_version,origin_read,admission,execution,attempt,sponsor,max_plaintext_bytes,source_digest,receipts) VALUES($1,9202,9203,9201,$2,9204,1024,'missing-proof','[]')")
        .bind(version.id).bind(uuid::Uuid::new_v4()).execute(db.pool()).await.unwrap();
    for _ in 0..2 {
        sqlx::raw_sql(include_str!(
            "../../../scripts/prepare-file-evidence-upgrade.sql"
        ))
        .execute(db.pool())
        .await
        .unwrap();
    }
    let parked:(i64,i64,i64)=sqlx::query_as("SELECT 'file_evidence_upgrade_reservation'::regclass::oid::bigint,(SELECT count(*) FROM file_evidence_upgrade_reservation),(SELECT count(*) FROM artifact_file_origin)").fetch_one(db.pool()).await.unwrap();
    assert_eq!(parked, (original, 1, 0));
    sqlx::raw_sql(include_str!(
        "../../../migrations/20261015006005_file_evidence_retention.sql"
    ))
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/20261015006006_file_evidence_readmission.sql"
    ))
    .execute(db.pool())
    .await
    .unwrap();
    let restored: i64 = sqlx::query_scalar("SELECT 'artifact_file_origin'::regclass::oid::bigint")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(restored, original);
    assert!(!Evidence::readmit(db.pool(), version.id).await.unwrap());
    let owner: (String, Option<String>, bool) = sqlx::query_as(
        "SELECT evidence_state,object_key,object_done FROM file_evidence_owner WHERE version=$1",
    )
    .bind(version.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(owner, ("quarantined".into(), version.object_key, false));
    let after = db.migration_history().await.unwrap();
    assert_eq!(checksums, after);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn crash_after_native_put_and_restart_before_receipt_keeps_pins_until_provider_fence() {
    use attune_common::{
        blob_store::{
            body_from_bytes, settle_atomic_file_put, sha256, BlobStore, FilesystemBlobStore,
            ObjectKey,
        },
        repositories::object_maintenance::ObjectMaintenanceRepository,
    };
    use bytes::Bytes;
    let db = database().await;
    let directory = tempfile::tempdir().unwrap();
    let store = FilesystemBlobStore::new(directory.path()).unwrap();
    let key = ObjectKey::new("file-settlement/crashed-writer").unwrap();
    let payload = Bytes::from_static(b"encrypted file body whose native result was not persisted");
    ObjectMaintenanceRepository::reserve_upload(db.pool(), key.as_str(), "artifact")
        .await
        .unwrap();
    let put = uuid::Uuid::new_v4();
    // This is the owning atomic-PUT crash fixture, not legacy enrollment. Enter
    // graph admission before creating its owner/pins/request, and commit that
    // durable admission before the simulated native provider call below.
    let mut admission = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut admission).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_key,staging_done) VALUES(9101,9102,9103,$1,true)")
        .bind(key.as_str()).execute(&mut *admission).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) VALUES(9101,'execution',9104)")
        .execute(&mut *admission)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO file_evidence_put(attempt,owner,protocol) VALUES($1,9101,'atomic_create_v1')",
    )
    .bind(put)
    .execute(&mut *admission)
    .await
    .unwrap();
    admission.commit().await.unwrap();
    let old = store
        .put_file_atomic(
            &key,
            body_from_bytes(payload.clone()),
            payload.len() as u64,
            sha256(&payload),
        )
        .await
        .unwrap();
    // Simulate process death after physical completion, before finish_put. No
    // remote absence/TTL assumption is needed for the restarted reconciler.
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    Evidence::request_cleanup(&mut tx, &[9101]).await.unwrap();
    sqlx::query("UPDATE file_evidence_owner SET metadata_gone=true WHERE version=9101")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT file_evidence_release(9101)")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let held: bool = sqlx::query_scalar("SELECT file_evidence_pinned('execution',9104)")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(held);
    let plans = Evidence::recovery_candidates(db.pool(), 1).await.unwrap();
    assert_eq!(plans.len(), 1);
    let observation = settle_atomic_file_put(&store, &plans[0].key().unwrap(), plans[0].token())
        .await
        .unwrap();
    // Crash after provider settlement but before its DB receipt. Pins still hold.
    let held: bool = sqlx::query_scalar("SELECT file_evidence_pinned('execution',9104)")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(held);
    let expected = observation.provider().clone();
    drop(observation);
    drop(store);
    let restarted = FilesystemBlobStore::new(directory.path()).unwrap();
    let retry = Evidence::recovery_candidates(db.pool(), 1).await.unwrap();
    assert_eq!(retry[0].token(), plans[0].token());
    let observation =
        settle_atomic_file_put(&restarted, &retry[0].key().unwrap(), retry[0].token())
            .await
            .unwrap();
    assert_eq!(observation.provider(), &expected);
    Evidence::complete_put_recovery(db.pool(), &retry[0], &observation)
        .await
        .unwrap();
    let held: bool = sqlx::query_scalar("SELECT file_evidence_pinned('execution',9104)")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(!held);
    assert!(
        Evidence::finish_put(db.pool(), Some(put), Some(&old))
            .await
            .is_err(),
        "a late old result cannot rebind the retired owner or publish Ready"
    );
    let current = restarted.head(&key).await.unwrap().unwrap();
    assert_eq!(&current.provider_version, observation.provider());
    let ledger: String =
        sqlx::query_scalar("SELECT state FROM object_maintenance_ledger WHERE object_key=$1")
            .bind(key.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(ledger, "write_fenced");
    assert!(
        ObjectMaintenanceRepository::claim_deletions(db.pool(), Utc::now() + Duration::days(1), 10)
            .await
            .unwrap()
            .is_empty(),
        "neither old nor new GC may remove the non-content anti-resurrection sentinel"
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn readmission_migration_preserves_orphan_object_and_staging_ownership_without_checksum_edits(
) {
    use attune_common::{
        blob_store::{body_from_bytes, sha256, BlobStore, FilesystemBlobStore, ObjectKey},
        repositories::object_maintenance::ObjectMaintenanceRepository,
    };
    use bytes::Bytes;
    let config = Config::load_from_file(&format!(
        "{}/../../config.test.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let db = TestDatabase::create_before_with_history(
        &config.database,
        "20261015006006_file_evidence_readmission.sql",
    )
    .await
    .unwrap()
    .with_cleanup_on_drop();
    let checksums = db.migration_history().await.unwrap();
    assert!(
        !checksums.is_empty(),
        "upgrade assertions require real runner-created history"
    );
    let directory = tempfile::tempdir().unwrap();
    let store = FilesystemBlobStore::new(directory.path()).unwrap();
    let key = ObjectKey::new("upgrade/orphan").unwrap();
    let bytes = Bytes::from_static(b"orphan body remains owned");
    let object = store
        .put(&key, body_from_bytes(bytes.clone()), sha256(&bytes))
        .await
        .unwrap();
    ObjectMaintenanceRepository::reserve_upload(db.pool(), key.as_str(), "artifact")
        .await
        .unwrap();
    ObjectMaintenanceRepository::record_uploaded(
        db.pool(),
        key.as_str(),
        object.provider_version.as_stored(),
        object.size as i64,
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO artifact_file_staging_guard(file_path,artifact_version) VALUES('files/versions/9005/orphan',9005)").execute(db.pool()).await.unwrap();
    let mut trial = db.pool().begin().await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/20261015006006_file_evidence_readmission.sql"
    ))
    .execute(&mut *trial)
    .await
    .unwrap();
    trial.rollback().await.unwrap();
    let rolled_back:bool=sqlx::query_scalar("SELECT to_regclass('file_evidence_orphan_object') IS NULL AND EXISTS(SELECT 1 FROM object_maintenance_ledger WHERE object_key='upgrade/orphan') AND EXISTS(SELECT 1 FROM artifact_file_staging_guard WHERE artifact_version=9005)")
        .fetch_one(db.pool()).await.unwrap();
    assert!(
        rolled_back,
        "transactional migration rollback must preserve prior physical ownership"
    );
    sqlx::raw_sql(include_str!(
        "../../../migrations/20261015006006_file_evidence_readmission.sql"
    ))
    .execute(db.pool())
    .await
    .unwrap();
    let after = db.migration_history().await.unwrap();
    assert_eq!(
        checksums, after,
        "a forward upgrade does not rewrite applied migration history"
    );
    let staging:(bool,bool,String)=sqlx::query_as("SELECT metadata_gone,staging_done,evidence_state FROM file_evidence_owner WHERE version=9005").fetch_one(db.pool()).await.unwrap();
    assert_eq!(staging, (true, false, "quarantined".into()));
    let owned: Option<String> = sqlx::query_scalar(
        "SELECT provider_version FROM file_evidence_orphan_object WHERE object_key=$1",
    )
    .bind(key.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(owned.as_deref(), Some(object.provider_version.as_stored()));
    let ledger: i64 = sqlx::query_scalar(
        "UPDATE object_maintenance_ledger SET state='deleting' WHERE object_key=$1 RETURNING id",
    )
    .bind(key.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    store.delete(&key, &object.provider_version).await.unwrap();
    assert!(
        ObjectMaintenanceRepository::complete_delete(db.pool(), ledger)
            .await
            .unwrap()
    );
    let held: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM file_evidence_orphan_object WHERE object_key=$1)",
    )
    .bind(key.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(!held);
    assert!(store.head(&key).await.unwrap().is_none());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn missing_upgrade_graph_is_located_and_quarantined_without_losing_cleanup_ownership() {
    let db = database().await;
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,evidence_state,object_key,staging_path) VALUES(9001,9002,9003,'pending_audit','upgrade/9001','files/versions/9001/pending')")
        .execute(db.pool()).await.unwrap();
    assert!(!Evidence::readmit(db.pool(), 9001).await.unwrap());
    let (state,diagnostic,gone,object_done,staging_done):(String,String,bool,bool,bool)=sqlx::query_as("SELECT evidence_state,diagnostic,metadata_gone,object_done,staging_done FROM file_evidence_owner WHERE version=9001")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(state, "quarantined");
    assert!(diagnostic.contains("version 9001") && diagnostic.contains("root/9003"));
    assert!(!gone && !object_done && !staging_done);
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    assert!(Evidence::require_retained(&mut tx, 9001).await.is_err());
    assert_eq!(
        Evidence::request_cleanup(&mut tx, &[9001]).await.unwrap(),
        vec![9001]
    );
    sqlx::query("UPDATE file_evidence_owner SET metadata_gone=true WHERE version=9001")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT file_evidence_release(9001)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let held: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM file_evidence_owner WHERE version=9001)")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(
        held,
        "failed audit and metadata removal cannot forget native cleanup obligations"
    );
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn requested_cleanup_closure_handles_self_cycles_without_absorbing_outside_files() {
    let db = database().await;
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_done,staging_done) VALUES(1,11,101,true,true),(2,12,102,true,true),(3,13,103,true,true)")
        .execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) VALUES(1,'artifact_version',1),(1,'artifact_version',2),(2,'artifact_version',1),(3,'artifact_version',1)")
        .execute(db.pool()).await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    assert!(Evidence::request_cleanup(&mut tx, &[1])
        .await
        .unwrap()
        .is_empty());
    assert!(Evidence::request_cleanup(&mut tx, &[1, 2])
        .await
        .unwrap()
        .is_empty());
    // The outside owner stays alive. Requesting the cycle does not silently
    // include it or discard its pins. Finish that owner separately first.
    assert_eq!(
        Evidence::request_cleanup(&mut tx, &[3]).await.unwrap(),
        vec![3]
    );
    sqlx::query("UPDATE file_evidence_owner SET metadata_gone=true WHERE version=3")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT file_evidence_release(3)")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        Evidence::request_cleanup(&mut tx, &[1, 2]).await.unwrap(),
        vec![1, 2]
    );
    let eligible: bool = sqlx::query_scalar(
        "SELECT file_evidence_version_eligible(1) AND file_evidence_version_eligible(2)",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(eligible);
    sqlx::query("UPDATE file_evidence_owner SET metadata_gone=true WHERE version=1")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT file_evidence_release(1)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let held: i64 =
        sqlx::query_scalar("SELECT count(*) FROM file_evidence_owner WHERE version IN (1,2)")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        held, 2,
        "one cycle member cannot release evidence before its peer's physical obligations"
    );
    sqlx::query("UPDATE file_evidence_owner SET metadata_gone=true WHERE version=2")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT file_evidence_release(2)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let held: i64 =
        sqlx::query_scalar("SELECT count(*) FROM file_evidence_owner WHERE version IN (1,2)")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(held, 0);
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn capture_gate_serializes_reclamation_and_rejects_late_upgrade() {
    let db = database().await;
    let mut capture = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    let mut reclaim = db.pool().begin().await.unwrap();
    let admitted:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended(current_schema() || ':named-type-graph',0))")
        .fetch_one(&mut *reclaim).await.unwrap();
    assert!(
        !admitted,
        "GC must not race a reservation holding shared graph admission"
    );
    assert!(
        Evidence::lock_reclamation(&mut capture).await.is_err(),
        "late upgrade behind sources must fail rather than deadlock another capture"
    );
    capture.rollback().await.unwrap();
    Evidence::lock_reclamation(&mut reclaim).await.unwrap();
    let mut later = db.pool().begin().await.unwrap();
    let admitted:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock_shared(hashtextextended(current_schema() || ':named-type-graph',0))")
        .fetch_one(&mut *later).await.unwrap();
    assert!(
        !admitted,
        "a deletion decision excludes new evidence capture until commit"
    );
    later.rollback().await.unwrap();
    reclaim.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn exact_event_pin_blocks_leaf_drop_but_not_bounded_unpinned_row_cleanup() {
    let db = database().await;
    let day = DateTime::parse_from_rfc3339("2020-08-02T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let pinned = day + Duration::hours(1);
    // Deliberately repeat the ID in the same partition at a different instant.
    sqlx::query("INSERT INTO event(id,created,trigger_ref) VALUES(7001,$1,'retention.fixture'),(7001,$2,'retention.fixture')")
        .bind(pinned).bind(day+Duration::hours(2)).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO event(created,trigger_ref) SELECT $1,'retention.fixture' FROM generate_series(1,17)")
        .bind(day+Duration::hours(3)).execute(db.pool()).await.unwrap();
    let native = NativeMaintenanceConfig {
        enabled: true,
        max_partition_operations_per_cycle: 1,
        default_repair_row_limit: 100,
        operation_timeout_milliseconds: 10000,
        max_partition_cycle_milliseconds: 20000,
        ..Default::default()
    };
    PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, day, &native)
        .await
        .unwrap();
    PartitionRepository::ensure_day(
        db.pool(),
        ManagedTable::Event,
        day + Duration::days(1),
        &native,
    )
    .await
    .unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut tx).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_done,staging_done) VALUES(1,11,101,true,true)").execute(&mut *tx).await.unwrap();
    sqlx::query(
        "INSERT INTO file_evidence_pin(owner,kind,source,source_created) VALUES(1,'event',7001,$1)",
    )
    .bind(pinned)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let expiry = PartitionRepository::expire_before(
        db.pool(),
        ManagedTable::Event,
        day + Duration::days(2),
        &native,
        false,
        || false,
    )
    .await
    .unwrap();
    assert_eq!(
        expiry.partitions_dropped, 1,
        "a pinned oldest leaf must not consume the sole DDL slot and starve an unpinned leaf"
    );
    // Observe parent invalidation at its actual transaction boundary. The
    // retention runner deliberately expires old summary coverage afterward.
    let mut invalidation = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut invalidation).await.unwrap();
    sqlx::query("DELETE FROM native_summary_invalidation WHERE kind='event_volume' AND bucket=$1")
        .bind(day + Duration::hours(2))
        .execute(&mut *invalidation)
        .await
        .unwrap();
    let removed = sqlx::query("DELETE FROM event WHERE id=7001 AND created=$1")
        .bind(day + Duration::hours(2))
        .execute(&mut *invalidation)
        .await
        .unwrap();
    assert_eq!(
        removed.rows_affected(),
        1,
        "the unpinned tuple sharing an ID remains eligible"
    );
    let marked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM native_summary_invalidation WHERE kind='event_volume' AND bucket=$1)")
        .bind(day+Duration::hours(2)).fetch_one(&mut *invalidation).await.unwrap();
    assert!(
        marked,
        "parent DELETE must invalidate coverage before its transaction can commit"
    );
    invalidation.rollback().await.unwrap();
    let retention = RetentionConfig {
        batch_size: 3,
        max_batches_per_target: 2,
        native_maintenance: native.clone(),
        ..Default::default()
    };
    for _ in 0..4 {
        let result = RetentionRepository::run_target_bounded(
            db.pool(),
            RetentionTarget::Events,
            1,
            &retention,
            || false,
        )
        .await
        .unwrap();
        assert!(result.deleted <= 6);
    }
    let remaining: Vec<(i64, DateTime<Utc>)> =
        sqlx::query_as("SELECT id,created FROM event WHERE created<$1 ORDER BY created,id")
            .bind(day + Duration::days(2))
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(remaining, vec![(7001, pinned)]);
    let expired_coverage_removed: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS(SELECT 1 FROM native_summary_invalidation WHERE kind='event_volume' AND bucket=$1) AND NOT EXISTS(SELECT 1 FROM native_summary_hour WHERE kind='event_volume' AND bucket=$1)",
    )
    .bind(day + Duration::hours(2))
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(
        expired_coverage_removed,
        "expired summaries and markers must not outlive raw retention; the pinned exact event still survives"
    );
    let mut tx = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    sqlx::query("UPDATE file_evidence_owner SET metadata_gone=true WHERE version=1")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT file_evidence_release(1)")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let expiry = PartitionRepository::expire_before(
        db.pool(),
        ManagedTable::Event,
        day + Duration::days(2),
        &native,
        false,
        || false,
    )
    .await
    .unwrap();
    assert_eq!(expiry.partitions_dropped, 1);
    db.cleanup().await.unwrap();
}
