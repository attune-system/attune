//! The collector deliberately keeps the existing leaf units. Receipts retain
//! ancestors, snapshots retain predecessors, and admissions retain dependencies.
//! No phase absorbs a referenced owner or splits a closed-set/cycle operation.
//! Cascades only remove a selected leaf's own normalized evidence. Existing
//! file-evidence statement/row guards and FK pins remain authoritative.

use super::bounded::GraphReclamationPhase as Phase;

impl Phase {
    pub(super) fn identity(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Receipts => ("runtime_input_receipt", "id", "receipt"),
            Self::AcceptedInputs => ("runtime_accepted_input", "id", "input"),
            Self::NativeSnapshots => ("runtime_native_input_snapshot", "id", "input"),
            Self::CacheEpochs => ("runtime_cache_iteration_epoch", "iteration", "epoch"),
            Self::Admissions => ("runtime_authority_admission", "id", "a"),
            Self::PolicyRevisions => ("pack_runtime_policy_revision", "id", "p"),
            Self::Releases => ("pack_release", "id", "r"),
        }
    }

    pub(super) fn delete_sql(self, accepted_items: bool, queue_admission: bool) -> String {
        let (table, id, alias) = self.identity();
        let joins = match self {
            Self::NativeSnapshots => {
                "JOIN runtime_authority_admission a ON a.id=input.renderer_admission"
            }
            Self::PolicyRevisions => "JOIN pack_release r ON r.id=p.release",
            _ => "",
        };
        let predicates = match self {
            Self::Receipts => "NOT runtime_accepted_input_holds_control(receipt.id)
                AND NOT file_evidence_pinned('runtime_input_receipt',receipt.id)
                AND NOT EXISTS(SELECT 1 FROM execution e WHERE e.id=receipt.execution)
                AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_source edge WHERE edge.source_receipt=receipt.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot_parent parent WHERE parent.receipt=receipt.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_native_empty_root_leaf root_leaf WHERE root_leaf.receipt=receipt.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_secret_origin_reference origin WHERE origin.source_id=receipt.id AND origin.source_entity LIKE 'declared_input:%')",
            Self::AcceptedInputs => "NOT file_evidence_pinned('accepted_input',input.id)
                AND NOT EXISTS(SELECT 1 FROM execution e WHERE e.id=input.execution)
                AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_accepted receipt WHERE receipt.accepted=input.id)",
            Self::NativeSnapshots => "NOT file_evidence_pinned('runtime_native_input_snapshot',input.id)
                AND NOT EXISTS(SELECT 1 FROM execution e WHERE e.id=a.owner_id)
                AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_snapshot receipt WHERE receipt.snapshot=input.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_native_cache_frontier frontier WHERE frontier.predecessor=input.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_native_wait_completion_release completed_wait WHERE completed_wait.original_snapshot=input.id)",
            Self::CacheEpochs => "NOT file_evidence_cache_epoch_pinned(epoch.iteration)
                AND NOT EXISTS(SELECT 1 FROM workflow_cache_iteration i WHERE i.id=epoch.iteration)
                AND NOT EXISTS(SELECT 1 FROM runtime_native_cache_frontier f WHERE f.bound_iteration=epoch.iteration)",
            // Two independent probes preserve the original OR's semantics and
            // allow each existing admission-reference index to be used.
            Self::Admissions => "NOT file_evidence_pinned('runtime_authority_admission',a.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_admission_dependency edge WHERE edge.dependency=a.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_declared_input_receipt receipt WHERE receipt.source_admission=a.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt receipt WHERE receipt.renderer_admission=a.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt receipt WHERE receipt.destination_admission=a.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_dependency proof WHERE proof.admission=a.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_accepted_input input WHERE input.admission=a.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot input WHERE input.renderer_admission=a.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot_admission input WHERE input.admission=a.id)
                AND ((a.owner_kind='execution' AND NOT EXISTS(SELECT 1 FROM execution e WHERE e.id=a.owner_id))
                  OR (a.owner_kind='enforcement' AND NOT EXISTS(SELECT 1 FROM enforcement e WHERE e.id=a.owner_id)))",
            Self::PolicyRevisions => "NOT file_evidence_pinned('pack_runtime_policy_revision',p.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot snapshot WHERE snapshot.policy_revision=p.id)
                AND NOT EXISTS(SELECT 1 FROM pack_runtime_policy_current c WHERE c.revision=p.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_authority_admission a WHERE a.policy_revision=p.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_workflow_policy_dependency graph WHERE graph.dependency=p.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_accepted_batch_item accepted WHERE accepted.target_policy=p.id)
                AND NOT EXISTS(SELECT 1 FROM runtime_declared_input_binding binding JOIN runtime_declared_input_receipt receipt ON receipt.declaration=binding.id WHERE binding.policy_revision=p.id)
                AND NOT EXISTS(SELECT 1 FROM sensor_rule_structural_origin o WHERE o.dispatch_policy_revision=p.id)",
            Self::Releases => RELEASE_PINS,
        };
        let retention = if matches!(self, Self::PolicyRevisions | Self::Releases) {
            // Equivalent to row_number > newest_inactive, including equal-time
            // ID ordering, without ranking the entire release catalog. Probe at
            // most the protected count for each raw candidate.
            "AND r.inactive_since < $4
             AND NOT EXISTS(SELECT 1 FROM pack active WHERE active.active_release=r.id)
             AND (SELECT count(*) FROM (
                 SELECT 1 FROM pack_release newer WHERE newer.pack=r.pack
                   AND newer.inactive_since IS NOT NULL
                   AND (newer.inactive_since,newer.id)>(r.inactive_since,r.id)
                 LIMIT $5
             ) protected) >= $5"
        } else {
            ""
        };
        let accepted_pin = if self == Self::Admissions && accepted_items {
            "AND NOT EXISTS(SELECT 1 FROM work_queue_item_ad_hoc_dispatch accepted WHERE accepted.source_admission=a.id)"
        } else {
            ""
        };
        let queue_pin = if self == Self::PolicyRevisions && queue_admission {
            "AND NOT EXISTS(SELECT 1 FROM work_queue_item_runtime_admission queued WHERE queued.policy_revision=p.id)"
        } else {
            ""
        };
        let returning = if self == Self::Releases {
            "RETURNING target.digest,target.content_path"
        } else {
            ""
        };
        format!("WITH raw AS MATERIALIZED (
                    SELECT candidate.id,candidate.version FROM unnest($1::bigint[],$2::text[]) AS candidate(id,version)
                 ), settings AS (SELECT $3::bigint AS delete_limit,$4::timestamptz AS cutoff,$5::bigint AS newest),
                 dormant AS (
                    SELECT {alias}.{id} AS id FROM raw
                    JOIN {table} {alias} ON {alias}.{id}=raw.id AND {alias}.xmin::text=raw.version
                    {joins} WHERE {predicates} {retention} {accepted_pin} {queue_pin}
                    ORDER BY {alias}.{id} LIMIT $3 FOR UPDATE OF {alias} SKIP LOCKED
                 ) DELETE FROM {table} target USING dormant WHERE target.{id}=dormant.id {returning}")
    }
}

