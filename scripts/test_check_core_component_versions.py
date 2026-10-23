import importlib.util
import unittest
from pathlib import Path
from unittest.mock import patch


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

    def test_current_layout_checks_timer_and_pack_without_legacy_workflow(self):
        def component_version(ref, path, pattern):
            self.assertIn(path, (
                "crates/core-timer-sensor/Cargo.toml", "packs/core/pack.yaml"
            ))
            return "1.0.0" if ref == "base" else "1.0.1"

        with (
            patch("sys.argv", [str(SCRIPT), "base", "--head-ref", "head"]),
            patch.object(versions, "version_at", side_effect=component_version),
            patch.object(versions, "tree_digest", side_effect=lambda ref, path: ref),
            patch.object(versions, "timer_toolchain", return_value="toolchain"),
            patch.object(versions, "validate_component", wraps=versions.validate_component) as validate,
        ):
            versions.main()

        self.assertEqual(
            [call.args[0] for call in validate.call_args_list],
            ["core timer sensor", "core pack"],
        )


if __name__ == "__main__":
    unittest.main()
