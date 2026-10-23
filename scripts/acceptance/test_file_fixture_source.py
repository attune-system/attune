"""Guard fixture construction order; no API or database acceptance implied."""
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]


class AcceptedFileFixtureSourceTests(unittest.TestCase):
    def test_original_key_consent_and_ciphertext_precede_seal(self):
        source = (ROOT / "crates/api/tests/recorded_execution_events_tests.rs").read_text()
        entry = "fixture_with_original_input_deadlines" if "async fn fixture_with_original_input_deadlines(" in source else "fixture_with_original_input"
        builder = source.split(f"async fn {entry}(", 1)[1].split("Ok(Fixture", 1)[0]
        self.assertLess(builder.index("KeyRepository::create"), builder.index("::capture_human"))
        self.assertLess(builder.index('"fixture.original_file_input"'), builder.index("::capture_human"))
        self.assertLess(builder.index("::capture_human"), builder.index("::upsert_many_with_conn"))
        self.assertLess(builder.index("::upsert_many_with_conn"), builder.index("::seal_execution"))
        self.assertEqual(builder.count("::seal_execution"), 1)

    def test_file_b_never_overwrites_an_accepted_input(self):
        source = (ROOT / "crates/api/tests/support/recorded_file_completion_cases.rs").read_text()
        case = source.split("async fn protected_a_to_file_b_retains_sources_encrypts_and_denies_public_bypass()", 1)[1].split("#[tokio::test]", 1)[0]
        self.assertIn('Some(("file_source", "retained-secret-file-bytes"))', case)
        self.assertIn("fixture_with_original_input(", case)
        self.assertNotIn("upsert_many_with_conn", case)
        self.assertNotIn("seal_execution", case)

    def test_file_b_uses_canonical_key_api_identity_instead_of_an_unqualified_lookup(self):
        source = (ROOT / "crates/api/tests/support/recorded_file_completion_cases.rs").read_text()
        case = source.split("async fn protected_a_to_file_b_retains_sources_encrypts_and_denies_public_bypass()", 1)[1].split("#[tokio::test]", 1)[0]
        self.assertIn("key_ref::canonical_key_ref", case)
        self.assertIn('"/api/v1/keys/{key_ref}"', case)
        for path in ["/data/ref", "/data/local_ref", "/data/owner_pack", "/data/encrypted", "/data/id"]:
            self.assertIn(f'"{path}"', case)
        self.assertNotIn("KeyRepository::find_by_ref", case)
        self.assertIn("positive /data/id", case)

    def test_ready_read_control_uses_a_distinct_scoped_reader_without_widening_producer(self):
        source = (ROOT / "crates/api/tests/support/recorded_file_completion_cases.rs").read_text()
        case = source.split("async fn pending_destination_is_not_an_acquired_read_during_process_file_publication()", 1)[1].split("#[tokio::test]", 1)[0]
        self.assertIn("let fixture = fixture(true)", case)
        self.assertNotIn("fixture_with_additional_grants", case)
        self.assertIn('"fixture.pending_file_reader"', case)
        self.assertIn("generate_access_token(reader.id", "".join(case.split()))
        self.assertIn("Some(&reader_token)", case)
        self.assertIn('"constraints":{"ids":[artifact]}', case)
        self.assertIn('"constraints":{"ids":[fixture.credential.execution_id]}', case)
        self.assertIn("StatusCode::FORBIDDEN", case)


if __name__ == "__main__":
    unittest.main()
