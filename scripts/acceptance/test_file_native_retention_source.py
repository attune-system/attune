"""Source guards only. These do not execute Rust, PostgreSQL, or a provider."""
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]


class NativeFileRetentionSourceTests(unittest.TestCase):
    def setUp(self):
        self.retention = (ROOT / "crates/common/src/repositories/file_evidence_retention.rs").read_text()
        self.cache = (ROOT / "crates/common/src/repositories/cache.rs").read_text()
        self.migration = (ROOT / "migrations/20261015006013_file_native_physical_parents.sql").read_text()

    def test_native_inventory_is_keyless_typed_and_cumulative(self):
        storage = self.retention.split("async fn native_storage(", 1)[1].split("async fn expand(", 1)[0]
        self.assertIn("::storage_evidence(conn, snapshot)", storage)
        self.assertIn("evidence.physical_parents()", storage)
        self.assertIn("evidence.metadata().source_site()", storage)
        self.assertNotIn("receipt_context", storage)
        self.assertNotIn("source_evidence", storage)
        self.assertNotIn("split_once", storage)
        self.assertIn("c.charge_receipt(parents.retained_bytes(), &native)?", self.retention)
        self.assertIn("total.checked_sub(duplicate_bytes)", self.retention)

    def test_partition_drop_enters_graph_before_cache_and_checks_exact_pins(self):
        drop = self.migration.split("CREATE OR REPLACE FUNCTION drop_cleanup_cache_generation", 1)[1]
        graph = drop.index("PERFORM file_evidence_gc_entry()")
        check = drop.index("file_evidence_cache_generation_pinned(generation_id)")
        cache = drop.index("PERFORM pg_advisory_xact_lock(7821101,0)")
        ddl = drop.index("EXECUTE format('DROP TABLE")
        self.assertLess(graph, check)
        self.assertLess(check, cache)
        self.assertLess(cache, ddl)
        self.assertIn("source=e AND source_generation=g", self.migration)
        self.assertIn("source_created,source_generation", self.migration)
        self.assertNotIn("CREATE OR REPLACE FUNCTION delete_runtime_authority_admission", self.migration)

    def test_collectors_exclude_pins_before_limits(self):
        candidates = self.cache.split("pub async fn select_cleanup_candidates_after(", 1)[1].split("pub async fn drop_if_cleanup_eligible(", 1)[0]
        self.assertLess(candidates.index("NOT file_evidence_cache_generation_pinned(g.id)"), candidates.index("LIMIT $1"))
        namespaces = self.cache.split("pub async fn delete_empty_tombstoned_batch(", 1)[1].split("///", 1)[0]
        self.assertLess(namespaces.index("NOT file_evidence_pinned('cache_namespace',n.id)"), namespaces.index("LIMIT $1"))
        self.assertLess(namespaces.index("::lock_reclamation"), namespaces.index("DELETE FROM cache_namespace"))
        collectors = (ROOT / "crates/common/src/repositories/pack_retention.rs").read_text()
        for kind in ["runtime_input_receipt", "accepted_input", "runtime_native_input_snapshot"]:
            query = next(line for line in collectors.splitlines() if f"NOT file_evidence_pinned('{kind}'" in line)
            self.assertLess(query.index(f"NOT file_evidence_pinned('{kind}'"), query.index("LIMIT $1"))

    def test_complete_typed_parent_match_and_qualified_entries(self):
        for variant in ["Execution", "WorkflowExecution", "WorkflowTaskWait", "CacheIteration", "CacheNamespace", "CacheGeneration", "CacheEntry", "Inquiry", "QueueItem", "Pack", "Identity", "Action", "Sensor", "Event", "Receipt", "Admission", "Policy", "Approval", "NativeArchive", "NativeBudget"]:
            self.assertIn(f"P::{variant} {{", self.retention)
        self.assertIn("generation: Some(*generation)", self.retention)
        self.assertIn("created: Some(*created)", self.retention)
        self.assertNotIn("immutable_site_identity:", self.retention)

    def test_destructive_repository_roots_are_exclusive_before_cache_locks(self):
        for method in ["delete_tombstoned_if_empty", "delete_empty_tombstoned_batch"]:
            root = self.cache.split(f"pub async fn {method}(", 1)[1].split("Ok(result.", 1)[0]
            self.assertLess(root.index("::lock_reclamation"), root.index("DELETE FROM cache_namespace"))
            self.assertNotIn("protect_transaction", root[:root.index("::lock_reclamation")])
        generation = self.cache.split("pub async fn drop_if_cleanup_eligible(", 1)[1].split("pub async fn", 1)[0]
        self.assertLess(generation.index("::lock_reclamation"), generation.index("lock_cache_admission"))
        api = (ROOT / "crates/api/tests/support/recorded_file_completion_cases.rs").read_text()
        self.assertIn("async fn recorded_empty_cache_tombstone_can_be_physically_reclaimed_at_an_exclusive_outer_entry", api)
        self.assertIn("CacheNamespaceRepository::delete_tombstoned_if_empty", api)

    def test_repository_regression_covers_owned_physical_release(self):
        regression = (ROOT / "crates/common/tests/support/file_native_retention_cases.rs").read_text()
        for call in ["Snapshots::prepare_cache", "Snapshots::prepare_wait", "Snapshots::release_wait", "FileOriginRepository::reserve", "Evidence::begin_put", "Evidence::finish_put", "Evidence::readmit", "RuntimeAuthorityRepository::revoke_admission", "Evidence::object_removed", "Evidence::staging_removed", "PackRetentionRepository::collect"]:
            self.assertIn(call, regression)
        self.assertIn("source_generation=$2", regression)
        self.assertNotIn("RuntimeReadParent {", regression)
        self.assertNotIn("INSERT INTO runtime_input_receipt", regression)
        final_record = regression.index("let receipt = Inputs::record_prepared")
        self.assertIn("let released_wait", regression[:final_record])
        self.assertIn("cache.ensure_fresh()", regression[final_record:])
        self.assertIn("wait.ensure_fresh()", regression[final_record:])


if __name__ == "__main__":
    unittest.main()
