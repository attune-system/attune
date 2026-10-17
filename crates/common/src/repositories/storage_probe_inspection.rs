//! Read-only, run-scoped native acceptance observations, never authorization.
//! No config, policy body, ciphertext, decrypted value or raw provider error is
//! returned. A local operator context is required by the owning example adapter.
use crate::{Error, Result};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use sqlx::{FromRow, PgPool};

const MAX_TEXT: usize = 4096;

fn denied() -> Error {
    Error::invalid_state("Storage inspection scope, identity or observation is invalid")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionAccess {
    pub run_id: String,
    pub project: String,
    pub invoker_identity: i64,
    pub legacy_version_ids: Vec<i64>,
    // Local exact-identity receipt key, NOT a native writer settlement signer.
    pub identity_key: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionScope {
    pub run_id: String,
    pub project: String,
    pub execution_id: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionRequest {
    pub scope: InspectionScope,
    pub operation: InspectionOperation,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InspectionOperation {
    Pins,
    Version,
    Backend,
    Protected { identity: ObservedFileIdentity },
    Ledger { identity: ObservedFileIdentity },
    Released { identity: ObservedFileIdentity },
    Legacy { version_id: i64 },
    LegacyEvidence { identity: ObservedLegacyIdentity },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedLegacyIdentity {
    version_id: i64,
    artifact: i64,
    version: i32,
    root_read: i64,
    mac: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    version_id: i64,
    artifact: i64,
    execution: i64,
    object_key: String,
    provider_version: String,
    staging_path: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedFileIdentity {
    file: FileIdentity,
    mac: String,
}

#[derive(Serialize)]
pub struct InspectionResponse {
    pub protocol: u8,
    pub data: Value,
    pub identity: Option<ObservedFileIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_identity: Option<ObservedLegacyIdentity>,
}

#[derive(Serialize, FromRow)]
struct Pins {
    release_id: i64,
    digest: String,
    policy_revision: i64,
    admission: i64,
    authority_digest: String,
    retained_release_digest: String,
    retained_policy_digest: String,
}
impl Pins {
    const SELECT_COLUMNS: &'static str = "e.pack_release AS release_id,e.pack_release_digest AS digest,a.policy_revision,a.id AS admission,a.authority_digest,r.digest AS retained_release_digest,p.policy_digest AS retained_policy_digest";
}

#[derive(Serialize, FromRow)]
struct FileRecord {
    id: i64,
    artifact: i64,
    version: i32,
    execution: i64,
    file_path: Option<String>,
    object_key: Option<String>,
    provider_version: Option<String>,
    body_state: Option<String>,
    staging_path: Option<String>,
    evidence_state: String,
    object_done: bool,
    staging_done: bool,
}
impl FileRecord {
    const SELECT_COLUMNS: &'static str = "v.id,v.artifact,v.version,v.execution,v.file_path,v.object_key,v.provider_version,v.body_state::TEXT AS body_state,o.staging_path,o.evidence_state,o.object_done,o.staging_done";

    fn identity(&self) -> Result<Option<FileIdentity>> {
        if self.body_state.as_deref() != Some("ready") {
            return Ok(None);
        }
        Ok(Some(FileIdentity {
            version_id: self.id,
            artifact: self.artifact,
            execution: self.execution,
            object_key: self.object_key.clone().ok_or_else(denied)?,
            provider_version: self.provider_version.clone().ok_or_else(denied)?,
            staging_path: self.staging_path.clone().ok_or_else(denied)?,
        }))
    }
}

#[derive(Serialize, FromRow)]
struct Ledger {
    id: i64,
    object_key: String,
    provider_version: String,
    state: String,
    attempts: i32,
    failed: bool,
}
impl Ledger {
    const SELECT_COLUMNS: &'static str = "id,object_key,provider_version,state,attempts,(last_error IS NOT NULL AND last_error<>'') AS failed";
}

#[derive(Serialize, FromRow)]
struct Protected {
    body: bool,
    unsettled: bool,
}
#[derive(Serialize, FromRow)]
struct Released {
    owner: bool,
    pins: bool,
    operations: bool,
}
#[derive(Serialize, FromRow)]
struct Legacy {
    artifact: i64,
    version: i32,
    evidence_state: String,
    root_read: i64,
}

#[derive(Serialize, FromRow)]
struct LegacyEvidence {
    owner: bool,
    root_read: Option<i64>,
    pins: i64,
    operations: i64,
    legacy_operations: i64,
}

fn bounded_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_TEXT && !value.contains('\0')
}

fn relative_identity(value: &str) -> bool {
    bounded_text(value)
        && !value.starts_with('/')
        && value
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

impl InspectionAccess {
    fn legacy_mac(
        &self,
        scope: &InspectionScope,
        identity: &ObservedLegacyIdentity,
    ) -> Result<Hmac<Sha256>> {
        self.validate_scope(scope)?;
        if !self.legacy_version_ids.contains(&identity.version_id)
            || identity.artifact <= 0
            || identity.version <= 0
            || identity.root_read <= 0
        {
            return Err(denied());
        }
        let mut signer =
            Hmac::<Sha256>::new_from_slice(self.identity_key.as_bytes()).map_err(|_| denied())?;
        signer.update(
            &serde_jcs::to_vec(&(
                "legacy-evidence",
                scope,
                identity.version_id,
                identity.artifact,
                identity.version,
                identity.root_read,
            ))
            .map_err(|_| denied())?,
        );
        Ok(signer)
    }

    fn verify_legacy(
        &self,
        scope: &InspectionScope,
        identity: &ObservedLegacyIdentity,
    ) -> Result<()> {
        if identity.mac.len() != 64 || !identity.mac.is_ascii() {
            return Err(denied());
        }
        let bytes: Vec<u8> = identity
            .mac
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| denied())?, 16)
                    .map_err(|_| denied())
            })
            .collect::<Result<_>>()?;
        self.legacy_mac(scope, identity)?
            .verify_slice(&bytes)
            .map_err(|_| denied())
    }

    pub fn validate_scope(&self, scope: &InspectionScope) -> Result<()> {
        let run = scope.run_id.as_bytes();
        if run.is_empty()
            || run.len() > 20
            || !run[0].is_ascii_lowercase()
            || !run
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
            || scope.run_id != self.run_id
            || scope.project != self.project
            || !scope
                .project
                .starts_with(&format!("eaccept-{}-", scope.run_id))
            || scope.project.len() > 128
            || !scope
                .project
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
            || scope.execution_id <= 0
            || self.invoker_identity <= 0
            || self.identity_key.len() < 32
            || self.identity_key.len() > 256
            || self.legacy_version_ids.len() > 16
            || self.legacy_version_ids.iter().any(|id| *id <= 0)
        {
            return Err(denied());
        }
        Ok(())
    }

    fn sign(&self, scope: &InspectionScope, file: FileIdentity) -> Result<ObservedFileIdentity> {
        self.check_file(scope, &file)?;
        let mut signer =
            Hmac::<Sha256>::new_from_slice(self.identity_key.as_bytes()).map_err(|_| denied())?;
        signer.update(&serde_jcs::to_vec(&(scope, &file)).map_err(|_| denied())?);
        let mac = signer
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(ObservedFileIdentity { file, mac })
    }

    fn check_file(&self, scope: &InspectionScope, file: &FileIdentity) -> Result<()> {
        self.validate_scope(scope)?;
        if file.version_id <= 0
            || file.artifact <= 0
            || file.execution != scope.execution_id
            || ![&file.object_key, &file.provider_version, &file.staging_path]
                .iter()
                .all(|s| bounded_text(s))
            || !relative_identity(&file.object_key)
            || !relative_identity(&file.staging_path)
            || crate::blob_store::ProviderVersion::from_stored(file.provider_version.clone())
                .is_err()
        {
            return Err(denied());
        }
        Ok(())
    }

    fn verify<'a>(
        &self,
        scope: &InspectionScope,
        identity: &'a ObservedFileIdentity,
    ) -> Result<&'a FileIdentity> {
        self.check_file(scope, &identity.file)?;
        if identity.mac.len() != 64 || !identity.mac.is_ascii() {
            return Err(denied());
        }
        let bytes: Vec<u8> = identity
            .mac
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| denied())?, 16)
                    .map_err(|_| denied())
            })
            .collect::<Result<_>>()?;
        let mut signer =
            Hmac::<Sha256>::new_from_slice(self.identity_key.as_bytes()).map_err(|_| denied())?;
        signer.update(&serde_jcs::to_vec(&(scope, &identity.file)).map_err(|_| denied())?);
        signer.verify_slice(&bytes).map_err(|_| denied())?;
        Ok(&identity.file)
    }
}

