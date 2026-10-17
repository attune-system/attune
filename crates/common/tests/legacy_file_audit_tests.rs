//! Native-controller protocol sources, using real owned filesystem bytes.
//! The test controller's writer stop is an owned joined task, never an age test.
use attune_common::{
    blob_store::{self, BlobStore, FilesystemBlobStore, ObjectKey, ProviderVersion},
    config::Config,
    repositories::{
        file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        file_legacy_audit::*,
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
        },
        object_maintenance::ObjectMaintenanceRepository,
        Create,
    },
    test_database::TestDatabase,
    Error, Result,
};
use bytes::Bytes;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use uuid::Uuid;

struct NativeFixture {
    store: Arc<FilesystemBlobStore>,
    scope: LegacyAuditScope,
    items: Vec<LegacyAuditItem>,
    stopped: Arc<AtomicBool>,
    fail_delete: AtomicBool,
}
impl NativeFixture {
    fn fence(&self, plan: &LegacyAuditPlan) -> Result<NativeWriterFence> {
        if !self.stopped.load(Ordering::SeqCst) {
            return Err(Error::invalid_state(
                "owned native writer has not been joined",
            ));
        }
        Ok(NativeWriterFence {
            scope: self.scope.clone(),
            plan_digest: hash(plan),
            stopped_native_instances: plan
                .members
                .iter()
                .filter_map(|m| m.native_instance)
                .collect(),
            stopped_worker_instances: plan
                .members
                .iter()
                .filter_map(|m| m.worker_instance)
                .collect(),
            stopped_execution_attempts: plan
                .members
                .iter()
                .filter_map(|m| m.execution_attempt)
                .collect(),
            restart_fence: json!({"fixture_writer_joined":true,"old_generation_cannot_restart":true}),
            provider_request_settlement: json!({"filesystem_fixture_has_no_pending_remote_requests":true}),
            namespace_ownership: json!({"owned_disposable_backend":self.scope.backend}),
        })
    }
}
fn hash(value: &impl serde::Serialize) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(serde_jcs::to_vec(value).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
#[async_trait::async_trait]
impl NativeLegacyAuditor for NativeFixture {
    fn scope(&self) -> LegacyAuditScope {
        self.scope.clone()
    }
    async fn stop_and_settle_writers(&self, plan: &LegacyAuditPlan) -> Result<NativeWriterFence> {
        self.fence(plan)
    }
    async fn resume_fence(
        &self,
        plan: &LegacyAuditPlan,
        receipt: &Value,
    ) -> Result<NativeWriterFence> {
        let fence = self.fence(plan)?;
        if serde_json::to_value(&fence)? != *receipt {
            return Err(Error::invalid_state("native fixture fence changed"));
        }
        Ok(fence)
    }
    async fn catalogue(
        &self,
        plan: &LegacyAuditPlan,
        fence: &NativeWriterFence,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<LegacyAuditPage> {
        assert!(limit <= 128);
        self.fence(plan)?;
        let offset = match cursor {
            None => 0,
            Some("next-owned-page") => 1,
            _ => return Err(Error::invalid_state("unknown native cursor")),
        };
        let terminal = offset == 1;
        Ok(LegacyAuditPage {
            items: if terminal {
                self.items[1..].to_vec()
            } else {
                self.items[..1].to_vec()
            },
            next_cursor: (!terminal).then(|| "next-owned-page".into()),
            complete: terminal.then(|| NativeCatalogueCompletion {
                plan_digest: hash(plan),
                fence_digest: hash(fence),
                proof: json!({"all_owned_fixture_versions_and_sessions_catalogued":true}),
            }),
        })
    }
    async fn delete_and_confirm_exact(
        &self,
        plan: &LegacyAuditPlan,
        fence: &NativeWriterFence,
        item: &LegacyAuditItem,
    ) -> Result<ExactLegacyAbsence> {
        self.fence(plan)?;
        if self.fail_delete.swap(false, Ordering::SeqCst) {
            return Err(Error::invalid_state(
                "injected exact provider deletion failure",
            ));
        }
        assert_eq!(
            item.ownership,
            Ownership::Owned,
            "unowned/unknown bytes must never reach native deletion"
        );
        let key = ObjectKey::new(item.key.clone()).unwrap();
        let version = ProviderVersion::from_stored(item.provider_identity.clone()).unwrap();
        self.store
            .delete(&key, &version)
            .await
            .map_err(|_| Error::invalid_state("exact fixture deletion failed"))?;
        if self
            .store
            .head_version(&key, &version)
            .await
            .map_err(|_| Error::invalid_state("fixture provider changed under fence"))?
            .is_some()
        {
            return Err(Error::invalid_state("fixture provider still exists"));
        }
        Ok(ExactLegacyAbsence {
            item_digest: hash(item),
            fence_digest: hash(fence),
            proof: json!({"key":item.key,"provider":item.provider_identity,"native_absence_confirmed":true}),
        })
    }
}

async fn fixture() -> (
    TestDatabase,
    tempfile::TempDir,
    NativeFixture,
    attune_common::auth::jwt::Claims,
) {
    let config = Config::load_from_file(&format!(
        "{}/../../config.test.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let db = TestDatabase::create(&config.database)
        .await
        .unwrap()
        .with_cleanup_on_drop();
    let operator = IdentityRepository::create(
        db.pool(),
        CreateIdentityInput {
            login: "owned-legacy-auditor".into(),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await
    .unwrap();
    let set = PermissionSetRepository::create(
        db.pool(),
        CreatePermissionSetInput {
            r#ref: "fixture.legacy-audit-operator".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"permissions","actions":["manage"]}]),
        },
    )
    .await
    .unwrap();
    PermissionAssignmentRepository::create(
        db.pool(),
        CreatePermissionAssignmentInput {
            identity: operator.id,
            permset: set.id,
        },
    )
    .await
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(FilesystemBlobStore::new(directory.path()).unwrap());
    let mut items = Vec::new();
    for (path, kind, ownership) in [
        (
            "artifacts/legacy/file",
            LegacyItemKind::FinalObject,
            Ownership::Owned,
        ),
        (
            ".attune-upload/41",
            LegacyItemKind::RemoteStaging,
            Ownership::Owned,
        ),
        (
            ".attune-upload/99",
            LegacyItemKind::RemoteStaging,
            Ownership::Unowned,
        ),
    ] {
        let key = ObjectKey::new(path).unwrap();
        let bytes = Bytes::from_static(b"owned audit test ciphertext");
        let object = store
            .put(
                &key,
                blob_store::body_from_bytes(bytes.clone()),
                blob_store::sha256(&bytes),
            )
            .await
            .unwrap();
        items.push(LegacyAuditItem {
            kind,
            key: path.into(),
            provider_identity: object.provider_version.as_stored().into(),
            ownership,
            ownership_proof: json!({"native_fixture_inventory":path}),
        });
        if path == "artifacts/legacy/file" {
            ObjectMaintenanceRepository::reserve_upload(db.pool(), path, "artifact")
                .await
                .unwrap();
            ObjectMaintenanceRepository::record_uploaded(
                db.pool(),
                path,
                object.provider_version.as_stored(),
                object.size as i64,
            )
            .await
            .unwrap();
        }
    }
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_key,staging_done,metadata_gone,evidence_state) VALUES(9301,9302,9303,'artifacts/legacy/file',true,true,'quarantined')").execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) VALUES(9301,'execution',9304)")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO file_evidence_put(attempt,owner,protocol,native_instance) VALUES($1,9301,'legacy_unknown',$2)").bind(Uuid::new_v4()).bind(Uuid::new_v4()).execute(db.pool()).await.unwrap();
    let scope = LegacyAuditScope {
        backend: format!("owned-fs:{}", directory.path().display()),
        staging_prefix: ".attune-upload/".into(),
    };
    (
        db,
        directory,
        NativeFixture {
            store,
            scope,
            items,
            stopped: Arc::new(AtomicBool::new(false)),
            fail_delete: AtomicBool::new(true),
        },
        attune_common::auth::jwt::Claims {
            sub: operator.id.to_string(),
            login: operator.login,
            iat: chrono::Utc::now().timestamp(),
            exp: chrono::Utc::now().timestamp() + 3600,
            token_type: attune_common::auth::jwt::TokenType::Access,
            scope: None,
            metadata: None,
        },
    )
}

#[tokio::test]
async fn joined_writer_cursor_restart_and_exact_retry_are_required_before_legacy_pin_release() {
    let (db, _directory, native, operator) = fixture().await;
    let mut process = operator.clone();
    process.token_type = attune_common::auth::jwt::TokenType::Execution;
    assert!(
        LegacyFileAuditRepository::request(db.pool(), &process, &[9301], &native)
            .await
            .is_err(),
        "a process must not borrow its sponsor's operator IAM"
    );
    let denied = LegacyFileAuditRepository::request(db.pool(), &operator, &[9301], &native)
        .await
        .unwrap();
    let mut retirement = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut retirement).await.unwrap();
    assert!(
        sqlx::query("DELETE FROM file_evidence_put WHERE owner=9301")
            .execute(&mut *retirement)
            .await
            .is_err(),
        "exclusive graph admission alone cannot settle an unknown native writer"
    );
    retirement.rollback().await.unwrap();
    assert_eq!(
        LegacyFileAuditRepository::step(db.pool(), denied, &native)
            .await
            .unwrap(),
        "manual_required"
    );
    let mut conn = db.pool().acquire().await.unwrap();
    assert!(!Evidence::eligible(&mut conn, "execution", 9304)
        .await
        .unwrap());
    drop(conn);
    // A real owned task is stopped and JOINED before the fixture controller may
    // mint its native test fence. A sleep/TTL would not establish this fact.
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let state = native.stopped.clone();
    let writer = tokio::spawn(async move {
        stopped.await.unwrap();
        state.store(true, Ordering::SeqCst);
    });
    stop.send(()).unwrap();
    writer.await.unwrap();
    let audit = LegacyFileAuditRepository::request(db.pool(), &operator, &[9301], &native)
        .await
        .unwrap();
    assert_eq!(
        LegacyFileAuditRepository::step(db.pool(), audit, &native)
            .await
            .unwrap(),
        "cataloguing"
    );
    let cursor: String = sqlx::query_scalar("SELECT cursor FROM file_legacy_audit WHERE id=$1")
        .bind(audit)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(cursor, "next-owned-page");
    assert!(
        native
            .store
            .head(&ObjectKey::new("artifacts/legacy/file").unwrap())
            .await
            .unwrap()
            .is_some(),
        "cataloguing must not delete even a known owned final object"
    );
    // The next call resumes from persisted cursor/fence rather than assuming a
    // missing client task means native work or multipart requests stopped.
    assert_eq!(
        LegacyFileAuditRepository::step(db.pool(), audit, &native)
            .await
            .unwrap(),
        "cleaning"
    );
    assert!(LegacyFileAuditRepository::step(db.pool(), audit, &native)
        .await
        .is_err());
    let held: bool = sqlx::query_scalar("SELECT file_evidence_pinned('execution',9304)")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(held);
    assert_eq!(
        LegacyFileAuditRepository::step(db.pool(), audit, &native)
            .await
            .unwrap(),
        "cleaning"
    );
    assert_eq!(
        LegacyFileAuditRepository::step(db.pool(), audit, &native)
            .await
            .unwrap(),
        "completed"
    );
    assert_eq!(
        LegacyFileAuditRepository::step(db.pool(), audit, &native)
            .await
            .unwrap(),
        "completed"
    );
    let held: bool = sqlx::query_scalar("SELECT file_evidence_pinned('execution',9304)")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(!held);
    assert!(
        native
            .store
            .head(&ObjectKey::new(".attune-upload/99").unwrap())
            .await
            .unwrap()
            .is_some(),
        "unowned bodies survive namespace audit"
    );
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM file_legacy_audit_evidence WHERE audit=$1")
            .bind(audit)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(receipts, 7);
    assert!(
        sqlx::query("DELETE FROM file_legacy_audit_evidence WHERE audit=$1")
            .bind(audit)
            .execute(db.pool())
            .await
            .is_err(),
        "audit evidence is immutable"
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn unknown_staging_ownership_is_actionable_manual_state_not_permission_to_delete() {
    let (db, _directory, mut native, operator) = fixture().await;
    native.stopped.store(true, Ordering::SeqCst);
    native.items[1].ownership = Ownership::Unknown;
    let audit = LegacyFileAuditRepository::request(db.pool(), &operator, &[9301], &native)
        .await
        .unwrap();
    LegacyFileAuditRepository::step(db.pool(), audit, &native)
        .await
        .unwrap();
    LegacyFileAuditRepository::step(db.pool(), audit, &native)
        .await
        .unwrap();
    assert_eq!(
        LegacyFileAuditRepository::step(db.pool(), audit, &native)
            .await
            .unwrap(),
        "manual_required"
    );
    let diagnostic: String =
        sqlx::query_scalar("SELECT diagnostic FROM file_legacy_audit WHERE id=$1")
            .bind(audit)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(diagnostic.contains("export/isolation") && diagnostic.contains("Never reset"));
    assert!(native
        .store
        .head(&ObjectKey::new("artifacts/legacy/file").unwrap())
        .await
        .unwrap()
        .is_some());
    let unresolved: i64 =
        sqlx::query_scalar("SELECT count(*) FROM file_evidence_put WHERE owner=9301")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(unresolved, 1);
    db.cleanup().await.unwrap();
}
