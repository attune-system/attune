//! Physical-cleanup-owned retention of frozen file disclosure evidence.
//! Pins preserve rows, never validity, grants or process-token liveness.
use super::{
    runtime_read::{RuntimeReadOrigin, RuntimeReadRepository, RuntimeReadTarget},
    schema_type::SchemaTypeRepository,
};
use crate::{
    secret_provenance::{SecretOrigin, SecretProvenance},
    Error, Result,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgConnection;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub struct FileEvidenceRetentionRepository;

#[cfg(test)]
#[path = "file_evidence_retention_prospective_tests.rs"]
mod prospective_tests;

#[cfg(test)]
mod purpose_retention_tests {
    use super::*;
    #[test]
    fn only_nine_exact_purpose_kinds_resolve_as_receipts_and_proof_rows_share_the_budget() {
        let mut closure = Closure {
            queue: VecDeque::new(),
            seen: BTreeSet::new(),
            bytes: 0,
            proof_rows: 0,
            ..Closure::default()
        };
        for kind in ["execution_config", "execution_result", "workflow_variables"] {
            for purpose in ["value", "condition", "control"] {
                closure
                    .entity(&format!("declared_input:{kind}:{purpose}"), 41, 0)
                    .unwrap();
            }
        }
        assert_eq!(closure.queue.len(), 9);
        assert!(closure
            .queue
            .iter()
            .all(|(node, _)| node.kind == "runtime_input_receipt" && node.id == 41));
        for unsupported in [
            "declared_input",
            "declared_input:execution_config",
            "declared_input:execution_config:api",
            "declared_input:execution_config:value:extra",
            "declared_input:event_payload:value",
            "accepted_input",
            "native_input_snapshot",
            "runtime_native_input_snapshot",
        ] {
            assert!(closure.entity(unsupported, 41, 0).is_err());
        }
        closure.rows(MAX_NODES).unwrap();
        assert!(closure.rows(1).is_err());
    }
    #[test]
    fn authenticated_receipt_and_archive_bytes_share_the_same_total_bound() {
        let mut closure = Closure {
            queue: VecDeque::new(),
            seen: BTreeSet::new(),
            bytes: 0,
            proof_rows: 0,
            ..Closure::default()
        };
        closure.charge_bytes(MAX_BYTES - 2).unwrap();
        closure.charge(&serde_json::json!(0)).unwrap();
        closure.charge_bytes(1).unwrap();
        assert!(closure.charge_bytes(1).is_err());
        closure.bytes = usize::MAX;
        assert!(
            closure.charge_bytes(1).is_err(),
            "opaque byte charges must use checked arithmetic"
        );
    }
    #[test]
    fn native_snapshot_selections_require_the_authenticated_storage_walker() {
        for purpose in [
            "authored",
            "inherited",
            "public_parameter",
            "native_outcome",
            "control",
            "system",
            "iteration",
            "native_snapshot",
        ] {
            assert!(retained_selection_supported(
                &serde_json::json!({"source":{"purpose":purpose}})
            ));
        }
        for purpose in ["unknown_future_source", ""] {
            assert!(!retained_selection_supported(
                &serde_json::json!({"source":{"purpose":purpose}})
            ));
        }
        assert!(!retained_selection_supported(&serde_json::json!({})));
    }
    #[test]
    fn native_bytes_are_charged_once_across_receipts_and_standalone_visits() {
        let mut closure = Closure::default();
        closure.native_bytes.insert(7, 800);
        closure.native_predecessors.insert(7, None);
        closure.charge_receipt(1000, &[(7, 800)]).unwrap();
        closure.charge_receipt(1200, &[(7, 800)]).unwrap();
        assert_eq!(closure.bytes, 1400);
        assert!(!closure.charged_native.insert(7));
        closure.charge_bytes(10).unwrap(); // additional physical-parent material
        assert_eq!(closure.bytes, 1410);
        assert!(closure.charge_receipt(799, &[(7, 800)]).is_err());
        let mut standalone = Closure::default();
        standalone.native_bytes.insert(7, 800);
        standalone.native_predecessors.insert(7, None);
        standalone.charged_native.insert(7);
        standalone.charge_bytes(800).unwrap();
        standalone.charge_receipt(1000, &[(7, 800)]).unwrap();
        assert_eq!(standalone.bytes, 1000);
        assert!(Closure::default()
            .charge_receipt(1600, &[(7, 800), (7, 800)])
            .is_err());
    }
    #[test]
    fn typed_cache_entries_and_events_keep_both_identity_coordinates() {
        use super::super::native_input_snapshot::NativeStorageParent as P;
        let first = FileEvidenceRetentionRepository::native_parent(&P::CacheEntry {
            generation: 11,
            entry: 31,
        })
        .unwrap();
        let twin = FileEvidenceRetentionRepository::native_parent(&P::CacheEntry {
            generation: 12,
            entry: 31,
        })
        .unwrap();
        assert_ne!(first, twin);
        let now = Utc::now();
        let first = FileEvidenceRetentionRepository::native_parent(&P::Event {
            event: 31,
            created: now,
        })
        .unwrap();
        let twin = FileEvidenceRetentionRepository::native_parent(&P::Event {
            event: 31,
            created: now + chrono::Duration::seconds(1),
        })
        .unwrap();
        assert_ne!(first, twin);
        assert!(
            FileEvidenceRetentionRepository::native_parent(&P::CacheEntry {
                generation: 0,
                entry: 31
            })
            .is_err()
        );
    }
    #[test]
    fn only_a_matching_archived_original_site_replaces_a_raw_source_pin() {
        let source = |kind: &str, id, path: &str| SecretOrigin::Entity {
            entity_type: kind.into(),
            entity_id: id,
            path: path.into(),
        };
        let archived = source("execution_result", 9, "");
        assert!(Closure::archived_site_covers(
            &archived,
            &source("execution_result", 9, "/token")
        ));
        assert!(!Closure::archived_site_covers(
            &archived,
            &source("execution_config", 9, "/token")
        ));
        assert!(!Closure::archived_site_covers(
            &archived,
            &source("execution_result", 10, "/token")
        ));
        assert!(!Closure::archived_site_covers(
            &source("execution_result", 9, "/a"),
            &source("execution_result", 9, "/ab")
        ));
        let mut closure = Closure::default();
        closure
            .origins(
                &SecretProvenance {
                    origins: vec![archived.clone()],
                    templates: vec![],
                },
                0,
            )
            .unwrap();
        closure.archived_sites.push(archived.clone());
        assert!(closure.pop().unwrap().is_none());
        let mut raw = Closure::default();
        raw.origins(
            &SecretProvenance {
                origins: vec![archived],
                templates: vec![],
            },
            0,
        )
        .unwrap();
        assert_eq!(raw.pop().unwrap().unwrap().0, Node::new("execution", 9));
    }
    #[test]
    fn completion_children_keep_independent_contract_proofs_and_original_envelope_sites() {
        let mut closure = Closure::default();
        for (child, revision) in [(19, 91), (20, 92)] {
            closure
                .origins(
                    &SecretProvenance {
                        origins: vec![SecretOrigin::Entity {
                            entity_type: "execution_result".into(),
                            entity_id: child,
                            path: "/data/token".into(),
                        }],
                        templates: vec![],
                    },
                    0,
                )
                .unwrap();
            closure.archived_sites.push(SecretOrigin::Entity {
                entity_type: "execution_result".into(),
                entity_id: child,
                path: String::new(),
            });
            closure
                .proof(
                    &serde_json::json!({"referenced_types":[{
                        "type_ref":"fixture.original", "content_digest":"original",
                        "revision_id":revision,
                    }]}),
                    0,
                )
                .unwrap();
        }
        let mut nodes = Vec::new();
        while let Some((node, _)) = closure.pop().unwrap() {
            nodes.push(node);
        }
        assert_eq!(
            nodes,
            vec![
                Node::new("schema_type_revision", 91),
                Node::new("schema_type_revision", 92)
            ]
        );
        assert!(closure.pending_sites.is_empty());
        // The aggregate namespace or one sibling cannot stand in for the other
        // child's actual original execution_result site.
        assert!(!Closure::archived_site_covers(
            &SecretOrigin::Entity {
                entity_type: "execution_result".into(),
                entity_id: 19,
                path: "/children/1".into(),
            },
            &SecretOrigin::Entity {
                entity_type: "execution_result".into(),
                entity_id: 20,
                path: "/data/token".into(),
            }
        ));
    }
    #[test]
    fn iteration_archive_origin_requires_exact_typed_authentication_without_self_recursion() {
        let origin = SecretOrigin::Entity {
            entity_type: "ordinary_iteration_outcome".into(),
            entity_id: 47,
            path: String::new(),
        };
        let provenance = SecretProvenance {
            origins: vec![origin.clone()],
            templates: vec![],
        };
        let mut closure = Closure::default();
        closure.origins(&provenance, 0).unwrap();
        assert_eq!(
            closure.pop().unwrap().unwrap().0,
            Node::new("runtime_native_input_snapshot", 47)
        );
        assert_eq!(closure.pending_iteration_origins, vec![origin.clone()]);
        assert!(closure.iteration_archive_origins.is_empty());
        // Only native_storage's authenticated typed projection can discharge it.
        closure.iteration_archive_origins.push(origin);
        closure.origins(&provenance, 0).unwrap();
        assert!(closure.pop().unwrap().is_none());
        for (kind, path) in [
            ("ordinary_iteration_outcome", "/child"),
            ("future_group_outcome", ""),
        ] {
            let mut unknown = Closure::default();
            assert!(unknown
                .origins(
                    &SecretProvenance {
                        origins: vec![SecretOrigin::Entity {
                            entity_type: kind.into(),
                            entity_id: 47,
                            path: path.into(),
                        }],
                        templates: vec![]
                    },
                    0
                )
                .is_err());
        }
        assert!(retained_selection_supported(
            &serde_json::json!({"source":{"purpose":"native_iteration_outcome"}})
        ));
        assert!(!retained_selection_supported(
            &serde_json::json!({"source":{"purpose":"future_group_outcome"}})
        ));
    }
}
#[derive(sqlx::FromRow)]
pub struct FilePutRecovery {
    version: i64,
    object_key: String,
    token: uuid::Uuid,
}
impl FilePutRecovery {
    pub fn key(&self) -> Result<crate::blob_store::ObjectKey> {
        crate::blob_store::ObjectKey::new(self.object_key.clone()).map_err(|_| denied())
    }
    pub fn token(&self) -> uuid::Uuid {
        self.token
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Node {
    kind: String,
    id: i64,
    created: Option<DateTime<Utc>>,
    generation: Option<i64>,
}
impl Node {
    fn new(kind: &str, id: i64) -> Self {
        Self {
            kind: kind.into(),
            id,
            created: None,
            generation: None,
        }
    }
}
fn denied() -> Error {
    Error::PermissionDenied("File evidence is missing or exceeds its bounded closure".into())
}
fn retained_selection_supported(selection: &Value) -> bool {
    matches!(
        selection
            .get("source")
            .and_then(|source| source.get("purpose"))
            .and_then(Value::as_str),
        Some(
            "authored"
                | "inherited"
                | "public_parameter"
                | "native_outcome"
                | "control"
                | "system"
                | "iteration"
                | "native_snapshot"
                | "native_iteration_outcome"
        )
    )
}
const MAX_NODES: usize = 256;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_NATIVE_SNAPSHOTS: usize = 128;

#[derive(Default)]
struct Closure {
    queue: VecDeque<(Node, usize)>,
    seen: BTreeSet<Node>,
    bytes: usize,
    proof_rows: usize,
    native_bytes: BTreeMap<i64, usize>,
    native_predecessors: BTreeMap<i64, Option<i64>>,
    charged_native: BTreeSet<i64>,
    cache_audit_origins: Vec<SecretOrigin>,
    pending_cache_origins: Vec<SecretOrigin>,
    iteration_archive_origins: Vec<SecretOrigin>,
    pending_iteration_origins: Vec<SecretOrigin>,
    success_metadata_origins: Vec<SecretOrigin>,
    pending_success_metadata_origins: Vec<SecretOrigin>,
    wait_completion_origins: Vec<SecretOrigin>,
    pending_wait_completion_origins: Vec<SecretOrigin>,
    native_physical: BTreeSet<Node>,
    archived_sites: Vec<SecretOrigin>,
    pending_sites: Vec<(SecretOrigin, usize)>,
    discovered_sites: Vec<SecretOrigin>,
}
impl Closure {
    fn charge_native(&mut self, snapshot: i64) -> Result<usize> {
        let mut current = Some(snapshot);
        let mut count = 0;
        let mut added = 0usize;
        while let Some(id) = current {
            count += 1;
            if id <= 0 || count > MAX_NATIVE_SNAPSHOTS {
                return Err(denied());
            }
            let total = *self.native_bytes.get(&id).ok_or_else(denied)?;
            let predecessor = *self.native_predecessors.get(&id).ok_or_else(denied)?;
            let inherited = if let Some(parent) = predecessor {
                if parent <= 0 || parent >= id {
                    return Err(denied());
                }
                *self.native_bytes.get(&parent).ok_or_else(denied)?
            } else {
                0
            };
            let own = total
                .checked_sub(inherited)
                .filter(|bytes| *bytes > 0)
                .ok_or_else(denied)?;
            if self.charged_native.insert(id) {
                added = added.checked_add(own).ok_or_else(denied)?;
            }
            current = predecessor;
        }
        Ok(added)
    }
    fn charge_receipt(&mut self, total: usize, native: &[(i64, usize)]) -> Result<()> {
        let mut duplicate_bytes = 0usize;
        let mut unique = BTreeSet::new();
        for &(snapshot, bytes) in native {
            if snapshot <= 0 || !unique.insert(snapshot) {
                return Err(denied());
            }
            if self.native_bytes.get(&snapshot) != Some(&bytes) {
                return Err(denied());
            }
            // A includes the entire predecessor chain in EACH selected root.
            // Deduct only exact row costs already covered by another root.
            let added = self.charge_native(snapshot)?;
            duplicate_bytes = duplicate_bytes
                .checked_add(bytes.checked_sub(added).ok_or_else(denied)?)
                .ok_or_else(denied)?;
        }
        self.charge_bytes(total.checked_sub(duplicate_bytes).ok_or_else(denied)?)
    }
    fn pop(&mut self) -> Result<Option<(Node, usize)>> {
        if self.queue.is_empty() {
            // Receipt discovery must finish first. Only A's exact authenticated
            // original site permits archived custody instead of a raw-row pin.
            for (origin, depth) in std::mem::take(&mut self.pending_sites) {
                if depth > 32 {
                    return Err(denied());
                }
                if self
                    .archived_sites
                    .iter()
                    .any(|site| Self::archived_site_covers(site, &origin))
                {
                    self.charge(&serde_json::to_value(&origin)?)?;
                } else if let SecretOrigin::Entity {
                    entity_type,
                    entity_id,
                    ..
                } = origin
                {
                    self.entity(&entity_type, entity_id, depth)?;
                } else {
                    return Err(denied());
                }
            }
        }
        // Discover authenticated receipt/storage inventories before their frozen
        // physical identities. No source authorization is performed here.
        let position = self.queue.iter().position(|(node, _)| {
            matches!(
                node.kind.as_str(),
                "execution_runtime_read_origin"
                    | "runtime_input_receipt"
                    | "runtime_native_input_snapshot"
            )
        });
        Ok(position
            .and_then(|index| self.queue.remove(index))
            .or_else(|| self.queue.pop_front()))
    }
    fn rows(&mut self, count: usize) -> Result<()> {
        self.proof_rows = self.proof_rows.checked_add(count).ok_or_else(denied)?;
        if self.proof_rows + self.seen.len() > MAX_NODES {
            return Err(denied());
        }
        Ok(())
    }
    fn archived_site_covers(site: &SecretOrigin, origin: &SecretOrigin) -> bool {
        match (site, origin) {
            (
                SecretOrigin::Entity {
                    entity_type: kind,
                    entity_id: id,
                    path: root,
                },
                SecretOrigin::Entity {
                    entity_type,
                    entity_id,
                    path,
                },
            ) => {
                kind == entity_type
                    && id == entity_id
                    && (root == path
                        || path
                            .strip_prefix(root)
                            .is_some_and(|tail| tail.starts_with('/')))
            }
            _ => false,
        }
    }
    fn push(&mut self, kind: &str, id: i64, depth: usize) -> Result<()> {
        if id <= 0 || depth > 32 || self.queue.len() + self.seen.len() > 2048 {
            return Err(denied());
        }
        self.queue.push_back((Node::new(kind, id), depth));
        Ok(())
    }
    fn charge(&mut self, value: &Value) -> Result<()> {
        self.charge_bytes(serde_json::to_vec(value)?.len())
    }
    fn charge_bytes(&mut self, bytes: usize) -> Result<()> {
        self.bytes = self.bytes.checked_add(bytes).ok_or_else(denied)?;
        if self.bytes > MAX_BYTES {
            return Err(denied());
        }
        Ok(())
    }
    fn origins(&mut self, p: &SecretProvenance, depth: usize) -> Result<()> {
        for origin in &p.origins {
            if let SecretOrigin::Entity {
                entity_type,
                entity_id,
                path,
            } = origin
            {
                if entity_type == "wait_completion_metadata" {
                    if *entity_id <= 0 || !path.is_empty() {
                        return Err(denied());
                    }
                    if !self.wait_completion_origins.contains(origin)
                        && !self.pending_wait_completion_origins.contains(origin)
                    {
                        self.rows(1)?;
                        self.pending_wait_completion_origins.push(origin.clone());
                        self.push("runtime_native_input_snapshot", *entity_id, depth + 1)?;
                    }
                    continue;
                }
            }
            if let SecretOrigin::Entity {
                entity_type,
                entity_id,
                path,
            } = origin
            {
                if entity_type == "ordinary_success_metadata" {
                    if *entity_id <= 0 || !path.is_empty() {
                        return Err(denied());
                    }
                    if !self.success_metadata_origins.contains(origin)
                        && !self.pending_success_metadata_origins.contains(origin)
                    {
                        self.rows(1)?;
                        self.pending_success_metadata_origins.push(origin.clone());
                        self.push("runtime_native_input_snapshot", *entity_id, depth + 1)?;
                    }
                    continue;
                }
            }
            if let SecretOrigin::Entity {
                entity_type,
                entity_id,
                path,
            } = origin
            {
                if entity_type == "ordinary_iteration_outcome" {
                    if *entity_id <= 0 || !path.is_empty() {
                        return Err(denied());
                    }
                    if !self.iteration_archive_origins.contains(origin)
                        && !self.pending_iteration_origins.contains(origin)
                    {
                        // Discovery may precede the receipt. Authenticate the
                        // actual snapshot; a kind string alone never discharges
                        // this exact lifecycle identity or creates a raw site.
                        self.rows(1)?;
                        self.pending_iteration_origins.push(origin.clone());
                        self.push("runtime_native_input_snapshot", *entity_id, depth + 1)?;
                    }
                    continue;
                }
            }
            if self.cache_audit_origins.contains(origin) {
                // Exact A-authenticated audit origin only. Its typed physical
                // inventory is already queued; never parse its encoded path.
                continue;
            }
            if matches!(origin, SecretOrigin::Entity { entity_type, .. } if entity_type == "cache_namespace")
            {
                // Discovery may visit C's immutable read before its receipt.
                // Defer this exact audit identity; only A's matching typed
                // inventory can discharge it before retention commits.
                if !self.pending_cache_origins.contains(origin) {
                    self.rows(1)?;
                    self.pending_cache_origins.push(origin.clone());
                }
                continue;
            }
            if matches!(origin, SecretOrigin::Entity { entity_type, .. } if ["execution_config", "execution_result", "workflow_variables", "inquiry_response", "queue_item_result"].contains(&entity_type.as_str()))
            {
                if !self.discovered_sites.contains(origin) {
                    self.rows(1)?;
                    self.discovered_sites.push(origin.clone());
                    self.pending_sites.push((origin.clone(), depth + 1));
                }
                continue;
            }
            if let SecretOrigin::Entity {
                entity_type,
                entity_id,
                ..
            } = origin
            {
                self.entity(entity_type, *entity_id, depth + 1)?;
            }
            if let SecretOrigin::QueueItem {
                queue_id, item_id, ..
            } = origin
            {
                self.push("work_queue", *queue_id, depth + 1)?;
                self.push("work_queue_item", *item_id, depth + 1)?;
            }
            // Key/QueueItem/etc identity is already frozen in C's provenance.
            // Never pin Key or human IAM rows: live revocation still denies.
        }
        Ok(())
    }
    fn entity(&mut self, entity: &str, id: i64, depth: usize) -> Result<()> {
        if let Some(site) = entity.strip_prefix("declared_input:") {
            let Some((kind, purpose)) = site.split_once(':') else {
                return Err(denied());
            };
            if !["execution_config", "execution_result", "workflow_variables"].contains(&kind)
                || !["value", "condition", "control"].contains(&purpose)
            {
                return Err(denied());
            }
            return self.push("runtime_input_receipt", id, depth);
        }
        let kind = match entity {
            "execution_runtime_read" => "execution_runtime_read_origin",
            "execution_config" | "execution_result" | "workflow_variables" => "execution",
            "component_configuration" => "component_configuration_revision",
            "sensor_generation_config" | "sensor_generation_rule_config" => {
                "sensor_generation_schema_binding"
            }
            "sensor_rule_delivery_config" => "sensor_rule_config_delivery",
            "inquiry_options" | "inquiry_response" => "inquiry",
            "event_source_payload" | "event_source_config" => "event_origin_snapshot",
            _ => return Err(denied()),
        };
        self.push(kind, id, depth)
    }
    fn archived_fields(
        &mut self,
        kind: &str,
        execution: i64,
        fields: &[crate::secret_values::StoredSecretValue],
        depth: usize,
    ) -> Result<()> {
        if execution <= 0
            || !["execution_config", "execution_result", "workflow_variables"].contains(&kind)
        {
            return Err(denied());
        }
        self.archived_source_fields(kind, execution, fields, depth)
    }

    fn archived_wait_fields(
        &mut self,
        wait: &super::native_input_snapshot::NativeWaitCompletionEvidence,
        depth: usize,
    ) -> Result<()> {
        let (kind, id, path) = wait.original_source_site();
        if id <= 0
            || !path.is_empty()
            || !["execution_result", "inquiry_response", "queue_item_result"].contains(&kind)
            || wait.original_metadata().source_site() != Some((kind, id, path))
        {
            return Err(denied());
        }
        self.archived_source_fields(kind, id, &wait.original_fields(), depth)
    }

    // Only callers above choose entity kinds. The Wait caller requires A's
    // authenticated typed evidence; inquiry/queue fields never become execution
    // VALUE sites or inherit the root workflow's delivery authority.
    fn archived_source_fields(
        &mut self,
        kind: &str,
        execution: i64,
        fields: &[crate::secret_values::StoredSecretValue],
        depth: usize,
    ) -> Result<()> {
        self.rows(fields.len())?;
        for field in fields {
            let site = SecretOrigin::Entity {
                entity_type: kind.into(),
                entity_id: execution,
                path: field.json_path.clone(),
            };
            if !self.archived_sites.contains(&site) {
                self.archived_sites.push(site);
            }
            let provenance = if field.source_kind == "provenance" {
                serde_json::from_str(field.source_ref.as_deref().ok_or_else(denied)?)?
            } else if field.source_kind == "parameter_schema"
                && field.source_ref.as_deref() == Some(field.json_path.as_str())
            {
                crate::secret_values::SecretSource::ParameterSchema {
                    path: field.json_path.clone(),
                }
                .provenance()
            } else {
                return Err(denied());
            };
            self.origins(&provenance, depth + 1)?;
        }
        Ok(())
    }
    fn read(&mut self, read: &RuntimeReadOrigin, depth: usize) -> Result<()> {
        match &read.target {
            RuntimeReadTarget::Execution { execution }
            | RuntimeReadTarget::ExecutionMetadata { execution } => {
                self.push("execution", *execution, depth + 1)?
            }
            RuntimeReadTarget::LiveLog {
                artifact,
                version,
                execution,
                stream,
                ..
            } => {
                self.push("artifact", *artifact, depth + 1)?;
                self.push("artifact_version", *version, depth + 1)?;
                self.push("execution", *execution, depth + 1)?;
                self.push("log_stream", *stream, depth + 1)?;
            }
            RuntimeReadTarget::ArtifactVersion {
                artifact,
                version,
                execution,
                ..
            } => {
                self.push("artifact", *artifact, depth + 1)?;
                self.push("artifact_version", *version, depth + 1)?;
                if let Some(execution) = execution {
                    self.push("execution", *execution, depth + 1)?;
                }
            }
            RuntimeReadTarget::Artifact {
                artifact,
                executions,
                ..
            } => {
                self.push("artifact", *artifact, depth + 1)?;
                for execution in executions {
                    self.push("execution", *execution, depth + 1)?;
                }
            }
        }
        for p in &read.provenance {
            self.origins(p, depth)?;
        }
        for parent in &read.parents {
            if [
                "cache_namespace",
                "execution_config",
                "execution_result",
                "workflow_variables",
                "inquiry_response",
                "queue_item_result",
            ]
            .contains(&parent.entity_type.as_str())
            {
                self.origins(
                    &SecretProvenance {
                        origins: vec![SecretOrigin::Entity {
                            entity_type: parent.entity_type.clone(),
                            entity_id: parent.entity_id,
                            path: parent.path.clone(),
                        }],
                        templates: vec![],
                    },
                    depth + 1,
                )?;
            } else {
                self.entity(&parent.entity_type, parent.entity_id, depth + 1)?;
            }
            self.origins(&parent.provenance, depth)?;
        }
        Ok(())
    }
    // Only immutable proof objects, never mutable caller metadata, reach this
    // scanner. Revisions remain pinned by their concrete IDs, not display refs.
    fn proof(&mut self, value: &Value, depth: usize) -> Result<()> {
        if depth > 32 {
            return Err(denied());
        }
        match value {
            Value::Object(fields) => {
                if fields.contains_key("type_ref") && fields.contains_key("content_digest") {
                    if let Some(id) = fields.get("revision_id").and_then(Value::as_i64) {
                        self.push("schema_type_revision", id, depth + 1)?;
                    }
                }
                for (name, value) in fields {
                    if [
                        "annotated_schema",
                        "validator_schema",
                        "param_schema",
                        "out_schema",
                        "conf_schema",
                        "schema",
                        "definition",
                    ]
                    .contains(&name.as_str())
                    {
                        continue;
                    }
                    let kind = match name.as_str() {
                        "policy_id"
                        | "policy_revision"
                        | "policy_revision_id"
                        | "dispatch_policy_revision" => Some("pack_runtime_policy_revision"),
                        "metadata_revision" | "metadata_revision_id" => {
                            Some("managed_executable_metadata_revision")
                        }
                        "approval_id" => Some("runtime_external_approval"),
                        "release_id"
                        | "target_release_id"
                        | "pack_release"
                        | "pack_release_id"
                        | "dispatch_source_release" => Some("pack_release"),
                        "pack_id" => Some("pack"),
                        "action_id" => Some("action"),
                        "sensor_id" => Some("sensor"),
                        "rule_id" => Some("rule"),
                        "execution_id" => Some("execution"),
                        "parent_admission"
                        | "parent_admission_id"
                        | "source_admission"
                        | "source_admission_id" => Some("runtime_authority_admission"),
                        _ => None,
                    };
                    if let (Some(kind), Some(id)) = (kind, value.as_i64()) {
                        self.push(kind, id, depth + 1)?;
                    }
                    if name == "release" {
                        if let Some(id) = value.get("id").and_then(Value::as_i64) {
                            self.push("pack_release", id, depth + 1)?;
                        }
                    }
                    self.proof(value, depth + 1)?;
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.proof(value, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl FileEvidenceRetentionRepository {
    /// Caller entered exclusive graph admission BEFORE source/target locks.
    /// Epoch rows are independent historical identities, not live source grants.
    pub async fn collect_cache_epochs(conn: &mut PgConnection, limit: i64) -> Result<u64> {
        let held: bool = sqlx::query_scalar("SELECT file_evidence_graph_held(true)")
            .fetch_one(&mut *conn)
            .await?;
        if !held {
            return Err(Error::invalid_state(
                "Native epoch collection requires an exclusive outer graph entry",
            ));
        }
        Ok(sqlx::query("WITH dormant AS (SELECT epoch.iteration FROM runtime_cache_iteration_epoch epoch WHERE NOT file_evidence_cache_epoch_pinned(epoch.iteration) AND NOT EXISTS(SELECT 1 FROM workflow_cache_iteration i WHERE i.id=epoch.iteration) AND NOT EXISTS(SELECT 1 FROM runtime_native_cache_frontier f WHERE f.bound_iteration=epoch.iteration) ORDER BY epoch.iteration LIMIT $1) DELETE FROM runtime_cache_iteration_epoch epoch USING dormant WHERE epoch.iteration=dormant.iteration")
            .bind(limit.clamp(1,256)).execute(conn).await?.rows_affected())
    }

    pub async fn require_retained(conn: &mut PgConnection, version: i64) -> Result<()> {
        let state: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT evidence_state,diagnostic FROM file_evidence_owner WHERE version=$1",
        )
        .bind(version)
        .fetch_optional(conn)
        .await?;
        if matches!(&state,Some((state,_)) if state=="retained") {
            return Ok(());
        }
        Err(Error::PermissionDenied(format!("File evidence readmission required for version {version}: {}",
            state.and_then(|(_,diagnostic)|diagnostic).unwrap_or_else(||"durable evidence owner missing; run bounded historical audit or clean up this version".into()))))
    }

    /// Audits a frozen historical reservation. This grants no process authority,
    /// and never substitutes current execution-wide provenance for saved parents.
    pub async fn readmit(pool: &sqlx::PgPool, version: i64) -> Result<bool> {
        let mut tx = pool.begin().await?;
        Self::lock_reclamation(&mut tx).await?;
        let candidate:Option<(i64,Value)>=sqlx::query_as("SELECT root_read,reservation FROM file_evidence_owner WHERE version=$1 AND evidence_state IN ('pending_audit','quarantined') AND NOT metadata_gone AND cleanup_group IS NULL FOR UPDATE")
            .bind(version).fetch_optional(&mut *tx).await?;
        let Some((root, reservation)) = candidate else {
            tx.rollback().await?;
            return Ok(false);
        };
        // A savepoint rolls back partial graph pins without losing the durable
        // physical owner or aborting the transaction that records quarantine.
        use sqlx::Acquire;
        let mut audit = (&mut *tx).begin().await?;
        let result=async {
            let origin=RuntimeReadRepository::find(&mut *audit,root).await?;
            let (artifact,execution)=match &origin.target {
                RuntimeReadTarget::ArtifactVersion{artifact,version:target,execution:Some(execution),..} if *target==version=>(*artifact,*execution),
                _=>return Err(denied()),
            };
            let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifact_file_origin o JOIN artifact_version v ON v.id=o.artifact_version JOIN execution_runtime_read_origin r ON r.id=o.origin_read AND r.admission=o.admission WHERE v.id=$1 AND o.origin_read=$2 AND to_jsonb(o)=$3 AND v.artifact=$4 AND v.execution=$5 AND o.execution=$5 AND (v.body_state<>'ready' OR EXISTS(SELECT 1 FROM artifact_file_body b WHERE b.artifact_version=v.id AND b.origin_read=o.origin_read AND b.ciphertext_bytes=v.size_bytes AND b.ciphertext_sha256=v.sha256)))")
                .bind(version).bind(root).bind(&reservation).bind(artifact).bind(execution).fetch_one(&mut *audit).await?;
            let digest=crate::value_schema::digest(&serde_json::json!({"origin":origin,"receipts":reservation.get("receipts").ok_or_else(denied)?}))
                .map_err(|_|denied())?;
            if !valid || reservation.get("source_digest").and_then(Value::as_str)!=Some(digest.as_str()) {return Err(denied());}
            Self::retain(&mut audit,version,root).await
        }.await;
        match result {
            Ok(()) => {
                audit.commit().await?;
                tx.commit().await?;
                Ok(true)
            }
            Err(error) => {
                audit.rollback().await?;
                // Only our located structural diagnostic is safe to persist.
                // SQL/provider error text may carry connection or object data.
                let location=match error {
                    Error::PermissionDenied(ref message) if message.starts_with("File evidence unavailable at ")=>message.clone(),
                    _=>format!("File evidence unavailable at root/{root}; saved reservation, schema, receipt digest or bounded dependency is missing or invalid"),
                };
                sqlx::query("UPDATE file_evidence_owner SET evidence_state='quarantined',diagnostic=$2 WHERE version=$1")
                    .bind(version).bind(format!("version {version}: {location}; repair the identified retained proof and retry bounded readmission, or request physical cleanup"))
                    .execute(&mut *tx).await?;
                tx.commit().await?;
                Ok(false)
            }
        }
    }

    pub async fn audit_pending(pool: &sqlx::PgPool, limit: i64) -> Result<u64> {
        let versions:Vec<i64>=sqlx::query_scalar("SELECT version FROM file_evidence_owner WHERE evidence_state='pending_audit' AND NOT metadata_gone AND cleanup_group IS NULL ORDER BY version LIMIT $1")
            .bind(limit.clamp(1,256)).fetch_all(pool).await?;
        let mut admitted = 0;
        for version in versions {
            admitted += u64::from(Self::readmit(pool, version).await?);
        }
        Ok(admitted)
    }

    pub async fn begin_put(conn: &mut PgConnection, version: i64) -> Result<Option<uuid::Uuid>> {
        let attempt = uuid::Uuid::new_v4();
        static NATIVE_INSTANCE: std::sync::OnceLock<uuid::Uuid> = std::sync::OnceLock::new();
        let native_instance = *NATIVE_INSTANCE.get_or_init(uuid::Uuid::new_v4);
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM file_evidence_owner WHERE version=$1)")
                .bind(version)
                .fetch_one(&mut *conn)
                .await?;
        if !exists {
            return Ok(None);
        }
        let inserted=sqlx::query("INSERT INTO file_evidence_put(attempt,owner,protocol,execution,execution_attempt,worker_instance,native_instance) SELECT $1,o.version,'atomic_create_v1',c.execution,c.attempt,c.worker_instance,$3 FROM file_evidence_owner o JOIN worker_delivery_permit p ON p.artifact_version=o.version JOIN worker_delivery_context c ON c.token=p.context AND c.execution=(o.reservation->>'execution')::bigint AND c.attempt=(o.reservation->>'attempt')::uuid WHERE o.version=$2 AND o.cleanup_group IS NULL AND NOT o.metadata_gone AND (SELECT count(*) FROM file_evidence_put WHERE owner=$2)<8 AND NOT EXISTS(SELECT 1 FROM file_evidence_write_fence WHERE object_key=o.object_key)")
            .bind(attempt).bind(version).bind(native_instance).execute(&mut *conn).await?.rows_affected();
        sqlx::query("UPDATE file_evidence_owner SET object_done=false WHERE version=$1")
            .bind(version)
            .execute(conn)
            .await?;
        if inserted != 1 {
            return Err(denied());
        }
        Ok(Some(attempt))
    }
    pub async fn finish_put(
        pool: &sqlx::PgPool,
        attempt: Option<uuid::Uuid>,
        observed: Option<&crate::blob_store::StoredObject>,
    ) -> Result<()> {
        let Some(attempt) = attempt else {
            return Ok(());
        };
        // An error/cancellation can leave a provider request in flight. Only a
        // successful exact observation or the native permanent write fence may
        // retire its operation. HEAD absence and TTL are not settlement proofs.
        if observed.is_none() {
            return Ok(());
        }
        let mut tx = pool.begin().await?;
        Self::lock_capture(&mut tx).await?;
        if let Some(object) = observed {
            let changed=sqlx::query("UPDATE file_evidence_owner o SET provider_version=$2,object_done=false FROM file_evidence_put p WHERE p.attempt=$1 AND p.owner=o.version AND o.object_key=$3 AND (o.provider_version IS NULL OR o.provider_version=$2)")
                .bind(attempt).bind(object.provider_version.as_stored()).bind(object.key.as_str()).execute(&mut *tx).await?.rows_affected();
            if changed != 1 {
                return Err(denied());
            }
            sqlx::query("INSERT INTO file_evidence_put_observation(attempt,object_key,provider_version) VALUES($1,$2,$3) ON CONFLICT(attempt) DO NOTHING")
                .bind(attempt).bind(object.key.as_str()).bind(object.provider_version.as_stored()).execute(&mut *tx).await?;
            // Observation of an actual PUT is not permission to publish Ready.
            let recorded=sqlx::query("UPDATE object_maintenance_ledger SET provider_version=$2,size_bytes=$3,state=CASE WHEN state='uploading' THEN 'ready' ELSE state END,updated=NOW() WHERE object_key=$1 AND (provider_version IS NULL OR provider_version=$2)")
                .bind(object.key.as_str()).bind(object.provider_version.as_stored()).bind(object.size as i64).execute(&mut *tx).await?.rows_affected();
            if recorded != 1 {
                return Err(denied());
            }
        }
        sqlx::query("DELETE FROM file_evidence_put WHERE attempt=$1")
            .bind(attempt)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Metadata reclamation has already closed publication. Recovery excludes
    /// legacy multipart operations whose remote staging identity is unknown.
    pub async fn recovery_candidates(
        pool: &sqlx::PgPool,
        limit: i64,
    ) -> Result<Vec<FilePutRecovery>> {
        let mut tx = pool.begin().await?;
        Self::lock_reclamation(&mut tx).await?;
        let versions:Vec<i64>=sqlx::query_scalar("SELECT version FROM file_evidence_owner o WHERE metadata_gone AND NOT object_done AND object_key IS NOT NULL AND EXISTS(SELECT 1 FROM file_evidence_put p WHERE p.owner=o.version AND protocol='atomic_create_v1') AND NOT EXISTS(SELECT 1 FROM file_evidence_put p WHERE p.owner=o.version AND protocol='legacy_unknown') ORDER BY version LIMIT $1 FOR UPDATE")
            .bind(limit.clamp(1,256)).fetch_all(&mut *tx).await?;
        for version in &versions {
            // Do this before installing the fence row. It also protects a
            // rollback to old maintenance binaries that do not know the table.
            sqlx::query("UPDATE object_maintenance_ledger SET state='write_fencing',updated=NOW() WHERE object_key=(SELECT object_key FROM file_evidence_owner WHERE version=$1) AND NOT EXISTS(SELECT 1 FROM file_evidence_write_fence WHERE object_key=object_maintenance_ledger.object_key)")
                .bind(version).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO file_evidence_write_fence(object_key,token,version) SELECT object_key,gen_random_uuid(),version FROM file_evidence_owner WHERE version=$1 ON CONFLICT(object_key) DO NOTHING")
                .bind(version).execute(&mut *tx).await?;
        }
        let plans=sqlx::query_as("SELECT version,object_key,token FROM file_evidence_write_fence WHERE version=ANY($1) AND NOT confirmed ORDER BY version")
            .bind(&versions).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(plans)
    }

    pub async fn complete_put_recovery(
        pool: &sqlx::PgPool,
        plan: &FilePutRecovery,
        settled: &crate::blob_store::FilePutSettlement,
    ) -> Result<()> {
        if settled.key().as_str() != plan.object_key || settled.token() != plan.token {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        Self::lock_reclamation(&mut tx).await?;
        let changed=sqlx::query("UPDATE file_evidence_write_fence SET confirmed=true,provider_version=$4 WHERE object_key=$1 AND token=$2 AND version=$3 AND (provider_version IS NULL OR provider_version=$4)")
            .bind(&plan.object_key).bind(plan.token).bind(plan.version).bind(settled.provider().as_stored()).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(denied());
        }
        sqlx::query("UPDATE object_maintenance_ledger SET state='write_fenced',provider_version=$2,size_bytes=0,updated=NOW() WHERE object_key=$1")
            .bind(&plan.object_key).bind(settled.provider().as_stored()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM file_evidence_put WHERE owner=$1 AND protocol='atomic_create_v1'")
            .bind(plan.version)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE file_evidence_owner SET object_done=true WHERE version=$1 AND metadata_gone AND NOT EXISTS(SELECT 1 FROM file_evidence_put WHERE owner=$1)")
            .bind(plan.version).execute(&mut *tx).await?;
        sqlx::query("SELECT file_evidence_release($1)")
            .bind(plan.version)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn lock_capture(conn: &mut PgConnection) -> Result<()> {
        SchemaTypeRepository::lock_selection(conn).await
    }
    pub async fn lock_reclamation(conn: &mut PgConnection) -> Result<()> {
        sqlx::query("SELECT file_evidence_gc_entry()")
            .execute(conn)
            .await?;
        Ok(())
    }
    pub async fn retain(conn: &mut PgConnection, version: i64, root: i64) -> Result<()> {
        let held: bool = sqlx::query_scalar("SELECT file_evidence_graph_held(false)")
            .fetch_one(&mut *conn)
            .await?;
        if !held {
            return Err(Error::invalid_state(
                "File capture must enter the shared graph gate before source locks",
            ));
        }
        // The original receipt/admission/attempt binding must outlive its
        // metadata-owned artifact_file_origin row as well as the root registry.
        let reservation:Option<Value>=sqlx::query_scalar("SELECT CASE WHEN octet_length(to_jsonb(o)::text)<=1048576 THEN to_jsonb(o) END FROM artifact_file_origin o WHERE artifact_version=$1 AND origin_read=$2")
            .bind(version).bind(root).fetch_optional(&mut *conn).await?.flatten();
        let reservation = reservation.ok_or_else(denied)?;
        let inserted=sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,reservation,object_key,provider_version,object_done,staging_path,staging_done) SELECT id,artifact,$2,$3,object_key,provider_version,object_key IS NULL,file_path,file_path IS NULL FROM artifact_version WHERE id=$1 ON CONFLICT(version) DO UPDATE SET reservation=EXCLUDED.reservation WHERE file_evidence_owner.evidence_state IN ('pending_audit','quarantined') AND file_evidence_owner.root_read=EXCLUDED.root_read AND file_evidence_owner.artifact=EXCLUDED.artifact AND NOT file_evidence_owner.metadata_gone AND file_evidence_owner.cleanup_group IS NULL")
            .bind(version).bind(root).bind(&reservation).execute(&mut *conn).await?.rows_affected();
        if inserted != 1 {
            return Err(denied());
        }
        // Even a reservation that never reaches PUT needs a native absence
        // observation. Otherwise deleting pending metadata leaves no ledger
        // candidate capable of completing its object obligation.
        let key: Option<String> =
            sqlx::query_scalar("SELECT object_key FROM file_evidence_owner WHERE version=$1")
                .bind(version)
                .fetch_one(&mut *conn)
                .await?;
        if let Some(key) = key {
            super::object_maintenance::ObjectMaintenanceRepository::reserve_upload(
                &mut *conn, &key, "artifact",
            )
            .await?;
        }
        let mut closure = Closure {
            queue: VecDeque::new(),
            seen: BTreeSet::new(),
            bytes: 0,
            proof_rows: 0,
            ..Closure::default()
        };
        closure.charge(&reservation)?;
        // Follow only the exact source rows already frozen in THIS reservation.
        // No execution-wide receipt query, historical Key lookup or newly
        // manufactured RuntimeReadParent is involved. C still owns read capture.
        let sources = reservation
            .get("receipts")
            .and_then(Value::as_array)
            .ok_or_else(denied)?;
        closure.rows(sources.len())?;
        for source in sources {
            // Other legacy source encodings are already covered by C's frozen
            // read parents. Never infer their ownership from source_kind.
            if source.get("source_kind").and_then(Value::as_str) != Some("provenance") {
                continue;
            }
            let provenance: SecretProvenance = serde_json::from_str(
                source
                    .get("source_ref")
                    .and_then(Value::as_str)
                    .ok_or_else(denied)?,
            )?;
            closure.origins(&provenance, 0)?;
        }
        closure.push("execution_runtime_read_origin", root, 0)?;
        while let Some((node, depth)) = closure.pop()? {
            if depth > 32 {
                return Err(denied());
            }
            if !closure.seen.insert(node.clone()) {
                continue;
            }
            if closure.seen.len() + closure.proof_rows > MAX_NODES {
                return Err(denied());
            }
            Self::expand(conn, &node, depth, &mut closure).await.map_err(|_|Error::PermissionDenied(format!("File evidence unavailable at {}/{}; retained dependency is missing, invalid or exceeds its bound",node.kind,node.id)))?;
            sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source,source_created,source_generation) VALUES($1,$2,$3,COALESCE($4,'-infinity'::timestamptz),COALESCE($5::BIGINT,0::BIGINT)) ON CONFLICT DO NOTHING")
                .bind(version).bind(&node.kind).bind(node.id).bind(node.created).bind(node.generation).execute(&mut *conn).await?;
        }
        let pins: i64 = sqlx::query_scalar("SELECT count(*) FROM file_evidence_pin WHERE owner=$1")
            .bind(version)
            .fetch_one(&mut *conn)
            .await?;
        if pins as usize != closure.seen.len()
            || closure
                .pending_cache_origins
                .iter()
                .any(|origin| !closure.cache_audit_origins.contains(origin))
            || closure
                .pending_iteration_origins
                .iter()
                .any(|origin| !closure.iteration_archive_origins.contains(origin))
            || closure
                .pending_success_metadata_origins
                .iter()
                .any(|origin| !closure.success_metadata_origins.contains(origin))
            || closure
                .pending_wait_completion_origins
                .iter()
                .any(|origin| !closure.wait_completion_origins.contains(origin))
        {
            return Err(denied());
        }
        sqlx::query("UPDATE file_evidence_owner SET evidence_state='retained',diagnostic=NULL WHERE version=$1")
            .bind(version).execute(conn).await?;
        Ok(())
    }
    fn native_parent(parent: &super::native_input_snapshot::NativeStorageParent) -> Result<Node> {
        use super::native_input_snapshot::NativeStorageParent as P;
        let (kind, id) = match parent {
            P::Execution { execution } => ("native_identity:execution", *execution),
            P::WorkflowExecution { workflow } => ("native_identity:workflow_execution", *workflow),
            P::WorkflowTaskWait { wait } => ("native_identity:workflow_task_wait", *wait),
            P::CacheIteration { iteration } => {
                ("native_identity:workflow_cache_iteration", *iteration)
            }
            P::CacheNamespace { namespace } => ("cache_namespace", *namespace),
            P::CacheGeneration { generation } => ("cache_generation", *generation),
            P::CacheEntry { generation, entry } => {
                if *generation <= 0 || *entry <= 0 {
                    return Err(denied());
                }
                return Ok(Node {
                    kind: "cache_entry".into(),
                    id: *entry,
                    generation: Some(*generation),
                    created: None,
                });
            }
            P::Inquiry { inquiry } => ("native_identity:inquiry", *inquiry),
            P::QueueItem { item } => ("native_identity:work_queue_item", *item),
            P::Pack { pack } => ("native_identity:pack", *pack),
            P::Identity { identity } => ("native_identity:identity", *identity),
            P::Action { action } => ("native_identity:action", *action),
            P::Sensor { sensor } => ("native_identity:sensor", *sensor),
            P::Event { event, created } => {
                if *event <= 0 {
                    return Err(denied());
                }
                return Ok(Node {
                    kind: "event".into(),
                    id: *event,
                    created: Some(*created),
                    generation: None,
                });
            }
            P::Receipt { receipt } => ("runtime_input_receipt", *receipt),
            P::Admission { admission } => ("runtime_authority_admission", *admission),
            P::Policy { policy } => ("pack_runtime_policy_revision", *policy),
            P::Approval { approval } => ("runtime_external_approval", *approval),
            P::NativeArchive { snapshot } => ("runtime_native_input_snapshot", *snapshot),
            P::NativeSnapshot { snapshot } => ("runtime_native_input_snapshot", *snapshot),
            P::NativeBudget { renderer_admission } => {
                ("runtime_native_input_snapshot_budget", *renderer_admission)
            }
        };
        if id <= 0 {
            return Err(denied());
        }
        Ok(Node::new(kind, id))
    }

    async fn native_storage(
        conn: &mut PgConnection,
        snapshot: i64,
        depth: usize,
        c: &mut Closure,
    ) -> Result<usize> {
        use super::native_input_snapshot::NativeInputSnapshotRepository;
        let root = snapshot;
        let mut pending = Some(snapshot);
        while let Some(snapshot) = pending.take() {
            if c.native_bytes.contains_key(&snapshot) {
                continue;
            }
            if c.native_bytes.len() >= MAX_NATIVE_SNAPSHOTS {
                return Err(denied());
            }
            let evidence = NativeInputSnapshotRepository::storage_evidence(conn, snapshot).await?;
            let bytes = evidence.retained_bytes();
            c.native_bytes.insert(snapshot, bytes);
            let archive_dependencies = evidence
                .wait_completion()
                .map(|wait| BTreeSet::from([wait.original_snapshot()]));
            if let Some(dependencies) = &archive_dependencies {
                let actual = evidence
                    .physical_parents()
                    .iter()
                    .filter_map(|parent| match parent {
                        super::native_input_snapshot::NativeStorageParent::NativeSnapshot {
                            snapshot,
                        } => Some(*snapshot),
                        _ => None,
                    })
                    .collect::<BTreeSet<_>>();
                if &actual != dependencies
                    || dependencies.iter().any(|id| *id <= 0 || *id >= snapshot)
                {
                    return Err(denied());
                }
            }
            let mut predecessor = None;
            for parent in evidence.physical_parents() {
                if let super::native_input_snapshot::NativeStorageParent::NativeSnapshot {
                    snapshot: parent,
                } = parent
                {
                    if archive_dependencies.is_some() {
                        continue;
                    }
                    if *parent <= 0 || *parent >= snapshot || predecessor.replace(*parent).is_some()
                    {
                        return Err(denied());
                    }
                }
            }
            c.native_predecessors.insert(snapshot, predecessor);
            pending = predecessor;
            if evidence.archive_owner() != ("native_input_snapshot", snapshot) {
                return Err(denied());
            }
            if let Some((kind, id, path)) = evidence.metadata().source_site() {
                let site = SecretOrigin::Entity {
                    entity_type: kind.into(),
                    entity_id: id,
                    path: path.into(),
                };
                if !c.archived_sites.contains(&site) {
                    c.archived_sites.push(site);
                }
            }
            if let Some(wait) = evidence.wait_completion() {
                let own = SecretOrigin::Entity {
                    entity_type: "wait_completion_metadata".into(),
                    entity_id: snapshot,
                    path: String::new(),
                };
                if wait.snapshot() != snapshot || !evidence.origins().contains(&own) {
                    return Err(denied());
                }
                if !c.wait_completion_origins.contains(&own) {
                    c.wait_completion_origins.push(own);
                }
                c.rows(
                    1 + wait.contributor_manifests().len()
                        + wait.original_receipt_edges().len()
                        + wait.receipt_edges().len()
                        + wait.controls().len()
                        + wait.issued_values().len(),
                )?;
                // Original wait Body remains a separate archive/byte charge,
                // never a predecessor-frontier subtraction or child ResultView.
                c.push(
                    "runtime_native_input_snapshot",
                    wait.original_snapshot(),
                    depth + 1,
                )?;
                c.push(
                    "runtime_authority_admission",
                    wait.original_renderer_admission(),
                    depth + 1,
                )?;
                c.push(
                    "runtime_authority_admission",
                    wait.original_source_admission(),
                    depth + 1,
                )?;
                c.push(
                    "runtime_authority_admission",
                    wait.original_registration().renderer_admission(),
                    depth + 1,
                )?;
                c.proof(&serde_json::to_value(wait.original_authority())?, depth + 1)?;
                c.proof(
                    &serde_json::to_value(wait.original_output_contract())?,
                    depth + 1,
                )?;
                if let Some(contract) = wait.original_source_contract() {
                    c.proof(contract, depth + 1)?;
                }
                let (kind, id, path) = wait.original_source_site();
                let site = SecretOrigin::Entity {
                    entity_type: kind.into(),
                    entity_id: id,
                    path: path.into(),
                };
                if !c.archived_sites.contains(&site) {
                    c.archived_sites.push(site);
                }
                c.archived_wait_fields(wait, depth)?;
                for (kind, present) in [
                    ("execution_config", wait.original_parent().config.is_some()),
                    ("execution_result", wait.original_parent().result.is_some()),
                ] {
                    if present {
                        let site = SecretOrigin::Entity {
                            entity_type: kind.into(),
                            entity_id: wait.execution(),
                            path: String::new(),
                        };
                        if !c.archived_sites.contains(&site) {
                            c.archived_sites.push(site);
                        }
                    }
                }
                c.archived_fields(
                    "execution_config",
                    wait.execution(),
                    &wait.original_config_fields(),
                    depth,
                )?;
                c.archived_fields(
                    "execution_result",
                    wait.execution(),
                    &wait.original_result_fields(),
                    depth,
                )?;
                c.archived_fields(
                    "workflow_variables",
                    wait.original_workflow().execution,
                    &wait.original_variable_fields(),
                    depth,
                )?;
                let site = SecretOrigin::Entity {
                    entity_type: "workflow_variables".into(),
                    entity_id: wait.original_workflow().execution,
                    path: String::new(),
                };
                if !c.archived_sites.contains(&site) {
                    c.archived_sites.push(site);
                }
                c.origins(
                    &SecretProvenance {
                        origins: wait.original_origins().to_vec(),
                        templates: vec![],
                    },
                    depth + 1,
                )?;
                // Controls and ordered VALUEs keep their genuine receipt sites
                // and purposes. No Condition -> VALUE conversion or grant union.
                for (receipt, _) in wait
                    .original_receipt_edges()
                    .iter()
                    .chain(wait.receipt_edges())
                    .chain(wait.controls())
                    .chain(wait.issued_values())
                {
                    c.push("runtime_input_receipt", *receipt, depth + 1)?;
                }
            }
            if let Some(success) = evidence.success_metadata() {
                let own = SecretOrigin::Entity {
                    entity_type: "ordinary_success_metadata".into(),
                    entity_id: snapshot,
                    path: String::new(),
                };
                if success.snapshot() != snapshot || !evidence.origins().contains(&own) {
                    return Err(denied());
                }
                if !c.success_metadata_origins.contains(&own) {
                    c.success_metadata_origins.push(own);
                }
                // A's distinct getter authenticates the real release, normalized
                // source inventory and live output-leaf rows. No pending branch.
                c.rows(
                    1 + success.children().len()
                        + success.root_contributor_manifests().len()
                        + success.original_child_contracts().len()
                        + success.output_leaves().len()
                        + success.receipt_edges().len()
                        + success.controls().len(),
                )?;
                c.proof(
                    &serde_json::to_value(success.original_contract())?,
                    depth + 1,
                )?;
                c.push(
                    "runtime_authority_admission",
                    success.original_renderer_admission(),
                    depth + 1,
                )?;
                for (_, contract) in success.original_child_contracts() {
                    c.proof(&serde_json::to_value(contract)?, depth + 1)?;
                }
                for (kind, present) in [
                    (
                        "execution_config",
                        success.original_parent().config.is_some(),
                    ),
                    (
                        "execution_result",
                        success.original_parent().result.is_some(),
                    ),
                ] {
                    if present {
                        let site = SecretOrigin::Entity {
                            entity_type: kind.into(),
                            entity_id: success.execution(),
                            path: String::new(),
                        };
                        if !c.archived_sites.contains(&site) {
                            c.archived_sites.push(site);
                        }
                    }
                }
                c.archived_fields(
                    "execution_config",
                    success.execution(),
                    &success.original_config_fields(),
                    depth,
                )?;
                c.archived_fields(
                    "execution_result",
                    success.execution(),
                    &success.original_result_fields(),
                    depth,
                )?;
                c.archived_fields(
                    "workflow_variables",
                    success.workflow().execution,
                    &success.original_variable_fields(),
                    depth,
                )?;
                let variables = SecretOrigin::Entity {
                    entity_type: "workflow_variables".into(),
                    entity_id: success.workflow().execution,
                    path: String::new(),
                };
                if !c.archived_sites.contains(&variables) {
                    c.archived_sites.push(variables);
                }
                for child in success.children() {
                    // Original child ResultView fields, never /children/N copies
                    // or aliases to finite-failure/public output metadata.
                    let site = SecretOrigin::Entity {
                        entity_type: "execution_result".into(),
                        entity_id: child.execution(),
                        path: String::new(),
                    };
                    if !c.archived_sites.contains(&site) {
                        c.archived_sites.push(site);
                    }
                    c.rows(child.contributor_manifests().len() + child.receipt_edges().len())?;
                    c.push("runtime_authority_admission", child.admission(), depth + 1)?;
                    if let Some(contract) = child.original_contract() {
                        c.proof(contract, depth + 1)?;
                    }
                    c.archived_fields(
                        "execution_result",
                        child.execution(),
                        &child.original_fields(),
                        depth,
                    )?;
                    c.origins(
                        &SecretProvenance {
                            origins: child.origins().to_vec(),
                            templates: vec![],
                        },
                        depth + 1,
                    )?;
                    for (receipt, _) in child.receipt_edges() {
                        c.push("runtime_input_receipt", *receipt, depth + 1)?;
                    }
                }
                for (receipt, _) in success.receipt_edges().iter().chain(success.controls()) {
                    c.push("runtime_input_receipt", *receipt, depth + 1)?;
                }
                for leaf in success.output_leaves() {
                    // Keep the actual canonical leaf receipt, including its
                    // independent original proof, ciphertext and key hash. A
                    // currently requires this live row for normalization.
                    c.proof(leaf.original_proof(), depth + 1)?;
                    c.push("runtime_input_receipt", leaf.receipt(), depth + 1)?;
                }
            }
            // Requires A d2b2a8fb + 628303e9 + b0d1cf3e. Generic fields and
            // manifests are intentionally empty for this distinct INPUT archive.
            if let Some(group) = evidence.iteration_outcome() {
                let own_origin = SecretOrigin::Entity {
                    entity_type: "ordinary_iteration_outcome".into(),
                    entity_id: snapshot,
                    path: String::new(),
                };
                if !evidence.origins().contains(&own_origin) {
                    return Err(denied());
                }
                if !c.iteration_archive_origins.contains(&own_origin) {
                    c.iteration_archive_origins.push(own_origin);
                }
                c.rows(
                    group.members().len()
                        + group.receipt_edges().len()
                        + group.list_controls().len(),
                )?;
                c.proof(&serde_json::to_value(group.original_schema())?, depth + 1)?;
                for (receipt, _) in group.receipt_edges().iter().chain(group.list_controls()) {
                    c.push("runtime_input_receipt", *receipt, depth + 1)?;
                }
                for member in group.members() {
                    // Never select only the latest retry or flatten independent
                    // original/latest contributor ceilings into one grant body.
                    for observed in [member.original(), member.latest()] {
                        let fields = observed.original_input_fields();
                        c.rows(
                            1 + fields.len()
                                + observed.input_receipt_edges().len()
                                + observed.contributor_manifests().len(),
                        )?;
                        c.proof(
                            &serde_json::to_value(observed.original_schema())?,
                            depth + 1,
                        )?;
                        c.push(
                            "runtime_authority_admission",
                            observed.admission(),
                            depth + 1,
                        )?;
                        for field in &fields {
                            let site = SecretOrigin::Entity {
                                entity_type: "execution_config".into(),
                                entity_id: observed.execution(),
                                path: field.json_path.clone(),
                            };
                            if !c.archived_sites.contains(&site) {
                                c.archived_sites.push(site);
                            }
                            // A authenticated these five original stored fields,
                            // including their actual receipt pointers, key hash
                            // and ciphertext. Traverse provenance, never decrypt.
                            if field.source_kind != "provenance" {
                                return Err(denied());
                            }
                            let provenance: SecretProvenance = serde_json::from_str(
                                field.source_ref.as_deref().ok_or_else(denied)?,
                            )?;
                            c.origins(&provenance, depth + 1)?;
                        }
                        for (receipt, _) in observed.input_receipt_edges() {
                            c.push("runtime_input_receipt", *receipt, depth + 1)?;
                        }
                    }
                }
            }
            // Requires A's finite completion storage projection (76419175).
            // Every child is archived by this SAME NativeArchive. Register its
            // original producer site, not the aggregate /children/N copy path,
            // before pending raw-source sites can be resolved by Closure::pop.
            for child in evidence.completion_sources() {
                let site = SecretOrigin::Entity {
                    entity_type: "execution_result".into(),
                    entity_id: child.execution(),
                    path: String::new(),
                };
                if !c.archived_sites.contains(&site) {
                    c.archived_sites.push(site);
                }
            }
            if let Some(cache) = evidence.cache() {
                // This is the concrete normalized binding, NOT the original opaque
                // audit path, which intentionally remains unchanged after binding.
                let epoch = Node::new("runtime_cache_iteration_epoch", cache.iteration());
                c.native_physical.insert(epoch.clone());
                c.push(&epoch.kind, epoch.id, depth + 1)?;
                for origin in evidence.origins() {
                    if !c.cache_audit_origins.contains(origin) {
                        c.cache_audit_origins.push(origin.clone());
                    }
                }
            }
            for parent in evidence.physical_parents() {
                let node = Self::native_parent(parent)?;
                c.native_physical.insert(node.clone());
                if depth > 32 || c.queue.len() + c.seen.len() > 2048 {
                    return Err(denied());
                }
                let parent_depth = if matches!(
                    parent,
                    super::native_input_snapshot::NativeStorageParent::NativeSnapshot { .. }
                ) && archive_dependencies.is_none()
                {
                    // Native chain depth has A's independent 128-row bound, while
                    // nested receipt/source edges keep F's existing depth bound.
                    depth
                } else {
                    depth + 1
                };
                c.queue.push_back((node, parent_depth));
            }
            c.rows(
                evidence.receipt_edges().len()
                    + usize::from(evidence.empty_iteration_release().is_some())
                    + evidence.admission_edges().len()
                    + evidence.archive_fields().len()
                    + evidence.contributor_manifests().len()
                    + evidence.completion_sources().len(),
            )?;
            // These copies are INCLUDED in A's byte total and the guarded archive.
            // Each contributor remains an independent frozen manifest, never a
            // merged grant set or a fresh requester/IAM lookup. Do not parse its
            // free-form attributes to infer additional physical source identities.
            if let Some(contract) = evidence.contract() {
                c.proof(&serde_json::to_value(contract)?, depth + 1)?;
            }
            for child in evidence.completion_sources() {
                if let Some(contract) = child.original_contract() {
                    c.proof(contract, depth + 1)?;
                }
            }
            c.origins(
                &SecretProvenance {
                    origins: evidence.origins().to_vec(),
                    templates: vec![],
                },
                depth + 1,
            )?;
            for (receipt, _) in evidence.receipt_edges() {
                c.push("runtime_input_receipt", *receipt, depth + 1)?;
            }
            for admission in evidence.admission_edges() {
                c.push("runtime_authority_admission", *admission, depth + 1)?;
            }
        }
        Ok(*c.native_bytes.get(&root).ok_or_else(denied)?)
    }

    async fn expand(
        conn: &mut PgConnection,
        node: &Node,
        depth: usize,
        c: &mut Closure,
    ) -> Result<()> {
        if node.kind.starts_with("native_identity:") {
            // A's archive keeps these frozen source identities/fields/contracts.
            // Raw execution/workflow/wait rows may already be gone. These are
            // not raw-row pins or current IAM freezes.
            if !c.native_physical.contains(node) {
                return Err(denied());
            }
            return c.charge(&serde_json::json!({"kind":node.kind,"id":node.id}));
        }
        if node.kind == "runtime_input_receipt" {
            return Self::receipt(conn, node.id, depth, c).await;
        }
        if node.kind == "accepted_input" {
            // Internal archive owner, never a public SecretOrigin. Its exact
            // body, ciphertext fields and original edges were authenticated and
            // charged by A's retention_parents before it entered this queue.
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM runtime_accepted_input WHERE id=$1)",
            )
            .bind(node.id)
            .fetch_one(conn)
            .await?;
            return if exists { Ok(()) } else { Err(denied()) };
        }
        if node.kind == "runtime_native_input_snapshot" {
            Self::native_storage(conn, node.id, depth, c).await?;
            let bytes = c.charge_native(node.id)?;
            c.charge_bytes(bytes)?;
            return Ok(());
        }
        if node.kind == "runtime_cache_iteration_epoch" {
            if !c.native_physical.contains(node) {
                return Err(denied());
            }
            // Historical pre-01017 iterations may have no epoch row. Never
            // invent one. If present, retain and byte-charge the exact extra
            // physical history once, without charging native inventory twice.
            let epoch:Option<Option<String>>=sqlx::query_scalar("SELECT CASE WHEN octet_length(creation_transaction)<=1048576 THEN creation_transaction END FROM runtime_cache_iteration_epoch WHERE iteration=$1")
                .bind(node.id).fetch_optional(&mut *conn).await?;
            if let Some(epoch) = epoch {
                c.rows(1)?;
                c.charge(&serde_json::json!({"iteration":node.id,"creation_transaction":epoch.ok_or_else(denied)?}))?;
            }
            return c.charge(&serde_json::json!({"kind":node.kind,"id":node.id}));
        }
        if matches!(
            node.kind.as_str(),
            "cache_namespace"
                | "cache_generation"
                | "cache_entry"
                | "runtime_native_input_snapshot_budget"
        ) {
            if !c.native_physical.contains(node) {
                return Err(denied());
            }
            // Exact IDs came from A's typed inventory, not current cursors.
            return c.charge(
                &serde_json::json!({"kind":node.kind,"id":node.id,"generation":node.generation}),
            );
        }
        if node.kind == "execution_runtime_read_origin" {
            let read = RuntimeReadRepository::find(&mut *conn, node.id).await?;
            c.charge(&serde_json::to_value(&read)?)?;
            c.read(&read, depth)?;
            let admission: i64 = sqlx::query_scalar(
                "SELECT admission FROM execution_runtime_read_origin WHERE id=$1",
            )
            .bind(node.id)
            .fetch_one(conn)
            .await?;
            return c.push("runtime_authority_admission", admission, depth + 1);
        }
        if node.kind == "event_origin_snapshot" {
            // Consume C's digest-checked exact-tuple evidence, not event.id alone.
            let event =
                super::event_origin::EventOriginRepository::evidence(&mut *conn, node.id).await?;
            c.charge(&serde_json::to_value(&event)?)?;
            c.proof(&serde_json::to_value(&event.binding)?, 0)?;
            if let Some(id) = event.trigger {
                c.push("trigger", id, depth + 1)?;
            }
            if let Some(id) = event.managing_pack {
                c.push("pack", id, depth + 1)?;
            }
            c.queue.push_back((
                Node {
                    kind: "event".into(),
                    id: event.event,
                    created: Some(event.created),
                    generation: None,
                },
                depth + 1,
            ));
            // Parent provenance is already frozen in C's RuntimeReadOrigin.
            // Event snapshot parents may introduce further registry references.
            for parent in event.payload.values().chain(event.config.values()) {
                c.origins(parent, depth + 1)?;
            }
            return Ok(());
        }
        if node.kind == "event" {
            let found: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM event WHERE id=$1 AND created=$2)")
                    .bind(node.id)
                    .bind(node.created)
                    .fetch_one(conn)
                    .await?;
            return if found { Ok(()) } else { Err(denied()) };
        }
        // An allowlist fixes identifiers. Server-side size checks keep enormous
        // mutable payloads/ciphertext out of the disclosure-evidence budget.
        let projection=match node.kind.as_str() {
            "execution"|"enforcement"=>"jsonb_build_object('pack_release',r.pack_release,'executable_snapshot',r.executable_snapshot)",
            "runtime_authority_admission"=>"jsonb_build_object('policy_revision',r.policy_revision,'authority',r.authority,'owner_kind',r.owner_kind,'owner_id',r.owner_id)",
            "pack_runtime_policy_revision"=>"jsonb_build_object('pack',r.pack,'release',jsonb_build_object('id',r.release),'policy',r.policy)",
            "pack_release"=>"jsonb_build_object('pack',r.pack)",
            "runtime_external_approval"=>"jsonb_build_object('pack',r.subject_pack)",
            "runtime_declared_input_binding"=>"jsonb_build_object('policy_revision',r.policy_revision,'declaration',r.declaration,'declaration_digest',r.declaration_digest)",
            "schema_type_revision"=>"jsonb_build_object('release_id',r.release)",
            "managed_executable_metadata_revision"=>"jsonb_build_object('pack',r.pack,'action',r.action,'release',jsonb_build_object('id',r.release),'snapshot',r.snapshot)",
            "sensor_generation_schema_binding"=>"jsonb_build_object('binding',r.binding,'initial_rules',r.initial_rules)",
            "sensor_rule_config_delivery"=>"jsonb_build_object('generation_binding',r.generation_binding,'rule',r.rule)",
            "component_configuration_revision"=>"jsonb_build_object('owner_kind',r.owner_kind,'owner_id',r.owner_id,'contract',r.contract)",
            "inquiry"=>"jsonb_build_object('created_by_execution',r.created_by_execution,'binding',(SELECT b.binding FROM inquiry_schema_binding b WHERE b.entity_id=r.id))",
            "artifact"=>"(SELECT jsonb_build_object('owner_kind',b.owner_type,'owner_id',b.owner_id,'pack',b.managing_pack) FROM artifact_runtime_owner_binding b WHERE b.artifact=r.id)",
            "artifact_version"=>"jsonb_build_object('artifact',r.artifact,'execution',r.execution)",
            "log_stream"=>"jsonb_build_object('artifact_version',r.artifact_version)",
            "work_queue_item"=>"jsonb_build_object('queue',r.queue,'pack_release',r.pack_release,'executable_snapshot',r.executable_snapshot,'binding',r.schema_binding)",
            "pack"|"action"|"sensor"|"rule"|"trigger"|"work_queue"=>"jsonb_build_object('id',r.id,'pack',to_jsonb(r)->'pack')",
            _=>return Err(denied()),
        };
        let sql=format!("SELECT CASE WHEN octet_length(({projection})::text)<=1048576 THEN {projection} END FROM {} r WHERE r.id=$1",node.kind);
        let body: Option<Value> = sqlx::query_scalar(&sql)
            .bind(node.id)
            .fetch_optional(&mut *conn)
            .await?
            .flatten();
        let body = body.ok_or_else(denied)?;
        c.charge(&body)?;
        if node.kind == "runtime_declared_input_binding" {
            let proof = body.get("declaration").ok_or_else(denied)?;
            let digest = crate::value_schema::digest(proof).map_err(|_| denied())?;
            if body.get("declaration_digest").and_then(Value::as_str) != Some(digest.as_str()) {
                return Err(denied());
            }
        }
        c.proof(&body, 0)?;
        let id = |field: &str| body.get(field).and_then(Value::as_i64);
        for (field, kind) in [
            ("queue", "work_queue"),
            ("pack", "pack"),
            ("action", "action"),
            ("rule", "rule"),
            ("generation_binding", "sensor_generation_schema_binding"),
            ("created_by_execution", "execution"),
            ("artifact", "artifact"),
            ("artifact_version", "artifact_version"),
            ("execution", "execution"),
        ] {
            if let Some(id) = id(field) {
                c.push(kind, id, depth + 1)?;
            }
        }
        if ["component_configuration_revision", "artifact"].contains(&node.kind.as_str()) {
            if let (Some(kind), Some(id)) = (
                body.get("owner_kind").and_then(Value::as_str),
                id("owner_id"),
            ) {
                if ["execution", "enforcement", "pack", "sensor", "action"].contains(&kind) {
                    c.push(kind, id, depth + 1)?;
                }
            }
        }
        if node.kind == "execution" {
            let contract:Option<Value>=sqlx::query_scalar("SELECT CASE WHEN octet_length(contract::text)<=1048576 THEN contract END FROM runtime_schema_binding WHERE entity_kind='execution' AND entity_id=$1")
                .bind(node.id).fetch_optional(&mut *conn).await?.flatten();
            let contract = contract.ok_or_else(denied)?;
            c.charge(&contract)?;
            c.proof(&contract, 0)?;
        }
        if node.kind == "runtime_authority_admission" {
            // A purpose proof retains admissions independently of raw source
            // executions. Only C's actual entity/target origins pin raw rows.
            let parents:Vec<i64>=sqlx::query_scalar("SELECT dependency FROM runtime_admission_dependency WHERE admission=$1 ORDER BY dependency LIMIT 257")
                .bind(node.id).fetch_all(&mut *conn).await?;
            c.rows(parents.len())?;
            c.charge(&serde_json::to_value(&parents)?)?;
            for parent in parents {
                c.push("runtime_authority_admission", parent, depth + 1)?;
            }
        }
        if node.kind == "pack_runtime_policy_revision" {
            let owners:Vec<(String,i64)>=sqlx::query_as("SELECT owner_type,owner_id FROM runtime_standard_owner_binding WHERE policy_revision=$1 ORDER BY owner_type,owner_id LIMIT 257")
                .bind(node.id).fetch_all(&mut *conn).await?;
            for (kind, id) in owners {
                if !["pack", "action"].contains(&kind.as_str()) {
                    return Err(denied());
                }
                c.push(&kind, id, depth + 1)?;
            }
            let releases:Vec<i64>=sqlx::query_scalar("SELECT release FROM runtime_authority_release_dependency WHERE policy_revision=$1 ORDER BY release LIMIT 257").bind(node.id).fetch_all(&mut *conn).await?;
            let metadata:Vec<i64>=sqlx::query_scalar("SELECT metadata_revision FROM runtime_authority_metadata_dependency WHERE policy_revision=$1 ORDER BY metadata_revision LIMIT 257").bind(node.id).fetch_all(&mut *conn).await?;
            let policies:Vec<i64>=sqlx::query_scalar("SELECT dependency FROM runtime_workflow_policy_dependency WHERE policy_revision=$1 ORDER BY dependency LIMIT 257").bind(node.id).fetch_all(&mut *conn).await?;
            for id in releases {
                c.push("pack_release", id, depth + 1)?;
            }
            for id in metadata {
                c.push("managed_executable_metadata_revision", id, depth + 1)?;
            }
            for id in policies {
                c.push("pack_runtime_policy_revision", id, depth + 1)?;
            }
        }
        if [
            "sensor_generation_schema_binding",
            "sensor_rule_config_delivery",
        ]
        .contains(&node.kind.as_str())
        {
            let source = if node.kind == "sensor_generation_schema_binding" {
                "sensor_generation_rule_config"
            } else {
                "sensor_rule_delivery_config"
            };
            let proofs:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('rule',rule_id,'policy_revision',dispatch_policy_revision,'pack_release',dispatch_source_release,'proof',proof) FROM sensor_rule_structural_origin WHERE source_entity=$1 AND source_id=$2 ORDER BY rule_id LIMIT 257")
                .bind(source).bind(node.id).fetch_all(&mut *conn).await?;
            for proof in proofs {
                c.charge(&proof)?;
                c.proof(&proof, 0)?;
                if let Some(id) = proof.get("rule").and_then(Value::as_i64) {
                    c.push("rule", id, depth + 1)?;
                }
            }
        }
        Ok(())
    }

    async fn receipt(
        conn: &mut PgConnection,
        id: i64,
        depth: usize,
        c: &mut Closure,
    ) -> Result<()> {
        use super::inherited_input::{
            InheritedInputRepository, InputSite, ReadSelection, SelectionSource,
        };
        let body:Option<Value>=sqlx::query_scalar("SELECT CASE WHEN octet_length(b::text)<=1048576 THEN b END FROM (SELECT jsonb_build_object('execution',execution,'entity_kind',entity_kind,'destination_purpose',destination_purpose,'destination_path',destination_path,'destination_admission',destination_admission,'renderer_admission',renderer_admission,'declaration',declaration,'proof',proof,'proof_digest',proof_digest,'encrypted_value',encrypted_value,'encryption_key_hash',encryption_key_hash) b FROM runtime_input_receipt WHERE id=$1) r")
            .bind(id).fetch_optional(&mut *conn).await?.flatten();
        let body = body.ok_or_else(denied)?;
        // Unknown future contracts still fail closed. NativeSnapshot now uses
        // A's authenticated typed storage inventory, never a name-only bypass.
        let selections = body
            .get("proof")
            .and_then(|proof| proof.get("selections"))
            .and_then(Value::as_array)
            .ok_or_else(denied)?;
        if selections.len() > MAX_NODES || !selections.iter().all(retained_selection_supported) {
            return Err(denied());
        }
        // A authenticates direct receipt/admission/archive/original-origin
        // edges, including accepted inputs, controls, System and Iteration.
        // Never rederive this closure from today's execution secret rows.
        let parents = InheritedInputRepository::retention_parents(conn, id).await?;
        let mut native = Vec::new();
        for snapshot in parents.native_snapshots() {
            let bytes = Self::native_storage(conn, *snapshot, depth + 1, c).await?;
            native.push((*snapshot, bytes));
        }
        // A's aggregate already contains native bytes. Deduct only an exact
        // snapshot charged earlier in this SAME cumulative closure.
        c.charge_receipt(parents.retained_bytes(), &native)?;
        c.rows(
            parents.receipts().len()
                + parents.admissions().len()
                + parents.accepted_inputs().len()
                + parents.native_snapshots().len()
                + parents.policies().len()
                + parents.approvals().len()
                + parents.origins().len(),
        )?;
        let mut envelope = body.clone();
        let envelope = envelope.as_object_mut().ok_or_else(denied)?;
        // Already included, byte-exactly, in A's opaque retained_bytes charge.
        envelope.remove("proof");
        envelope.remove("encrypted_value");
        c.charge(&serde_json::to_value(envelope)?)?;
        let proof = body.get("proof").ok_or_else(denied)?;
        let digest = crate::value_schema::digest(proof).map_err(|_| denied())?;
        if body.get("proof_digest").and_then(Value::as_str) != Some(digest.as_str()) {
            return Err(denied());
        }
        let site: InputSite =
            serde_json::from_value(proof.get("site").cloned().ok_or_else(denied)?)?;
        if body.get("execution").and_then(Value::as_i64) != Some(site.execution)
            || body.get("entity_kind").and_then(Value::as_str) != Some(site.kind.as_str())
            || body.get("destination_purpose") != Some(&serde_json::to_value(site.purpose)?)
            || body.get("destination_path").and_then(Value::as_str) != Some(site.path.as_str())
            || proof.get("renderer") != body.get("renderer_admission")
            || proof.get("declaration") != body.get("declaration")
        {
            return Err(denied());
        }
        for receipt in parents.receipts() {
            c.push("runtime_input_receipt", *receipt, depth + 1)?;
        }
        for admission in parents.admissions() {
            c.push("runtime_authority_admission", *admission, depth + 1)?;
        }
        for accepted in parents.accepted_inputs() {
            c.push("accepted_input", *accepted, depth + 1)?;
        }
        for snapshot in parents.native_snapshots() {
            c.push("runtime_native_input_snapshot", *snapshot, depth + 1)?;
        }
        for policy in parents.policies() {
            c.push("pack_runtime_policy_revision", *policy, depth + 1)?;
        }
        for approval in parents.approvals() {
            c.push("runtime_external_approval", *approval, depth + 1)?;
        }
        c.origins(
            &SecretProvenance {
                origins: parents.origins().to_vec(),
                templates: vec![],
            },
            depth,
        )?;
        let declaration = body.get("declaration").and_then(Value::as_i64);
        if let Some(declaration) = declaration {
            c.push("runtime_declared_input_binding", declaration, depth + 1)?;
        }
        let selections: Vec<ReadSelection> =
            serde_json::from_value(proof.get("selections").cloned().ok_or_else(denied)?)?;
        if selections.len() > 256 {
            return Err(denied());
        }
        for selection in selections {
            // Only selected authored approval/read and structural evidence need
            // additional pins. All generic ancestry comes from A's hook above;
            // this is not a second parser/evaluator for source dependencies.
            if let SelectionSource::Authored {
                origins,
                target,
                input_authority,
            } = selection.source
            {
                if let Some(target) = target {
                    c.proof(&serde_json::to_value(target)?, 0)?;
                }
                if let Some(approval) = input_authority {
                    let declaration = declaration.ok_or_else(denied)?;
                    c.push("runtime_external_approval", approval, depth + 1)?;
                    for origin in &origins {
                        let origin = serde_json::to_value(origin)?;
                        let digest = crate::value_schema::digest(&origin).map_err(|_| denied())?;
                        // Only the actual selected declaration/approval/read
                        // is frozen. Replacement approvals appended later
                        // are not unioned into the file's dependencies.
                        let read:Option<Value>=sqlx::query_scalar("SELECT CASE WHEN octet_length(b::text)<=1048576 THEN b END FROM (SELECT jsonb_build_object('origin',r.origin,'target',r.target) b FROM runtime_declared_input_key_read r JOIN runtime_declared_input_approval a USING(declaration,approval) JOIN runtime_input_only_approval purpose ON purpose.approval=a.approval WHERE r.declaration=$1 AND r.approval=$2 AND r.origin_digest=$3 AND r.origin=$4) actual")
                                .bind(declaration).bind(approval).bind(&digest).bind(&origin).fetch_optional(&mut *conn).await?.flatten();
                        c.rows(1)?;
                        c.charge(&read.ok_or_else(denied)?)?;
                    }
                }
            }
        }
        let configurations:Vec<i64>=sqlx::query_scalar("SELECT revision FROM runtime_input_configuration_reference WHERE receipt=$1 ORDER BY revision LIMIT 257")
            .bind(id).fetch_all(&mut *conn).await?;
        c.rows(configurations.len())?;
        c.charge(&serde_json::to_value(&configurations)?)?;
        for revision in configurations {
            c.push("component_configuration_revision", revision, depth + 1)?;
        }
        Ok(())
    }
    pub async fn request_cleanup(conn: &mut PgConnection, versions: &[i64]) -> Result<Vec<i64>> {
        Ok(
            sqlx::query_scalar("SELECT file_evidence_request_cleanup($1)")
                .bind(versions)
                .fetch_one(conn)
                .await?,
        )
    }
    pub async fn eligible(conn: &mut PgConnection, kind: &str, id: i64) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT NOT file_evidence_pinned($1,$2)")
            .bind(kind)
            .bind(id)
            .fetch_one(conn)
            .await?)
    }
    pub async fn artifact_eligible(conn: &mut PgConnection, id: i64) -> Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT file_evidence_artifact_eligible($1)")
                .bind(id)
                .fetch_one(conn)
                .await?,
        )
    }
    pub async fn staging_removed(conn: &mut PgConnection, path: &str, version: i64) -> Result<()> {
        Self::lock_reclamation(conn).await?;
        sqlx::query("UPDATE file_evidence_owner SET staging_done=true WHERE version=$1 AND staging_path=$2 AND metadata_gone")
            .bind(version).bind(path).execute(&mut *conn).await?;
        sqlx::query("SELECT file_evidence_release($1)")
            .bind(version)
            .execute(conn)
            .await?;
        Ok(())
    }
    pub async fn object_removed(
        conn: &mut PgConnection,
        key: &str,
        provider: Option<&str>,
    ) -> Result<()> {
        // Caller owns exclusive graph admission BEFORE locking its ledger row.
        let versions:Vec<i64>=sqlx::query_scalar("UPDATE file_evidence_owner SET object_done=true WHERE object_key=$1 AND provider_version IS NOT DISTINCT FROM $2 RETURNING version")
            .bind(key).bind(provider).fetch_all(&mut *conn).await?;
        for version in versions {
            sqlx::query("SELECT file_evidence_release($1)")
                .bind(version)
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query("DELETE FROM file_evidence_orphan_object WHERE object_key=$1 AND provider_version IS NOT DISTINCT FROM $2")
            .bind(key).bind(provider).execute(conn).await?;
        Ok(())
    }
}

#[cfg(test)]
mod success_custody_dispatch_tests {
    use super::*;
    #[test]
    fn success_metadata_origin_requires_its_own_authenticated_storage_kind() {
        let origin = SecretOrigin::Entity {
            entity_type: "ordinary_success_metadata".into(),
            entity_id: 53,
            path: String::new(),
        };
        let provenance = SecretProvenance {
            origins: vec![origin.clone()],
            templates: vec![],
        };
        let mut closure = Closure::default();
        closure.origins(&provenance, 0).unwrap();
        assert_eq!(
            closure.pop().unwrap().unwrap().0,
            Node::new("runtime_native_input_snapshot", 53)
        );
        assert_eq!(
            closure.pending_success_metadata_origins,
            vec![origin.clone()]
        );
        assert!(closure.success_metadata_origins.is_empty());
        assert!(closure.iteration_archive_origins.is_empty());
        closure.success_metadata_origins.push(origin);
        closure.origins(&provenance, 0).unwrap();
        assert!(closure.pop().unwrap().is_none());
        for (id, path) in [(0, ""), (53, "/invented-output-leaf")] {
            let provenance = SecretProvenance {
                origins: vec![SecretOrigin::Entity {
                    entity_type: "ordinary_success_metadata".into(),
                    entity_id: id,
                    path: path.into(),
                }],
                templates: vec![],
            };
            assert!(Closure::default().origins(&provenance, 0).is_err());
        }
    }
}

#[cfg(test)]
mod wait_completion_custody_tests {
    use super::*;
    #[test]
    fn wait_completion_origin_is_distinct_and_original_archive_bytes_are_independent() {
        let origin = SecretOrigin::Entity {
            entity_type: "wait_completion_metadata".into(),
            entity_id: 59,
            path: String::new(),
        };
        let provenance = SecretProvenance {
            origins: vec![origin.clone()],
            templates: vec![],
        };
        let mut closure = Closure::default();
        closure.origins(&provenance, 0).unwrap();
        assert_eq!(
            closure.pop().unwrap().unwrap().0,
            Node::new("runtime_native_input_snapshot", 59)
        );
        assert_eq!(
            closure.pending_wait_completion_origins,
            vec![origin.clone()]
        );
        assert!(closure.wait_completion_origins.is_empty());
        assert!(closure.success_metadata_origins.is_empty());
        assert!(closure.iteration_archive_origins.is_empty());
        closure.wait_completion_origins.push(origin);
        closure.origins(&provenance, 0).unwrap();
        assert!(closure.pop().unwrap().is_none());
        closure.native_bytes.insert(41, 120);
        closure.native_bytes.insert(59, 180);
        closure.native_predecessors.insert(41, None);
        closure.native_predecessors.insert(59, None);
        assert_eq!(closure.charge_native(59).unwrap(), 180);
        assert_eq!(closure.charge_native(41).unwrap(), 120);
        assert_eq!(closure.charge_native(41).unwrap(), 0);
        for (id, path) in [(0, ""), (59, "/fake-child")] {
            let provenance = SecretProvenance {
                origins: vec![SecretOrigin::Entity {
                    entity_type: "wait_completion_metadata".into(),
                    entity_id: id,
                    path: path.into(),
                }],
                templates: vec![],
            };
            assert!(Closure::default().origins(&provenance, 0).is_err());
        }
    }
}
