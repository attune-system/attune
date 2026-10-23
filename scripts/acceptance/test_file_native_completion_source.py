"""Static adapter guards only. Never imports or runs Rust/PG/kernel fixtures."""
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]


class NativeCompletionRetentionSourceTests(unittest.TestCase):
    def setUp(self):
        source = (ROOT / "crates/common/src/repositories/file_evidence_retention.rs").read_text()
        self.native = source.split("async fn native_storage(", 1)[1].split("async fn expand(", 1)[0]
        self.compact = "".join(self.native.split())
        self.tests = (ROOT / "crates/common/tests/support/file_native_completion_cases.rs").read_text()

    def test_typed_children_register_original_sites_before_origin_discovery(self):
        registration = self.compact.index("forchildinevidence.completion_sources()")
        discovery = self.compact.index("origins:evidence.origins().to_vec()")
        self.assertLess(registration, discovery)
        self.assertIn('entity_type:"execution_result".into(),entity_id:child.execution(),path:String::new()',
                      self.compact)
        self.assertIn("if!c.archived_sites.contains(&site){c.archived_sites.push(site);}", self.compact)

    def test_each_original_contract_uses_existing_proof_graph_and_shared_budget(self):
        self.assertIn("ifletSome(contract)=child.original_contract(){c.proof(contract,depth+1)?;}", self.compact)
        self.assertIn("+evidence.completion_sources().len()", self.compact)
        self.assertIn("letbytes=evidence.retained_bytes()", self.compact)
        self.assertIn("c.native_bytes.len()>=MAX_NATIVE_SNAPSHOTS", self.compact)
        self.assertIn("c.native_predecessors.insert(snapshot,predecessor)", self.compact)
        self.assertIn("forparentinevidence.physical_parents()", self.compact)
        self.assertNotIn("restore_secret_values", self.native)

    def test_no_future_kind_or_synthetic_child_snapshot_dispatch(self):
        self.assertNotIn('source_kind() == "workflow_failure_metadata"', self.native)
        self.assertNotIn('source_kind() == "cache', self.native)
        self.assertNotIn("NativeCompletionParent", self.native)
        self.assertNotIn('Node::new("completion', self.native)
        self.assertIn('evidence.archive_owner() != ("native_input_snapshot", snapshot)', self.native)
        self.assertNotIn("KeyRepository", self.native)

    def test_actual_repository_fixture_seals_original_input_before_running(self):
        for required in ("ExecutionStatus::Requested", "record_admission", "materialize_action_pair",
                         "initialize_config_with_conn", "capture_human", "seal_execution",
                         "ExecutionStatus::Running", "Secrets::upsert_many_with_conn"):
            self.assertIn(required, self.tests)
        lifecycle = self.tests.split("for index in 0..3", 1)[1]
        self.assertLess(lifecycle.index("seal_execution"), lifecycle.index("ExecutionStatus::Running"))
        self.assertIn("redact_bound_action_result(", self.tests)
        self.assertIn('original_fields[0].json_path,"/data/token"', "".join(self.tests.split()))
        self.assertIn('original_fields[0].source_kind,"provenance"', "".join(self.tests.split()))
        self.assertIn('SecretSource::ParameterSchema{path:"/token".into()}.provenance()', "".join(self.tests.split()))
        self.assertNotIn("SecretValueInput {", self.tests)
        self.assertIn("mapping.source_path()", self.tests)
        self.assertIn("Inputs::prepare_outcome", self.tests)
        self.assertIn('prior.value("failed")', self.tests)
        self.assertLess(self.tests.index("WorkflowExecutionRepository::update"),
                        self.tests.index("Failures::prepare_for_completion"))
        self.assertIn("FFinalFailureFactoryDeclinedAfterPriorAdvancement", self.tests)

    def test_raw_children_are_removed_before_initial_file_capture_and_keyless_readmission(self):
        compact = "".join(self.tests.split())
        self.assertLess(compact.index("ExecutionRepository::delete(&mut*tx,child.id)"),
                        compact.index("hold(&db,&fixture,receipt"))
        for required in ("native_identity:execution", "runtime_authority_admission", "Evidence::readmit",
                         "raw_pins", "archive_count"):
            self.assertIn(required, self.tests)

    def test_missing_release_contract_corruption_and_exact_physical_reclamation_are_authored(self):
        for required in ("snapshot_negatives", "runtime_native_result_view_release", "runtime_native_completion_release",
                         "corrupt_contract", "value_schema::digest", "Evidence::request_cleanup",
                         "store.delete", "Evidence::object_removed", "Evidence::staging_removed",
                         "PackRetentionRepository::collect", "release_count"):
            self.assertIn(required, "".join(self.tests.split()) if required == "store.delete" else self.tests)
        self.assertIn("evidence.wait_release().is_none()", self.tests)
        self.assertIn("evidence.result_view_release().is_none()", self.tests)
        self.assertIn("evidence.completion_release().is_none()", self.tests)
        crash = (ROOT / "crates/common/tests/file_evidence_retention_tests.rs").read_text().split(
            "async fn crash_after_native_put_and_restart_before_receipt_keeps_pins_until_provider_fence()", 1)[1]
        self.assertLess(crash.index("Evidence::lock_capture(&mut admission)"),
                        crash.index("INSERT INTO file_evidence_put"))
        self.assertLess(crash.index("admission.commit()"), crash.index(".put_file_atomic("))
        api = (ROOT / "crates/api/src/routes/internal_files.rs").read_text().split(
            "async fn reserve_proxy_artifact_upload(", 1)[1].split("\nasync fn ", 1)[0]
        self.assertLess(api.index("::lock_capture("), api.index("authorize_version_mutation("))
        self.assertLess(api.index("::lock_capture("), api.index("::begin_put("))


if __name__ == "__main__":
    unittest.main()
