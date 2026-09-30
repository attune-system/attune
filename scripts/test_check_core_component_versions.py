import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("check_core_component_versions.py")
SPEC = importlib.util.spec_from_file_location("check_core_component_versions", SCRIPT)
versions = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(versions)


class CoreComponentVersionTests(unittest.TestCase):
    def test_unchanged_inputs_keep_version(self):
        versions.validate_component("timer", "1.0.0", "same", "1.0.0", "same")

    def test_changed_inputs_require_version_bump(self):
        with self.assertRaisesRegex(RuntimeError, "changed without a version bump"):
            versions.validate_component("timer", "1.0.0", "old", "1.0.0", "new")

    def test_version_bump_requires_changed_inputs(self):
        with self.assertRaisesRegex(RuntimeError, "version changed but its inputs did not"):
            versions.validate_component("timer", "1.0.0", "same", "1.0.1", "same")

    def test_changed_inputs_and_version_pass(self):
        versions.validate_component("timer", "1.0.0", "old", "1.0.1", "new")


if __name__ == "__main__":
    unittest.main()
