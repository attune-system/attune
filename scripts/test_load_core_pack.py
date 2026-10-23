import os
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))
import load_core_pack as loader


class LoaderAdmissionTests(unittest.TestCase):
    def test_loader_calls_common_api_publisher(self):
        with mock.patch.dict(os.environ, {"ATTUNE_API_URL": "http://api:8080"}), \
             mock.patch.object(sys, "argv", ["load_core_pack.py", "--pack-dir", "/source/packs"]), \
             mock.patch.object(loader, "publish_core") as publish:
            loader.main()
        publish.assert_called_once_with("http://api:8080", Path("/source/packs/core"))

    def test_database_loader_flags_are_rejected_before_effects(self):
        with mock.patch.object(sys, "argv", ["load_core_pack.py", "--database-url", "postgresql://unused"]), \
             mock.patch.object(loader, "publish_core") as publish, \
             self.assertRaises(SystemExit):
            loader.main()
        publish.assert_not_called()

    def test_dry_run_needs_no_api_or_credentials(self):
        with mock.patch.dict(os.environ, {}, clear=True), \
             mock.patch.object(sys, "argv", ["load_core_pack.py", "--dry-run"]), \
             mock.patch.object(loader, "publish_core") as publish:
            loader.main()
        publish.assert_not_called()


if __name__ == "__main__":
    unittest.main()
