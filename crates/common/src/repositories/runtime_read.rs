//! Admission-owned immutable SDK read evidence. Close before producing final
//! output, so an in-flight read either commits before closure or delivers nothing.
use super::runtime_authority::ValidatedRuntimeAuthority;
use crate::{
    models::Id,
    secret_provenance::{SecretOrigin, SecretProvenance},
    value_schema::digest,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use sqlx::{Executor, PgConnection, Postgres};

pub const ENTITY_RUNTIME_READ: &str = "execution_runtime_read";
pub const MAX_READ_ORIGINS: usize = 128;
pub const MAX_READ_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeReadTarget {
    ExecutionMetadata {
        execution: Id,
    },
    Execution {
        execution: Id,
    },
    ArtifactVersion {
        artifact: Id,
        version: Id,
        execution: Option<Id>,
        owner: RuntimeReadOwner,
        encrypted_manifest: Option<String>,
    },
    Artifact {
        artifact: Id,
        owner: RuntimeReadOwner,
        executions: Vec<Id>,
    },
    /// One immutable acquired origin-set revision, not a mutable live permit.
    /// Frames are authenticated separately; only final seal proof permits Done.
    LiveLog {
        artifact: Id,
        version: Id,
        execution: Id,
        stream: Id,
        owner: RuntimeReadOwner,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeReadOwner {
    managing_pack: Option<Id>,
    pack: Option<Id>,
    owner: Option<Id>,
    scope: Option<crate::models::OwnerType>,
    owner_ref: Option<String>,
    owner_identity: Option<Id>,
    reference: Option<String>,
}

impl RuntimeReadOwner {
    pub(crate) fn related_wait_reference(
        &self,
        target: &crate::runtime_authority::BoundRuntimeTarget,
    ) -> Result<Option<&str>> {
        if !matches!(target.kind.as_str(), "inquiry" | "work_queue_item")
            || target.id <= 0
            || target.owner_type.is_some()
            || target.owner_id.is_some()
            || self.pack != target.pack_id
            || self.managing_pack != target.pack_id
            || self.owner.is_some()
            || self.scope.is_some()
            || self.owner_ref.is_some()
            || self.owner_identity.is_some()
            || target.kind == "inquiry" && self.reference.is_some()
            || target.kind == "work_queue_item"
                && self
                    .reference
                    .as_ref()
                    .is_none_or(|reference| reference.is_empty())
        {
            return Err(denied());
        }
        Ok(self.reference.as_deref())
    }

    pub(crate) fn execution_reference(
        &self,
        target: &crate::runtime_authority::BoundRuntimeTarget,
        action_ref: &str,
    ) -> Result<&str> {
        if target.kind != "execution"
            || target.id <= 0
            || target.pack_id.is_none()
            || target.owner_type.is_some()
            || target.owner_id.is_some()
            || self.pack != target.pack_id
            || self.managing_pack != target.pack_id
            || self.owner.is_some()
            || self.scope.is_some()
            || self.owner_ref.is_some()
            || self.owner_identity.is_some()
            || self.reference.as_deref() != Some(action_ref)
        {
            return Err(denied());
        }
        self.reference.as_deref().ok_or_else(denied)
    }

    pub fn of(target: &super::runtime_target::ResolvedRuntimeTarget) -> Self {
        let context = target.context();
        Self {
            managing_pack: context.managing_pack_id,
            pack: target.pack_id(),
            owner: target.owner_id(),
            scope: context.owner_type,
            owner_ref: context.owner_ref.clone(),
            owner_identity: context.owner_identity_id,
            reference: context.target_ref.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeReadOrigin {
    pub target: RuntimeReadTarget,
    pub provenance: Vec<SecretProvenance>,
    pub parents: Vec<RuntimeReadParent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeReadParent {
    pub entity_type: String,
    pub entity_id: Id,
    pub path: String,
    pub provenance: SecretProvenance,
}

pub struct RuntimeReadRepository;

/// Authenticated file-output source capture, not an SDK read or permission to
/// read its target. Only the exact admitted producer may replay its input rows
/// as deliveries. Other readers must use ordinary target authorization.
pub struct FileSinkCapture {
    binding: super::runtime_authority::RuntimeCredentialBinding,
    rows: Vec<super::execution_secret_value::SecretProvenanceRow>,
    bytes: usize,
}

impl FileSinkCapture {
    pub fn is_for(&self, binding: &super::runtime_authority::RuntimeCredentialBinding) -> bool {
        self.binding.execution_id == binding.execution_id
            && self.binding.admission_id == binding.admission_id
            && self.binding.sponsor_identity_id == binding.sponsor_identity_id
            && self.binding.attempt == binding.attempt
            && self.binding.action_ref == binding.action_ref
    }

    pub fn rows(&self) -> &[super::execution_secret_value::SecretProvenanceRow] {
        &self.rows
    }

    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapturedSourceRow {
    entity_type: String,
    entity_id: Id,
    json_path: String,
    source_kind: String,
    source_ref: Option<String>,
}

fn denied() -> Error {
    Error::PermissionDenied("Runtime read evidence is unavailable or exceeds its bound".into())
}

impl RuntimeReadOrigin {
    pub fn validate(&self) -> Result<()> {
        let parents = self
            .parents
            .iter()
            .map(|parent| {
                (
                    parent.entity_type.as_str(),
                    parent.entity_id,
                    parent.path.as_str(),
                )
            })
            .collect::<std::collections::BTreeSet<_>>();
        if parents.len() != self.parents.len() {
            return Err(denied());
        }
        for provenance in self
            .provenance
            .iter()
            .chain(self.parents.iter().map(|parent| &parent.provenance))
        {
            for origin in &provenance.origins {
                if let SecretOrigin::Entity {
                    entity_type,
                    entity_id,
                    path,
                } = origin
                {
                    if entity_type != ENTITY_RUNTIME_READ
                        && !parents.contains(&(entity_type.as_str(), *entity_id, path.as_str()))
                    {
                        return Err(denied());
                    }
                }
            }
        }
        let valid_target = match &self.target {
            RuntimeReadTarget::Execution { execution }
            | RuntimeReadTarget::ExecutionMetadata { execution } => *execution > 0,
            RuntimeReadTarget::LiveLog {
                artifact,
                version,
                execution,
                stream,
                ..
            } => *artifact > 0 && *version > 0 && *execution > 0 && *stream > 0,
            RuntimeReadTarget::ArtifactVersion {
                artifact,
                version,
                execution,
                ..
            } => *artifact > 0 && *version > 0 && execution.is_none_or(|id| id > 0),
            RuntimeReadTarget::Artifact {
                artifact,
                executions,
                ..
            } => *artifact > 0 && executions.len() <= 256 && executions.iter().all(|id| *id > 0),
        };
        if !valid_target
            || self.parents.len() > 256
            || self
                .parents
                .iter()
                .any(|parent| parent.entity_id <= 0 || parent.provenance.origins.is_empty())
            || self.provenance.len() > MAX_READ_ORIGINS
            || self
                .provenance
                .iter()
                .map(|parent| parent.origins.len())
                .sum::<usize>()
                > 256
            || self
                .provenance
                .iter()
                .any(|p| p.origins.is_empty() || p.origins.len() > MAX_READ_ORIGINS)
            || serde_json::to_vec(self)?.len() > MAX_READ_BYTES
        {
            return Err(denied());
        }
        Ok(())
    }
}

impl RuntimeReadRepository {
    /// Temporary native-consumer barrier. Historical sources may be closed,
    /// but their origin set must remain locked and contain no generic reads
    /// until native input/output closure propagation is implemented.
    pub async fn require_no_generic_reads(
        connection: &mut PgConnection,
        admission: Id,
    ) -> Result<()> {
        sqlx::query("INSERT INTO execution_runtime_read_fence (admission) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(admission).execute(&mut *connection).await?;
        let _: bool = sqlx::query_scalar(
            "SELECT closed FROM execution_runtime_read_fence WHERE admission = $1 FOR SHARE",
        )
        .bind(admission)
        .fetch_one(&mut *connection)
        .await?;
        let tainted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM execution_runtime_read_origin WHERE admission = $1)",
        )
        .bind(admission)
        .fetch_one(connection)
        .await?;
        if tainted {
            return Err(denied());
        }
        Ok(())
    }

    /// Prepare before consumer target locks. The registry discriminator and F's
    /// retained source digest authenticate the capture; arbitrary RuntimeRead
    /// JSON, a matching version number or a bare receipt ID cannot create it.
    pub async fn file_sink_capture(
        connection: &mut PgConnection,
        read_id: Id,
    ) -> Result<Option<FileSinkCapture>> {
        let sink: Option<(Option<Id>, Id)> = sqlx::query_as(
            "SELECT file_sink_version,admission FROM execution_runtime_read_origin WHERE id=$1",
        )
        .bind(read_id)
        .fetch_optional(&mut *connection)
        .await?;
        let Some((Some(version), registry_admission)) = sink else {
            return Ok(None);
        };
        let saved = super::file_origin::FileOriginRepository::find(connection, version)
            .await?
            .ok_or_else(denied)?;
        let origin = Self::find(&mut *connection, read_id).await?;
        if saved.origin_read != read_id
            || saved.artifact_version != version
            || saved.admission != registry_admission
            || !matches!(&origin.target, RuntimeReadTarget::ArtifactVersion { version: target, execution: Some(execution), .. } if *target == version && *execution == saved.execution)
            || digest(&serde_json::json!({"origin": &origin, "receipts": &saved.receipts}))
                .map_err(|_| denied())?
                != saved.source_digest
        {
            return Err(denied());
        }
        let bytes = serde_json::to_vec(&saved.receipts)?.len();
        if bytes > MAX_READ_BYTES {
            return Err(denied());
        }
        let rows: Vec<CapturedSourceRow> =
            serde_json::from_value(saved.receipts).map_err(|_| denied())?;
        if rows.len() > 256 {
            return Err(denied());
        }
        let admission =
            super::runtime_authority::RuntimeAuthorityRepository::require_retained_admission(
                connection,
                saved.admission,
            )
            .await?;
        if admission.owner_kind != "execution"
            || admission.owner_id != saved.execution
            || admission.authority.owner.sponsor_identity_id != saved.sponsor
            || admission.authority.attempt != saved.attempt
        {
            return Err(denied());
        }
        let binding = super::runtime_authority::RuntimeCredentialBinding {
            execution_id: saved.execution,
            admission_id: saved.admission,
            sponsor_identity_id: saved.sponsor,
            attempt: saved.attempt,
            action_ref: admission.authority.binding.action_ref().to_owned(),
        };
        let rows = rows
            .into_iter()
            .map(|row| super::execution_secret_value::SecretProvenanceRow {
                entity_type: row.entity_type,
                entity_id: row.entity_id,
                json_path: row.json_path,
                source_kind: row.source_kind,
                source_ref: row.source_ref,
            })
            .collect::<Vec<_>>();
        if super::immutable_input_origin::snapshot_input_rows(&rows)? != origin.provenance {
            return Err(denied());
        }
        Ok(Some(FileSinkCapture {
            binding,
            bytes,
            rows,
        }))
    }

    /// Sinks without durable origin propagation must reject tainted writes.
    /// Share the capture fence through the effect commit, so a racing read
    /// cannot introduce plaintext after this check and before publication.
    pub async fn require_untainted(connection: &mut PgConnection, admission: Id) -> Result<()> {
        Self::lock_open(connection, admission).await?;
        let tainted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM execution_runtime_read_origin WHERE admission = $1 AND file_sink_version IS NULL)",
        )
        .bind(admission)
        .fetch_one(connection)
        .await?;
        if tainted {
            return Err(denied());
        }
        Ok(())
    }

    pub async fn lock_open(connection: &mut PgConnection, admission: Id) -> Result<()> {
        sqlx::query("INSERT INTO execution_runtime_read_fence (admission) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(admission).execute(&mut *connection).await?;
        let closed: bool = sqlx::query_scalar(
            "SELECT closed FROM execution_runtime_read_fence WHERE admission = $1 FOR UPDATE",
        )
        .bind(admission)
        .fetch_one(connection)
        .await?;
        if closed {
            return Err(denied());
        }
        Ok(())
    }

    /// Call under graph selection, before source/target row acquisition. Lock all
    /// involved capture rows in ID order, including reciprocal live readers.
    /// The producer may be closed; the acquiring consumer must remain open.
    pub async fn lock_log_frontier(
        connection: &mut PgConnection,
        consumer: Id,
        execution: Id,
    ) -> Result<Option<Id>> {
        let source = Self::source_admission(&mut *connection, execution).await?;
        let mut ids = vec![consumer];
        ids.extend(source);
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            sqlx::query("INSERT INTO execution_runtime_read_fence(admission) VALUES ($1) ON CONFLICT DO NOTHING")
                .bind(id).execute(&mut *connection).await?;
            let closed: bool = sqlx::query_scalar(
                "SELECT closed FROM execution_runtime_read_fence WHERE admission=$1 FOR UPDATE",
            )
            .bind(id)
            .fetch_one(&mut *connection)
            .await?;
            if id == consumer && closed {
                return Err(denied());
            }
        }
        Ok(source)
    }

    pub async fn source_admission<'e, E: Executor<'e, Database = Postgres>>(
        executor: E,
        execution: Id,
    ) -> Result<Option<Id>> {
        sqlx::query_scalar("SELECT id FROM runtime_authority_admission WHERE owner_kind='execution' AND owner_id=$1")
            .bind(execution).fetch_optional(executor).await.map_err(Into::into)
    }

    /// A self-tail's earlier revisions contain only a subset of the same
    /// monotone acquisition set. Exclude those carriers, not their underlying
    /// Key/input/other-stream receipts, when taking the next self-tail snapshot.
    pub async fn self_log_carriers(
        connection: &mut PgConnection,
        execution: Id,
        stream: Id,
    ) -> Result<std::collections::BTreeSet<Id>> {
        let ids: Vec<Id> = sqlx::query_scalar("SELECT o.id FROM execution_runtime_read_origin o JOIN runtime_authority_admission a ON a.id=o.admission WHERE a.owner_kind='execution' AND a.owner_id=$1 AND o.file_sink_version IS NULL AND o.origin->'target'->>'kind'='live_log' AND o.origin->'target'->>'execution'=$1::text AND o.origin->'target'->>'stream'=$2::text ORDER BY o.id LIMIT 129")
            .bind(execution).bind(stream).fetch_all(connection).await?;
        if ids.len() > MAX_READ_ORIGINS {
            return Err(denied());
        }
        Ok(ids.into_iter().collect())
    }

    /// Structural check only. No read/decrypt authority is inferred from a live
    /// stream or a Ready body. Callers must authorize the actual targets too.
    pub async fn validate_log_target(
        connection: &mut PgConnection,
        artifact: Id,
        version: Id,
        execution: Id,
        stream: Id,
    ) -> Result<()> {
        let row =
            super::artifact::ArtifactVersionRepository::find_pinned_for_share(connection, version)
                .await?
                .ok_or_else(denied)?;
        let log = super::log_stream::LogStreamRepository::find_by_id(connection, stream).await?;
        if row.artifact != artifact
            || row.execution != Some(execution)
            || log.artifact_version != version
            || !matches!(
                row.body_state,
                Some(
                    crate::models::ArtifactBodyState::Pending
                        | crate::models::ArtifactBodyState::Ready
                )
            )
            || row
                .meta
                .as_ref()
                .and_then(|meta| meta.get("log_encoding"))
                .and_then(serde_json::Value::as_str)
                != Some(crate::runtime_read_log::ENCODING)
        {
            return Err(denied());
        }
        Ok(())
    }

    pub async fn close_execution(
        connection: &mut PgConnection,
        execution: Id,
    ) -> Result<Vec<SecretOrigin>> {
        let admission: Option<Id> = sqlx::query_scalar("SELECT id FROM runtime_authority_admission WHERE owner_kind = 'execution' AND owner_id = $1")
            .bind(execution).fetch_optional(&mut *connection).await?;
        let Some(admission) = admission else {
            return Ok(Vec::new());
        };
        sqlx::query("INSERT INTO execution_runtime_read_fence (admission,closed) VALUES ($1,TRUE) ON CONFLICT (admission) DO UPDATE SET closed = TRUE")
            .bind(admission).execute(&mut *connection).await?;
        Ok(Self::references_for_executions(connection, &[execution])
            .await?
            .remove(&execution)
            .unwrap_or_default())
    }

    /// Caller retains source, target and exact version fences through commit.
    pub async fn record(
        connection: &mut PgConnection,
        authority: &ValidatedRuntimeAuthority,
        origin: RuntimeReadOrigin,
    ) -> Result<SecretOrigin> {
        origin.validate()?;
        let admission = authority.credential_binding().admission_id;
        let body = serde_json::to_value(&origin)?;
        let hash = digest(&body).map_err(|_| denied())?;
        Self::lock_open(connection, admission).await?;
        let existing: Option<Id> = sqlx::query_scalar("SELECT id FROM execution_runtime_read_origin WHERE admission = $1 AND origin_digest = $2 AND file_sink_version IS NULL")
            .bind(admission).bind(&hash).fetch_optional(&mut *connection).await?;
        if let Some(id) = existing {
            authority.ensure_fresh_on(connection).await?;
            return Ok(Self::reference(id));
        }
        let (count, bytes): (i64, i64) = sqlx::query_as("SELECT count(*),COALESCE(sum(octet_length(origin::TEXT)),0)::BIGINT FROM execution_runtime_read_origin WHERE admission = $1 AND file_sink_version IS NULL")
            .bind(admission).fetch_one(&mut *connection).await?;
        let size: i64 = sqlx::query_scalar("SELECT octet_length($1::JSONB::TEXT)::BIGINT")
            .bind(&body)
            .fetch_one(&mut *connection)
            .await?;
        if count >= MAX_READ_ORIGINS as i64
            || bytes
                .checked_add(size)
                .is_none_or(|n| n > MAX_READ_BYTES as i64)
        {
            return Err(denied());
        }
        let id: Id = sqlx::query_scalar("INSERT INTO execution_runtime_read_origin (admission,origin_digest,origin) VALUES ($1,$2,$3) RETURNING id")
            .bind(admission).bind(hash).bind(body).fetch_one(&mut *connection).await?;
        authority.ensure_fresh_on(connection).await?;
        Ok(Self::reference(id))
    }

    pub fn reference(id: Id) -> SecretOrigin {
        SecretOrigin::Entity {
            entity_type: ENTITY_RUNTIME_READ.into(),
            entity_id: id,
            path: String::new(),
        }
    }

    pub async fn find<'e, E: Executor<'e, Database = Postgres>>(
        executor: E,
        id: Id,
    ) -> Result<RuntimeReadOrigin> {
        let (body, hash): (serde_json::Value, String) = sqlx::query_as(
            "SELECT origin,origin_digest FROM execution_runtime_read_origin WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(executor)
        .await?
        .ok_or_else(denied)?;
        if digest(&body).map_err(|_| denied())? != hash {
            return Err(denied());
        }
        let origin: RuntimeReadOrigin = serde_json::from_value(body).map_err(|_| denied())?;
        origin.validate()?;
        Ok(origin)
    }

    pub async fn references_for_executions<'e, E: Executor<'e, Database = Postgres>>(
        executor: E,
        executions: &[Id],
    ) -> Result<std::collections::BTreeMap<Id, Vec<SecretOrigin>>> {
        if executions.len() > 256 {
            return Err(denied());
        }
        let rows: Vec<(Id, Id)> = sqlx::query_as("SELECT a.owner_id,o.id FROM execution_runtime_read_origin o JOIN runtime_authority_admission a ON a.id = o.admission WHERE a.owner_kind = 'execution' AND a.owner_id = ANY($1) AND o.file_sink_version IS NULL ORDER BY a.owner_id,o.id LIMIT 32769")
            .bind(executions).fetch_all(executor).await?;
        if rows.len() > 32768 {
            return Err(denied());
        }
        let mut result = std::collections::BTreeMap::<Id, Vec<SecretOrigin>>::new();
        for (execution, id) in rows {
            let origins = result.entry(execution).or_default();
            if origins.len() >= MAX_READ_ORIGINS {
                return Err(denied());
            }
            origins.push(Self::reference(id));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_capture_role_requires_every_credential_binding_field() {
        let binding = super::super::runtime_authority::RuntimeCredentialBinding {
            execution_id: 3,
            admission_id: 4,
            sponsor_identity_id: 5,
            action_ref: "test.action".into(),
            attempt: uuid::Uuid::new_v4(),
        };
        let capture = FileSinkCapture {
            binding: binding.clone(),
            rows: vec![],
            bytes: 0,
        };
        assert!(capture.is_for(&binding));
        for field in 0..5 {
            let mut other = binding.clone();
            match field {
                0 => other.execution_id += 1,
                1 => other.admission_id += 1,
                2 => other.sponsor_identity_id += 1,
                3 => other.action_ref = "other.action".into(),
                _ => other.attempt = uuid::Uuid::new_v4(),
            }
            assert!(!capture.is_for(&other));
        }
        // Captured rows contain origins only, not arbitrary value/ciphertext
        // fields supplied as an alternate native/read evidence envelope.
        assert!(
            serde_json::from_value::<CapturedSourceRow>(serde_json::json!({
                "entity_type":"execution_config", "entity_id":3, "json_path":"/input",
                "source_kind":"provenance", "source_ref":null, "value":"not evidence"
            }))
            .is_err()
        );
    }

    #[test]
    fn rejects_unknown_target_and_invalid_ids_and_bounds() {
        assert!(serde_json::from_value::<RuntimeReadOrigin>(
            serde_json::json!({"target":{"kind":"unknown"},"provenance":[]})
        )
        .is_err());
        assert!(RuntimeReadOrigin {
            target: RuntimeReadTarget::Execution { execution: 0 },
            provenance: vec![],
            parents: vec![],
        }
        .validate()
        .is_err());
        assert!(RuntimeReadOrigin {
            target: RuntimeReadTarget::Execution { execution: 1 },
            provenance: vec![SecretProvenance::default()],
            parents: vec![],
        }
        .validate()
        .is_err());
    }

    #[test]
    fn every_mutable_parent_must_have_a_unique_bounded_snapshot() {
        let parent = SecretOrigin::Entity {
            entity_type: crate::secret_values::ENTITY_EXECUTION_CONFIG.into(),
            entity_id: 3,
            path: "/input".into(),
        };
        let mut read = RuntimeReadOrigin {
            target: RuntimeReadTarget::Execution { execution: 3 },
            provenance: vec![SecretProvenance {
                origins: vec![parent],
                templates: vec![],
            }],
            parents: vec![],
        };
        assert!(read.validate().is_err());
        read.parents.push(RuntimeReadParent {
            entity_type: crate::secret_values::ENTITY_EXECUTION_CONFIG.into(),
            entity_id: 3,
            path: "/input".into(),
            provenance: SecretProvenance {
                origins: vec![SecretOrigin::Local {
                    source_kind: "parameter_schema".into(),
                    source_ref: None,
                }],
                templates: vec![],
            },
        });
        assert!(read.validate().is_ok());
        read.parents.push(read.parents[0].clone());
        assert!(read.validate().is_err());
        read.parents.pop();
        read.parents[0]
            .provenance
            .templates
            .push(crate::secret_provenance::TemplateOrigin {
                component_type: "action".into(),
                component_ref: "p.run".into(),
                input_path: "/input".into(),
                expression: "x".repeat(MAX_READ_BYTES),
            });
        assert!(read.validate().is_err());
    }
}
