"""Source guards for ct085's native fixture prerequisite; no DB execution."""
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]


class NativeFixtureInitializationSourceTests(unittest.TestCase):
    def setUp(self):
        self.source = (ROOT / "crates/common/tests/support/file_native_retention_cases.rs").read_text()
        self.fixture = self.source.split("async fn native_storage_custody_case(", 1)[1]

    def test_actual_admission_and_complete_schema_binding_precede_initialization(self):
        admission = self.fixture.index("RuntimeAuthorityRepository::record_admission(")
        materialize = self.fixture.index("SchemaContractRepository::materialize_action_pair(")
        attach = self.fixture.index("SchemaContractRepository::attach(")
        initialize = self.fixture.index("ExecutionRepository::initialize_config_with_conn(")
        capture = self.fixture.index("::capture_human(")
        seal = self.fixture.index("::seal_execution(")
        self.assertLess(admission, materialize)
        self.assertLess(materialize, attach)
        self.assertLess(attach, initialize)
        self.assertLess(initialize, capture)
        self.assertLess(capture, seal)
        self.assertEqual(self.fixture.count("SchemaContractRepository::attach("), 1)

    def test_contract_is_materialized_from_the_actual_snapshot_not_fabricated(self):
        compact = "".join(self.fixture.split())
        self.assertIn("schema.input.authored_digest", compact)
        self.assertIn("schema.output.as_ref()", compact)
        self.assertIn("action.param_schema", compact)
        self.assertIn("action.out_schema", compact)
        self.assertIn("SourceIdentity::Released", self.fixture)
        self.assertIn("schema.validate()", self.fixture)
        self.assertIn("SchemaContractRepository::find(", self.fixture)
        self.assertIn("stored, schema", self.fixture)
        self.assertNotIn("INSERT INTO runtime_schema_binding", self.fixture)

    def test_one_shot_initialization_uses_exact_admission_before_running_and_seal(self):
        creation = self.fixture.split("let root = ExecutionRepository::create_pinned(", 1)[1].split("let workflow =", 1)[0]
        self.assertIn("config: None", creation)
        self.assertIn("status: ExecutionStatus::Requested", creation)
        initialize = self.fixture.split("ExecutionRepository::initialize_config_with_conn(", 1)[1].split("let source_admission", 1)[0]
        self.assertIn("Some(root_admission.id)", initialize)
        self.assertIn("&executables[&graph_action.r#ref]", initialize)
        self.assertIn('"target_execution":source.id', initialize)
        self.assertIn("&[]", initialize)
        self.assertNotIn('config: Some(json!({"target_execution"', self.fixture)
        running = self.fixture.index("status: Some(ExecutionStatus::Running)")
        self.assertLess(self.fixture.index("::seal_execution("), running)
        self.assertLess(running, self.fixture.index("let wait = WorkflowTaskWaitRepository"))

    def test_persisted_and_prospective_cases_share_the_fixed_prerequisite(self):
        self.assertIn("native_storage_custody_case(false).await", self.source)
        prospective = (ROOT / "crates/common/tests/support/file_native_prospective_cases.rs").read_text()
        self.assertIn("native_storage_custody_case(true).await", prospective)

    def test_source_is_not_terminal_until_its_original_input_was_sealed(self):
        creation = self.fixture.split("let source = ExecutionRepository::create_pinned(", 1)[1].split("let grants =", 1)[0]
        compact = "".join(creation.split())
        self.assertIn("config:Some(json!({}))", compact)
        self.assertIn("result:None", compact)
        self.assertIn("status:ExecutionStatus::Requested", compact)
        self.assertNotIn("ExecutionStatus::Completed", creation)
        seal = self.fixture.index("::seal_execution(")
        running = self.fixture.index("status: Some(ExecutionStatus::Running)")
        ciphertext = self.fixture.index("::upsert_many_with_conn(")
        completed = self.fixture.index("status: Some(ExecutionStatus::Completed)")
        self.assertLess(seal, running)
        self.assertLess(running, ciphertext)
        self.assertLess(ciphertext, completed)
        self.assertLess(completed, self.fixture.index("let wait = WorkflowTaskWaitRepository"))

    def test_result_is_published_to_the_original_source_not_resealed_as_input(self):
        completion = self.fixture.split("status: Some(ExecutionStatus::Completed)", 1)[1].split("let wait = WorkflowTaskWaitRepository", 1)[0]
        self.assertIn('result: Some(json!({"token":attune_common::secret_values::redaction_marker()}))', completion)
        self.assertNotIn("::seal_execution(", completion)
        ciphertext = self.fixture.split("let fields =", 1)[1].split("status: Some(ExecutionStatus::Completed)", 1)[0]
        self.assertIn("key_access::key_origin(&original_key)", ciphertext)
        self.assertIn("ENTITY_EXECUTION_RESULT,source.id,&fields", "".join(ciphertext.split()))
        self.assertNotIn("ENTITY_EXECUTION_CONFIG", ciphertext)

    def test_file_producer_has_original_schema_and_acceptance_before_dispatch(self):
        self.assertIn("let (db, fixture) = native_file_producer_fixture().await;", self.fixture)
        producer = self.source.split("async fn native_file_producer_fixture()", 1)[1].split("async fn hold(", 1)[0]
        steps = ["ExecutionRepository::create_pinned(",
                 "RuntimeAuthorityRepository::record_admission(",
                 "SchemaContractRepository::materialize_action_pair(",
                 "SchemaContractRepository::attach(",
                 "SchemaContractRepository::find(",
                 "ExecutionRepository::initialize_config_with_conn(",
                 "::capture_human(", "::seal_execution(",
                 "status: Some(ExecutionStatus::Running)", "tx.commit()"]
        positions = [producer.index(step) for step in steps]
        self.assertEqual(positions, sorted(positions))
        self.assertIn("config: None", producer)
        self.assertIn("status: ExecutionStatus::Requested", producer)
        self.assertIn("Some(admission.id)", producer)
        self.assertIn("assert_eq!(stored, schema)", producer)
        self.assertIn("fixture.credential = RuntimeCredentialBinding", producer)
        self.assertIn("execution_id: producer.id", producer)
        self.assertIn("fixture.admission = admission", producer)
        self.assertNotIn("sqlx::query", producer)

    def test_keeper_still_retains_the_actual_producer_execution_and_schema(self):
        hold = self.source.split("async fn hold(", 1)[1].split("async fn native_storage_custody_case(", 1)[0]
        self.assertIn("Some(fixture.credential.execution_id)", hold)
        self.assertIn('"execution":fixture.credential.execution_id', hold)
        reserve = hold.split("FileOriginRepository::reserve(", 1)[1].split(".await", 1)[0]
        self.assertIn("fixture.credential.admission_id", reserve)
        self.assertIn("fixture.credential.execution_id", reserve)
        keeper = (ROOT / "crates/common/src/repositories/file_evidence_retention.rs").read_text()
        self.assertIn('if node.kind == "execution"', keeper)
        self.assertIn("FROM runtime_schema_binding WHERE entity_kind='execution' AND entity_id=$1", keeper)
        self.assertIn("let contract = contract.ok_or_else(denied)?", keeper)


if __name__ == "__main__":
    unittest.main()
