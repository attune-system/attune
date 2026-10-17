"""Prospective custody source assertions, not Rust/DB/provider acceptance."""
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]


class ProspectiveCustodySourceTests(unittest.TestCase):
    def setUp(self):
        self.retention = (ROOT / "crates/common/src/repositories/file_evidence_retention.rs").read_text()
        self.forward = (ROOT / "migrations/20261015006014_file_prospective_native_custody.sql").read_text()

    def test_native_predecessor_is_never_a_receipt(self):
        self.assertIn('P::NativeSnapshot { snapshot } => ("runtime_native_input_snapshot", *snapshot)', self.retention)
        self.assertIn("const MAX_NATIVE_SNAPSHOTS: usize = 128;", self.retention)
        storage = self.retention.split("async fn native_storage(", 1)[1].split("async fn expand(", 1)[0]
        self.assertIn("c.native_predecessors.insert(snapshot, predecessor)", storage)
        self.assertIn("c.native_bytes.len() >= MAX_NATIVE_SNAPSHOTS", storage)
        self.assertIn("cache.iteration()", storage)
        self.assertNotIn("split_once", storage)
        self.assertNotIn("body#", storage)

    def test_cumulative_native_bytes_are_not_recharged_per_inventory(self):
        accounting = self.retention.split("fn charge_native(", 1)[1].split("fn pop(", 1)[0]
        self.assertIn(".checked_sub(inherited)", accounting)
        self.assertIn("self.charged_native.insert(id)", accounting)
        self.assertIn("bytes.checked_sub(added)", accounting)
        self.assertIn("total.checked_sub(duplicate_bytes)", accounting)
        self.assertIn("self.native_bytes.get(&snapshot) != Some(&bytes)", accounting)
        expansion = self.retention.split('if node.kind == "runtime_native_input_snapshot" {', 1)[1].split("if matches!(", 1)[0]
        self.assertIn("c.charge_native(node.id)", expansion)
        self.assertNotIn("charged_native.insert(node.id)", expansion)

    def test_forward_preserves_epoch_history_and_actual_normalized_associations(self):
        self.assertIn("DROP CONSTRAINT runtime_cache_iteration_epoch_iteration_fkey", self.forward)
        self.assertIn("f.bound_iteration=i", self.forward)
        self.assertIn("file_evidence_pinned('runtime_native_input_snapshot',f.snapshot)", self.forward)
        self.assertIn("BEFORE DELETE ON runtime_native_cache_frontier", self.forward)
        self.assertIn("BEFORE DELETE ON runtime_cache_iteration_epoch", self.forward)
        self.assertNotIn("body#", self.forward)
        self.assertNotIn("CREATE OR REPLACE FUNCTION delete_runtime_authority_admission", self.forward)
        self.assertNotIn("CREATE OR REPLACE FUNCTION drop_cleanup_cache_generation", self.forward)

    def test_collectors_exclude_predecessor_and_epoch_pins_before_limits(self):
        collectors = (ROOT / "crates/common/src/repositories/pack_retention.rs").read_text()
        query = next(line for line in collectors.splitlines() if "DELETE FROM runtime_native_input_snapshot " in line)
        self.assertLess(query.index("frontier.predecessor=input.id"), query.index("LIMIT $1"))
        self.assertLess(query.index("file_evidence_pinned('runtime_native_input_snapshot'"), query.index("LIMIT $1"))
        epoch = next(line for line in self.retention.splitlines() if "DELETE FROM runtime_cache_iteration_epoch " in line)
        self.assertLess(epoch.index("NOT file_evidence_cache_epoch_pinned"), epoch.index("LIMIT $1"))
        self.assertLess(epoch.index("f.bound_iteration=epoch.iteration"), epoch.index("LIMIT $1"))
        drop = (ROOT / "migrations/20261015006013_file_native_physical_parents.sql").read_text()
        self.assertLess(drop.index("PERFORM file_evidence_gc_entry()"), drop.index("PERFORM pg_advisory_xact_lock"))
        self.assertIn("OR EXISTS(SELECT 1 FROM file_evidence_pin WHERE kind='cache_entry' AND source_generation=g)", self.forward)

    def test_repository_regression_prepares_initial_and_linked_next_before_actual_targets(self):
        case = (ROOT / "crates/common/tests/support/file_native_prospective_cases.rs").read_text()
        for call in ["prepare_cache_initial", "prepare_cache_next", "prepare_destination"]:
            self.assertLess(case.index(f"::{call}"), case.index("::protect_transaction"))
        self.assertIn("initial.initial_create_input()", case)
        self.assertIn("::update_scan_progress", case)
        self.assertIn("::bind_cache_frontier", case)
        self.assertIn("DELETE FROM workflow_cache_iteration WHERE id=$1", case)
        self.assertIn("::readmit", case)
        self.assertIn("retained.metadata(), &metadata", case)
        self.assertIn("creation history releases only after all physical/frontier owners settle", case)


if __name__ == "__main__":
    unittest.main()
