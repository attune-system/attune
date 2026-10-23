"""Static F typed group adapter guards, not Rust/DB/kernel acceptance."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class FileIterationOutcomeSourceTests(unittest.TestCase):
    def setUp(self):
        self.source = (ROOT / "crates/common/src/repositories/file_evidence_retention.rs").read_text()
        native = self.source.split("async fn native_storage(", 1)[1].split("async fn expand(", 1)[0]
        self.group = native.split("if let Some(group) = evidence.iteration_outcome()", 1)[1].split(
            "// Requires A's finite completion", 1)[0]
        self.compact = "".join(self.group.split())
        self.tests = (ROOT / "crates/common/tests/support/file_native_iteration_outcome_cases.rs").read_text()

    def test_every_original_and_latest_input_observation_is_visited(self):
        self.assertIn("forobservedin[member.original(),member.latest()]", self.compact)
        self.assertIn("letfields=observed.original_input_fields()", self.compact)
        self.assertIn('entity_type:"execution_config".into(),entity_id:observed.execution(),path:field.json_path.clone()', self.compact)
        self.assertNotIn('entity_type:"execution_result"', self.compact)
        self.assertNotIn("/children/", self.group)

    def test_full_original_schemas_and_each_receipt_graph_use_existing_hooks(self):
        for term in ("group.original_schema()", "observed.original_schema()", "observed.input_receipt_edges()",
                     "group.receipt_edges().iter().chain(group.list_controls())", "observed.admission()",
                     "c.origins(&provenance,depth+1)"):
            self.assertIn(term, self.compact)
        self.assertIn("serde_json::from_str", self.group)
        self.assertNotIn("restore_secret_values", self.group)
        self.assertNotIn("KeyRepository", self.group)

    def test_independent_manifests_count_without_grant_union_or_duplicate_ciphertext_charge(self):
        self.assertIn("observed.contributor_manifests().len()", self.compact)
        self.assertIn("c.rows(1+fields.len()", self.compact)
        self.assertNotIn("c.charge(", self.group)
        self.assertNotIn(".grants", self.group)
        self.assertNotIn("manifest.body", self.group)
        self.assertNotIn("NativeSnapshot{", self.compact)

    def test_exact_archive_origin_discharge_is_only_inside_typed_storage_branch(self):
        self.assertIn('entity_type:"ordinary_iteration_outcome".into(),entity_id:snapshot,path:String::new()', self.compact)
        self.assertIn("evidence.origins().contains(&own_origin)", self.compact)
        self.assertIn("c.iteration_archive_origins.push(own_origin)", self.compact)
        self.assertIn("pending_iteration_origins", self.source)
        self.assertIn("!closure.iteration_archive_origins.contains(origin)", self.source)
        self.assertNotIn('"ordinary_iteration_outcome" => "execution"', self.source)

    def test_real_d_seals_and_a_group_source_precede_raw_gc_and_f_reservation(self):
        compact = "".join(self.tests.split())
        for term in ("Materialize::seal_execution_inputs", "Iteration::prepare", "plan.apply",
                     "Inputs::prepare_native_iteration_outcome", "attach_native_iteration_control"):
            self.assertIn(term, self.tests)
        self.assertLess(compact.index("ExecutionRepository::delete(&mut*tx,*id)"),
                        compact.index("hold(&db,&fixture,&receipt"))
        self.assertIn("workflow_task_dispatch", self.tests)
        self.assertIn("Secrets::find_by_entity", self.tests)

    def test_typed_normalized_records_and_physical_custody_are_observed_not_asserted_inputs(self):
        for term in ("runtime_native_iteration_outcome_member", "runtime_native_iteration_outcome_release",
                     "storage.archive_fields().is_empty()", "storage.contributor_manifests().is_empty()",
                     "storage.completion_sources().is_empty()", "revoke_admission", "receipt_context",
                     "Evidence::readmit", "Evidence::object_removed", "Evidence::staging_removed",
                     "PackRetentionRepository::collect"):
            self.assertIn(term, self.tests)


if __name__ == "__main__":
    unittest.main()