const RELEASE_PINS: &str = "NOT file_evidence_pinned('pack_release',r.id)
    AND NOT EXISTS(SELECT 1 FROM runtime_authority_release_dependency d WHERE d.release=r.id)
    AND NOT EXISTS(SELECT 1 FROM managed_sensor_metadata_revision m WHERE m.release=r.id)
    AND NOT EXISTS(SELECT 1 FROM sensor_rule_structural_origin o WHERE o.dispatch_source_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM execution e WHERE e.pack_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM enforcement e WHERE e.pack_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM work_queue_item q WHERE q.pack_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM sensor_workload s WHERE s.pack_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM sensor_generation_schema_binding b WHERE (b.binding->'executable'->'release'->>'id')::BIGINT=r.id)
    AND NOT EXISTS(SELECT 1 FROM execution e CROSS JOIN LATERAL jsonb_each(COALESCE(e.executable_snapshot->'pack_executables','{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT=r.id)
    AND NOT EXISTS(SELECT 1 FROM enforcement e CROSS JOIN LATERAL jsonb_each(COALESCE(e.executable_snapshot->'pack_executables','{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT=r.id)
    AND NOT EXISTS(SELECT 1 FROM work_queue_item q CROSS JOIN LATERAL jsonb_each(COALESCE(q.executable_snapshot->'pack_executables','{}'::jsonb)) child WHERE (child.value->'release'->>'id')::BIGINT=r.id)
    AND NOT EXISTS(SELECT 1 FROM runtime_schema_binding b CROSS JOIN LATERAL jsonb_each(COALESCE(b.contract->'static_actions','{}'::jsonb)) child WHERE (child.value->'executable_snapshot'->'release'->>'id')::BIGINT=r.id)
    AND NOT EXISTS(SELECT 1 FROM runtime_schema_binding b CROSS JOIN LATERAL jsonb_each(COALESCE(b.contract->'static_actions','{}'::jsonb)) child CROSS JOIN LATERAL jsonb_each(COALESCE(child.value->'executable_snapshot'->'pack_executables','{}'::jsonb)) nested WHERE (nested.value->'release'->>'id')::BIGINT=r.id)
    AND NOT EXISTS(SELECT 1 FROM runtime c WHERE c.retired_at IS NULL AND c.managed_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM runtime_version c WHERE c.retired_at IS NULL AND c.managed_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM permission_set c WHERE c.retired_at IS NULL AND c.managed_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM trigger c WHERE c.retired_at IS NULL AND c.managed_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM action c WHERE c.retired_at IS NULL AND c.managed_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM sensor c WHERE c.retired_at IS NULL AND c.managed_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM rule c WHERE c.retired_at IS NULL AND c.managed_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM policy c WHERE c.retired_at IS NULL AND c.managed_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM work_queue c WHERE c.retired_at IS NULL AND c.managed_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM workflow_definition c WHERE c.retired_at IS NULL AND c.managed_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM dashboard c WHERE c.retired_at IS NULL AND c.managed_release=r.id)
    AND NOT EXISTS(SELECT 1 FROM cache_namespace c WHERE c.retired_at IS NULL AND c.managed_release=r.id)";
