"""Static/mock adapter regressions. No PostgreSQL, SQLite, native or API jobs."""
import ast
from pathlib import Path
import unittest
from unittest.mock import Mock

from controller import Controller
from protocol import Quarantine

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
OP = "00000000-0000-4000-8000-000000000001"


class NativeAdapterSourceTests(unittest.TestCase):
    def setUp(self):
        self.repo = (ROOT / "crates/common/src/repositories/file_native_setup.rs").read_text()
        self.compact = "".join(self.repo.split())

    def test_real_initial_lifecycle_and_schema_precede_running_and_file_reservation(self):
        source = self.repo.split("pub async fn setup", 1)[1]
        steps = ["Evidence::lock_capture", "Self::authorize", "resolve_current_snapshot",
                 "ExecutionRepository::create_pinned", "RuntimeAuthorityRepository::record_admission",
                 "materialize_action_pair", "SchemaContractRepository::attach",
                 "initialize_config_with_conn", "::capture_human", "::seal_execution",
                 "ExecutionStatus::Running", "WorkerDeliveryRepository::register",
                 "AcceptedInputRepository::prepare_source", "ArtifactRepository::create",
                 "FileOriginRepository::reserve", "INSERT INTO file_native_operation",
                 "INSERT INTO file_evidence_put"]
        indices = [source.index(step) for step in steps]
        self.assertEqual(indices, sorted(indices))
        self.assertIn("status:ExecutionStatus::Requested,config:None", self.compact)
        self.assertIn("source.ensure_fresh()?;receipt.ensure_fresh()?;effect.ensure_fresh()?", self.compact)

    def test_actual_accepted_receipt_not_a_fabricated_read_parent_or_key_origin(self):
        self.assertIn("redact_secret_parameters", self.repo)
        self.assertIn("receipt.provenance()", self.repo)
        self.assertIn("parents:vec![]", self.compact)
        self.assertNotIn("RuntimeReadParent {", self.repo)
        self.assertNotIn("KeyRepository", self.repo)
        self.assertIn("source_receipt", self.repo)

    def test_no_synthetic_legacy_history_or_successful_provider_observation(self):
        self.assertNotIn("'legacy_unknown'", self.repo)
        self.assertIn("'controlled_native_v1'", self.repo)
        self.assertNotIn("UPDATE execution SET config", self.repo)
        self.assertNotIn("mark_object_ready", self.repo)
        self.assertIn("provider_version:None", self.compact)
        self.assertTrue("access.mode.blob()" in self.compact)
        self.assertTrue("access.mode.shared()" in self.compact)

    def test_original_nonce_comes_only_from_authenticated_native_enrollment(self):
        source = self.repo.split("pub async fn setup", 1)[1]
        compact = "".join(source.split())
        self.assertLess(compact.index("authority.attempt=origin.attempt"),
                        compact.index("RuntimeAuthorityRepository::record_admission"))
        self.assertIn("runtime_authority_execution_fence WHERE attempt=$1", self.repo)
        verifier = (ROOT / "crates/common/src/repositories/native_generation_origin.rs").read_text()
        for term in ("controlled-native-origin", "RSA_PKCS1_2048_8192_SHA256", "request_digest",
                     "nonce", "pub struct VerifiedNativeOrigin", "generation"):
            self.assertIn(term, verifier)
        self.assertNotIn("Deserialize)]\npub struct VerifiedNativeOrigin", verifier)

    def test_ordinary_exact_audit_path_is_extended_not_skipped(self):
        migration = (ROOT / "migrations/20261016006015_controlled_native_file_operations.sql").read_text()
        for term in ("o.evidence_state='retained'", "c.attempt=NEW.execution_attempt",
                     "pin.kind='runtime_input_receipt'", "a.plan->'scope'=g.scope", "member=n.member",
                     "a.state='completed'", "e.kind='settled'", "OLD.protocol='atomic_create_v1'",
                     "OLD.protocol='legacy_unknown'"):
            self.assertIn(term, migration)
        guard = migration.split("CREATE OR REPLACE FUNCTION file_evidence_guard_put_retirement", 1)[1]
        self.assertLess(guard.index("OLD.protocol='controlled_native_v1'"), guard.index("file_evidence_put_observation"))
        audit = (ROOT / "crates/common/src/repositories/file_legacy_audit.rs").read_text()
        self.assertIn("original != serde_json::to_value(member)?", audit)
        self.assertIn("scope != serde_json::to_value(&plan.scope)?", audit)

    def test_example_is_typed_private_and_repository_only(self):
        source = (ROOT / "crates/common/examples/native_settlement_adapter.rs").read_text()
        self.assertNotIn("sqlx::query", source)
        for term in ("ATTUNE_NATIVE_ACCEPTANCE_CONTEXT", "validate_token", "context.access.validate",
                     "NativeFileSetupRepository::setup", "NativeFileSetupRepository::inspect",
                     "libc::O_NOFOLLOW", "--execute"):
            self.assertIn(term, source)
        self.assertIn('"prepare-original-native-producer"', self.repo)
        self.assertIn('"observe-original-native-custody"', self.repo)
        self.assertIn("version.is_some_and", self.repo)

    def test_setup_role_cannot_inherit_or_hold_cluster_administration(self):
        for field in ("rolsuper", "rolcreatedb", "rolcreaterole", "rolreplication", "rolbypassrls"):
            self.assertIn("r." + field, self.repo)
        self.assertIn("pg_auth_members m WHERE m.member=r.oid", self.repo)
        self.assertIn("database != access.database_name", self.repo)
        self.assertIn("role != access.database_role", self.repo)
        self.assertNotIn("GRANT ", self.repo)
        self.assertNotIn("SET ROLE", self.repo)

    def test_inspection_reads_actual_owners_pins_puts_and_controller_receipts(self):
        for term in ("file_evidence_owner WHERE version=$1", "file_evidence_pin WHERE owner=$1",
                     "file_evidence_put WHERE owner=$1", "file_legacy_audit_evidence",
                     "controller_receipts", "plan_matches_original", "evidence_owner_count"):
            self.assertIn(term, self.repo)
        self.assertNotIn("HEAD", self.repo)
        self.assertNotIn("placeholder", self.repo)

    def test_fixture_action_never_prints_source(self):
        source = (HERE / "producer_pack/actions/producer.py").read_text()
        ast.parse(source)
        self.assertNotIn("print(value", source)
        self.assertNotIn("print(raw", source)
        yaml = (HERE / "producer_pack/actions/producer.yaml").read_text()
        self.assertIn("secret: true", yaml)
        self.assertIn("parameter_delivery: stdin", yaml)


