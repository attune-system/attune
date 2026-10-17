"""CENTRAL ONLY: real SQLite crash/reopen tests, not native/provider acceptance.

Run explicitly after authorization: python3 -m unittest runtime_tests
These tests own a temporary journal only. They do not start services or cgroups.
"""
import copy
from pathlib import Path
import tempfile
import unittest

from journal import Journal
from protocol import Quarantine, digest
from test_controller_source import ENTRY, PLAN, OP, physical
from controller import catalogue_items


class DurableJournalTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="attune-native-journal-")
        self.path = Path(self.temporary.name) / "journal.sqlite3"
        self.journal = Journal(self.path)
        self.journal.bind({"generation": OP}, initialize=True)

    def tearDown(self):
        self.journal.db.close()
        self.temporary.cleanup()

    def reopen(self):
        self.journal.db.close()
        self.journal = Journal(self.path)
        self.journal.bind({"generation": OP})

    def test_accepted_before_exec_never_restarts_after_crash(self):
        self.assertTrue(self.journal.admit(ENTRY, "blob", "digest"))
        self.reopen()
        self.assertFalse(self.journal.admit(ENTRY, "blob", "digest"))

    def test_changed_payload_cannot_reuse_operation_identity(self):
        self.journal.admit(ENTRY, "blob", "original")
        self.reopen()
        with self.assertRaisesRegex(Quarantine, "RequestIdentityRebound"):
            self.journal.admit(ENTRY, "blob", "replacement")

    def test_fence_committed_before_kill_survives_reopen(self):
        self.journal.admit(ENTRY, "blob", "digest")
        self.journal.fence(PLAN)
        self.reopen()
        second = dict(ENTRY, operation="00000000-0000-4000-8000-000000000002",
                      object_key="artifacts/2/body")
        with self.assertRaisesRegex(Quarantine, "GenerationFenced"):
            self.journal.admit(second, "blob", "digest")
        self.assertIsNone(self.journal.audit(PLAN)[0])

    def test_partial_plan_cannot_certify_global_staging_ownership(self):
        self.journal.admit(ENTRY, "blob", "digest")
        second = dict(ENTRY, operation="00000000-0000-4000-8000-000000000002",
                      object_key="artifacts/2/body")
        self.journal.admit(second, "blob", "digest")
        with self.assertRaisesRegex(Quarantine, "IncompleteGenerationPlan"):
            self.journal.fence(PLAN)

    def test_original_member_identity_cannot_be_substituted(self):
        self.journal.admit(ENTRY, "blob", "digest")
        changed = copy.deepcopy(PLAN)
        changed["members"][0]["root_read"] += 1
        with self.assertRaisesRegex(Quarantine, "IncompleteGenerationPlan"):
            self.journal.fence(changed)

    def test_lost_delete_reply_preserves_exact_intent(self):
        self.journal.admit(ENTRY, "blob", "digest")
        self.journal.fence(PLAN)
        items = catalogue_items(PLAN, [physical(ENTRY["object_key"])], OP)
        self.journal.settled(PLAN, {"native": "proof"}, items)
        self.journal.delete_intent(PLAN, items[0])
        self.reopen()
        self.assertTrue(self.journal.deletion_started(PLAN, items[0]))
        self.journal.absent(PLAN, items[0])
        self.reopen()
        self.assertEqual(self.journal.audit(PLAN), ({"native": "proof"}, items))

    def test_arbitrary_item_cannot_become_a_delete_intent(self):
        self.journal.admit(ENTRY, "blob", "digest")
        self.journal.fence(PLAN)
        self.journal.settled(PLAN, {"native": "proof"}, [])
        with self.assertRaisesRegex(Quarantine, "UncataloguedIdentity"):
            self.journal.delete_intent(PLAN, {"injected": True})

    def test_fence_receipt_is_immutable_on_retry(self):
        self.journal.admit(ENTRY, "blob", "digest")
        self.journal.fence(PLAN)
        self.journal.settled(PLAN, {"native": "proof"}, [])
        with self.assertRaisesRegex(Quarantine, "FenceChanged"):
            self.journal.settled(PLAN, {"native": "replacement"}, [])

    def test_new_native_mount_cannot_inherit_old_ownership(self):
        self.reopen()
        with self.assertRaisesRegex(Quarantine, "NativeGenerationChanged"):
            self.journal.bind({"generation": digest("different-volume")})


if __name__ == "__main__":
    unittest.main()
