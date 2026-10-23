"""Source-only checks for ct071's publication/read purpose distinction."""
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]


class PublicationSourceTests(unittest.TestCase):
    def test_publication_authorizes_frozen_sources_not_its_pending_sink_as_a_read(self):
        source = (ROOT / "crates/api/src/file_origins.rs").read_text()
        preparation = source.split("pub(crate) async fn prepare_existing(", 1)[1].split("/// Recheck an existing capture", 1)[0]
        process = preparation.split("let budget = if let Some(authority) = authority {", 1)[1].split("} else {", 1)[0]
        self.assertIn("FileOriginRepository::retained_sources(conn, &retained)", process)
        self.assertIn("authorize_origins_with_key(conn, authority, &rows, key)", process)
        self.assertNotIn("RuntimeReadRepository::reference(retained.origin_read)", process)
        # Ordinary readers must still reject Pending bodies. No C bypass.
        disclosure = (ROOT / "crates/api/src/recorded_disclosure.rs").read_text()
        self.assertIn("ArtifactBodyState::Pending", disclosure)
        delivery = source.split("pub(crate) async fn authorize_delivery(", 1)[1].split("async fn target(", 1)[0]
        self.assertIn("row(captured.clone())", delivery)
        self.assertIn("require_ready_body_on", delivery)
        self.assertIn("FileOriginRepository::require_sources", source)

    def test_original_sites_are_bound_to_the_exact_reservation_digest_and_sink(self):
        source = (ROOT / "crates/common/src/repositories/file_origin.rs").read_text()
        self.assertIn("id=$1 AND admission=$2 AND file_sink_version=$3", source)
        decode = source.split("fn decode_retained_sources(", 1)[1].split("/// A retained plaintext", 1)[0]
        self.assertIn("retained.source_digest", decode)
        self.assertIn("MAX_READ_BYTES", decode)
        self.assertIn("rows.len() > 256", decode)
        self.assertIn("entity_type: row.entity_type", decode)
        self.assertIn("entity_id: row.entity_id", decode)
        self.assertIn("json_path: row.json_path", decode)
        self.assertNotIn("execution_origins", decode)
        self.assertNotIn("RuntimeReadParent", decode)

    def test_six_cases_do_not_overwrite_accepted_input_secret_rows(self):
        source = (ROOT / "crates/api/tests/support/recorded_file_completion_cases.rs").read_text()
        for name in [
            "by_ref_initial_upload_keeps_create_only_authority_distinct_from_update",
            "generic_read_between_external_put_and_ready_commit_denies_process_file_effect",
            "key_only_sdk_read_encrypts_public_multipart_file_and_retains_key_receipt",
            "protected_a_to_file_b_retains_sources_encrypts_and_denies_public_bypass",
            "protected_file_chunks_and_successful_eof_recheck_source_and_ready_pin",
            "recorded_file_allocations_bind_worker_delivery_and_reject_impersonation",
        ]:
            case = source.split(f"async fn {name}(", 1)[1].split("#[tokio::test]", 1)[0]
            self.assertNotIn("upsert_many_with_conn", case, name)
            self.assertNotIn("replace_entity_with_conn", case, name)
        self.assertIn("pending_destination_is_not_an_acquired_read_during_process_file_publication", source)


if __name__ == "__main__":
    unittest.main()