class NativeEnrollmentMockTests(unittest.TestCase):
    def setUp(self):
        self.journal, self.native = Mock(), Mock()
        self.native.binding.return_value = {"roots": {}, "cgroup": {}}
        self.journal.binding.return_value = self.native.binding.return_value
        self.native.cgroup_read.side_effect = lambda key: "0" if key == "cgroup.freeze" else ""
        self.native.scan.return_value = []
        config = {"generation": OP, "scope": {}, "principals": {"f" * 64:
                  {"role": "writer", "native_instance": OP, "worker_instance": OP, "execution_attempt": OP}}}
        self.controller = Controller(config, self.journal, self.native)
        self.request = {"nonce": OP, "generation": OP, "run_id": "owned-run",
                        "project": "eaccept-owned-run-native-blob", "mode": "blob"}

    def test_pristine_native_observations_precede_enrollment(self):
        self.journal.enroll.return_value = {"operation": OP}
        result = self.controller.prepare_origin(self.request)
        self.native.exclusive_processes.assert_called_once()
        self.native.scan.assert_called_once()
        self.journal.enroll.assert_called_once()
        self.assertEqual(result["enrollment"]["operation"], OP)

    def test_preexisting_physical_body_cannot_be_enrolled(self):
        self.native.scan.return_value = [{"old": "body"}]
        with self.assertRaisesRegex(Quarantine, "OriginalGenerationNotPristine"):
            self.controller.prepare_origin(self.request)
        self.journal.enroll.assert_not_called()

    def test_a_live_or_frozen_writer_cannot_be_relabelled_as_original(self):
        self.native.cgroup_read.side_effect = lambda key: "123" if key == "cgroup.procs" else "0"
        with self.assertRaisesRegex(Quarantine, "OriginalGenerationNotPristine"):
            self.controller.prepare_origin(self.request)
        self.journal.enroll.assert_not_called()


if __name__ == "__main__":
    unittest.main()