pub struct StorageProbeInspectionRepository;
impl StorageProbeInspectionRepository {
    /// The caller owns local DB credentials. All observations use one bounded
    /// repeatable-read read-only transaction. This API grants no HTTP authority.
    pub async fn inspect(
        pool: &PgPool,
        schema: &str,
        access: &InspectionAccess,
        request: InspectionRequest,
    ) -> Result<InspectionResponse> {
        access.validate_scope(&request.scope)?;
        if schema.is_empty()
            || schema.len() > 63
            || !schema
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT set_config('search_path',quote_ident($1),true),set_config('statement_timeout','5s',true)")
            .bind(schema).execute(&mut *tx).await?;
        let scope = &request.scope;
        let marker = format!(" f-storage-probe-{}", scope.run_id);
        // No config value is selected or returned. Prove this exact run's actual
        // ordinary invoker dispatched the known action before inspecting IDs.
        let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution WHERE id=$1 AND executor=$2 AND action_ref='core.run_agent_command' AND right(config->>'command',length($3))=$3)")
            .bind(scope.execution_id).bind(access.invoker_identity).bind(marker).fetch_one(&mut *tx).await?;
        if !owned {
            return Err(denied());
        }
        let mut identity = None;
        let mut legacy_identity = None;
        let data = match request.operation {
            InspectionOperation::Pins => {
                let query = format!("SELECT {} FROM execution e JOIN runtime_authority_admission a ON a.owner_kind='execution' AND a.owner_id=e.id JOIN pack_release r ON r.id=e.pack_release AND r.digest=e.pack_release_digest JOIN pack_runtime_policy_revision p ON p.id=a.policy_revision AND p.revoked_at IS NULL WHERE e.id=$1 AND octet_length(e.pack_release_digest)<=256 AND octet_length(a.authority_digest)<=256 AND octet_length(p.policy_digest)<=256", Pins::SELECT_COLUMNS);
                serde_json::to_value(
                    sqlx::query_as::<_, Pins>(&query)
                        .bind(scope.execution_id)
                        .fetch_optional(&mut *tx)
                        .await?,
                )?
            }
            InspectionOperation::Version => {
                let query = format!("SELECT {} FROM artifact_version v JOIN artifact a ON a.id=v.artifact JOIN file_evidence_owner o ON o.version=v.id WHERE v.execution=$1 AND a.ref=$2 AND v.meta->>'f_probe_run'=$3 AND octet_length(COALESCE(v.file_path,''))<=4096 AND octet_length(COALESCE(v.object_key,''))<=4096 AND octet_length(COALESCE(v.provider_version,''))<=4096 AND octet_length(COALESCE(o.staging_path,''))<=4096 ORDER BY v.id LIMIT 2", FileRecord::SELECT_COLUMNS);
                let rows = sqlx::query_as::<_, FileRecord>(&query)
                    .bind(scope.execution_id)
                    .bind(format!("core.f_probe_{}", scope.run_id.replace('-', "_")))
                    .bind(&scope.run_id)
                    .fetch_all(&mut *tx)
                    .await?;
                if rows.len() > 1 {
                    return Err(denied());
                }
                match rows.into_iter().next() {
                    None => Value::Null,
                    Some(row) => {
                        identity = row
                            .identity()?
                            .map(|file| access.sign(scope, file))
                            .transpose()?;
                        serde_json::to_value(row)?
                    }
                }
            }
            InspectionOperation::Backend => {
                let rows: Vec<String> = sqlx::query_scalar("SELECT DISTINCT s.backend::TEXT FROM log_stream s JOIN artifact_version v ON v.id=s.artifact_version WHERE v.execution=$1 LIMIT 2")
                    .bind(scope.execution_id).fetch_all(&mut *tx).await?;
                if rows.len() > 1 {
                    return Err(denied());
                }
                serde_json::to_value(rows.into_iter().next())?
            }
            InspectionOperation::Protected { identity } => {
                let file = access.verify(scope, &identity)?;
                serde_json::to_value(sqlx::query_as::<_, Protected>("SELECT EXISTS(SELECT 1 FROM artifact_file_body WHERE artifact_version=$1) AS body,EXISTS(SELECT 1 FROM file_evidence_put WHERE owner=$1) AS unsettled")
                    .bind(file.version_id).fetch_one(&mut *tx).await?)?
            }
            InspectionOperation::Ledger { identity } => {
                let file = access.verify(scope, &identity)?;
                let query = format!("SELECT {} FROM object_maintenance_ledger WHERE object_key=$1 AND provider_version=$2 LIMIT 2", Ledger::SELECT_COLUMNS);
                let rows = sqlx::query_as::<_, Ledger>(&query)
                    .bind(&file.object_key)
                    .bind(&file.provider_version)
                    .fetch_all(&mut *tx)
                    .await?;
                if rows.len() > 1 {
                    return Err(denied());
                }
                serde_json::to_value(rows.into_iter().next())?
            }
            InspectionOperation::Released { identity } => {
                let file = access.verify(scope, &identity)?;
                serde_json::to_value(sqlx::query_as::<_, Released>("SELECT EXISTS(SELECT 1 FROM file_evidence_owner WHERE version=$1) AS owner,EXISTS(SELECT 1 FROM file_evidence_pin WHERE owner=$1) AS pins,EXISTS(SELECT 1 FROM file_evidence_put WHERE owner=$1) AS operations")
                    .bind(file.version_id).fetch_one(&mut *tx).await?)?
            }
            InspectionOperation::Legacy { version_id } => {
                if !access.legacy_version_ids.contains(&version_id) {
                    return Err(denied());
                }
                let row = sqlx::query_as::<_, Legacy>("SELECT v.artifact,v.version,o.evidence_state,o.root_read FROM artifact_version v JOIN file_evidence_owner o ON o.version=v.id WHERE v.id=$1 AND v.meta->>'f_probe_run'=$2 AND EXISTS(SELECT 1 FROM file_evidence_put p WHERE p.owner=v.id AND p.protocol='legacy_unknown')")
                    .bind(version_id).bind(&scope.run_id).fetch_optional(&mut *tx).await?;
                if let Some(row) = &row {
                    let mut original = ObservedLegacyIdentity {
                        version_id,
                        artifact: row.artifact,
                        version: row.version,
                        root_read: row.root_read,
                        mac: String::new(),
                    };
                    original.mac = access
                        .legacy_mac(scope, &original)?
                        .finalize()
                        .into_bytes()
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect();
                    legacy_identity = Some(original);
                }
                serde_json::to_value(row)?
            }
            InspectionOperation::LegacyEvidence { identity } => {
                access.verify_legacy(scope, &identity)?;
                let state = sqlx::query_as::<_, LegacyEvidence>("SELECT EXISTS(SELECT 1 FROM file_evidence_owner WHERE version=$1) AS owner,(SELECT root_read FROM file_evidence_owner WHERE version=$1) AS root_read,(SELECT count(*) FROM file_evidence_pin WHERE owner=$1) AS pins,(SELECT count(*) FROM file_evidence_put WHERE owner=$1) AS operations,(SELECT count(*) FROM file_evidence_put WHERE owner=$1 AND protocol='legacy_unknown') AS legacy_operations")
                    .bind(identity.version_id).fetch_one(&mut *tx).await?;
                if state
                    .root_read
                    .is_some_and(|root| root != identity.root_read)
                {
                    return Err(denied());
                }
                serde_json::to_value(state)?
            }
        };
        tx.commit().await?;
        Ok(InspectionResponse {
            protocol: 1,
            data,
            identity,
            legacy_identity,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access() -> InspectionAccess {
        InspectionAccess {
            run_id: "owned-run".into(),
            project: "eaccept-owned-run-shared".into(),
            invoker_identity: 7,
            legacy_version_ids: vec![91],
            identity_key: "test-only-inspection-key-at-least-32-bytes".into(),
        }
    }
    fn scope() -> InspectionScope {
        InspectionScope {
            run_id: "owned-run".into(),
            project: "eaccept-owned-run-shared".into(),
            execution_id: 11,
        }
    }
    fn file() -> FileIdentity {
        FileIdentity {
            version_id: 21,
            artifact: 22,
            execution: 11,
            object_key: "owned/exact".into(),
            provider_version: "v:exact".into(),
            staging_path: "staged/exact".into(),
        }
    }
    #[test]
    fn exact_identity_receipt_rejects_cross_scope_and_provider_substitution() {
        let access = access();
        let scope = scope();
        let signed = access.sign(&scope, file()).unwrap();
        access.verify(&scope, &signed).unwrap();
        let mut wrong = signed.clone();
        wrong.file.provider_version = "v:other".into();
        assert!(access.verify(&scope, &wrong).is_err());
        wrong = signed;
        wrong.file.version_id += 1;
        assert!(access.verify(&scope, &wrong).is_err());
        let mut foreign = scope.clone();
        foreign.execution_id += 1;
        assert!(access.verify(&foreign, &wrong).is_err());
        foreign.project = "eaccept-other-shared".into();
        assert!(access.validate_scope(&foreign).is_err());
    }
    #[test]
    fn protocol_has_no_sql_or_secret_projection_escape() {
        assert!(serde_json::from_value::<InspectionRequest>(serde_json::json!({"scope":scope(),"operation":{"kind":"sql","query":"SELECT anything"}})).is_err());
        assert!(serde_json::from_value::<InspectionRequest>(serde_json::json!({"scope":scope(),"operation":{"kind":"pins","query":"SELECT anything"}})).is_err());
        assert!(serde_json::from_value::<InspectionRequest>(
            serde_json::json!({"scope":scope(),"operation":{"kind":"pins"},"include_secrets":true})
        )
        .is_err());
        let mut invalid = file();
        invalid.object_key = "x".repeat(MAX_TEXT + 1);
        assert!(access().sign(&scope(), invalid).is_err());
    }

    #[test]
    fn exact_storage_identity_rejects_traversal_and_invalid_provider_generation() {
        for path in ["/outside", "../outside", "owned//other", "owned/../other"] {
            let mut invalid = file();
            invalid.staging_path = path.into();
            assert!(access().sign(&scope(), invalid).is_err());
        }
        let mut invalid = file();
        invalid.provider_version = "v:null".into();
        assert!(access().sign(&scope(), invalid).is_err());
    }

    #[test]
    fn legacy_evidence_identity_is_issued_before_metadata_gc_and_never_rebound() {
        let access = access();
        let scope = scope();
        let mut original = ObservedLegacyIdentity {
            version_id: 91,
            artifact: 22,
            version: 1,
            root_read: 71,
            mac: String::new(),
        };
        original.mac = access
            .legacy_mac(&scope, &original)
            .unwrap()
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        access.verify_legacy(&scope, &original).unwrap();
        let mut changed = original.clone();
        changed.root_read += 1;
        assert!(access.verify_legacy(&scope, &changed).is_err());
        changed = original.clone();
        changed.version_id += 1;
        assert!(access.verify_legacy(&scope, &changed).is_err());
        let mut foreign = scope.clone();
        foreign.execution_id += 1;
        assert!(access.verify_legacy(&foreign, &original).is_err());
    }
}
