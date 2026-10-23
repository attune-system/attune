"""CENTRAL ONLY: real SQLite prospective enrollment recovery. Not run by agents."""
import tempfile
from pathlib import Path
import unittest

from journal import Journal
from protocol import Quarantine
from test_controller_source import ENTRY, OP


class OriginalEnrollmentTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="attune-native-enrollment-")
        self.path = Path(self.temporary.name) / "journal.sqlite3"
        self.journal = Journal(self.path)
        self.journal.bind({"generation": OP}, initialize=True)
        self.original = {"run_id": "owned-run", "project": "eaccept-owned-run-native-blob", "mode": "blob",
                         "writer": {key: ENTRY[key] for key in
                                    ("native_instance", "worker_instance", "execution_attempt")},
                         "certificate_sha256": "a" * 64}

    def tearDown(self):
        self.journal.db.close()
        self.temporary.cleanup()

    def test_nonce_is_durable_before_any_body_request(self):
        original = self.journal.enroll(self.original)
        self.journal.db.close()
        self.journal = Journal(self.path)
        self.assertEqual(self.journal.enroll(self.original), original)
        self.assertEqual(Journal.observe(self.path)["requests"], {})

    def test_existing_native_request_cannot_be_enrolled_after_the_fact(self):
        self.journal.admit(ENTRY, "blob", "digest")
        with self.assertRaisesRegex(Quarantine, "GenerationAlreadyWriting"):
            self.journal.enroll(self.original)

    def test_writer_identity_and_mode_are_immutable(self):
        self.journal.enroll(self.original)
        with self.assertRaisesRegex(Quarantine, "NativeEnrollmentRebound"):
            self.journal.enroll(dict(self.original, mode="shared"))

    def test_only_the_original_registered_request_can_reach_the_writer(self):
        original = self.journal.enroll(self.original)
        entry = dict(ENTRY, operation=original["operation"])
        self.journal.require_enrollment(entry, "blob")
        with self.assertRaisesRegex(Quarantine, "OriginalEnrollmentMismatch"):
            self.journal.require_enrollment(ENTRY, "blob")


if __name__ == "__main__":
    unittest.main()
