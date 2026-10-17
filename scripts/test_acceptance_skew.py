"""Offline skew-fixture tests. No image is run and no HTTP server is created."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts/acceptance"))
import probe_old_image as skew
from fixture_support import FixtureError, write_private

spec = importlib.util.spec_from_file_location("actual_publisher_for_skew_test", ROOT / "scripts/bootstrap_core_pack.py")
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)
READY = {"pack_publication": {"protocol_version": 1, "client_version": 1, "revision": publisher.BOOTSTRAP_REVISION, "authorization": "registrar", "components": list(publisher.BOOTSTRAP_COMPONENTS)}}


class SkewTests(unittest.TestCase):
    def test_absent_protocol_can_be_explicit_but_gateoff_is_not_skew(self):
        self.assertIsNone(skew.assert_incompatible({}, {"bootstrap_admission_protocol": None}))
        for ready, expected in (({}, {}), ({"bootstrap_admission_protocol": 1}, {"bootstrap_admission_protocol": 1}), ({"bootstrap_admission_protocol": 2}, {"bootstrap_admission_protocol": None})):
            with self.assertRaises(FixtureError):
                skew.assert_incompatible(ready, expected)

    def test_actual_publisher_rejects_after_one_observed_get_and_inventory_is_compared(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            root = Path(directory)
            secret = root / "installer.private"
            write_private(secret, "unit-test-not-live")
            ctx = {"source_root": str(ROOT), "api_url": "http://127.0.0.1:1234", "credential_output": str(secret), "compose_file": str(root / "compose.private.json"), "old_expected_contract": {"bootstrap_admission_protocol": None}, "old_image": "unit-test@sha256:" + "a" * 64}
            api = mock.Mock()
            api.call.side_effect = [READY, {"data": {"access_token": "not-live"}}]
            observed = mock.Mock()
            observed.call.return_value = {"status": "ready"}
            old = mock.Mock()
            old.start.return_value = "http://127.0.0.1:1235"
            def response(*args, **kwargs):
                value = io.BytesIO(b'{"status":"ready"}')
                value.status = 200
                return contextlib.closing(value)
            driver = SimpleNamespace(api_request=response)
            inventory = {"core": {"release_id": 3, "policy_revision": 7}, "releases": [3]}
            with mock.patch.object(skew, "OldAPI", return_value=old), mock.patch.object(skew, "API", return_value=observed), mock.patch.object(skew, "pins", return_value=inventory) as pins:
                result = skew.probe(ctx, driver, api)
            old.cleanup.assert_called_once()
            self.assertEqual(pins.call_count, 2)
            self.assertEqual(result["publisher_requests"], [("GET", "/health/ready")])
            self.assertEqual(result["publication_requests"], 0)
            self.assertTrue(result["rejected_before_publication"])

    def test_gateoff_current_api_fails_before_any_old_container_is_created(self):
        api = mock.Mock()
        api.call.return_value = {"bootstrap_admission_protocol": 1}
        ctx = {"source_root": str(ROOT), "api_url": "http://127.0.0.1:1234", "old_expected_contract": {"bootstrap_admission_protocol": None}}
        with mock.patch.object(skew, "OldAPI") as old:
            with self.assertRaisesRegex(FixtureError, "gate-OFF"):
                skew.probe(ctx, SimpleNamespace(), api)
            old.assert_not_called()

    def test_teardown_joins_and_rechecks_ownership_before_exact_volume_remove(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            ctx = {"run_id": "test", "project": "eaccept-test-sharedfile", "owner_label": "io.attune.acceptance.owner", "compose_file": str(Path(directory) / "compose.private.json")}
            old = skew.OldAPI(ctx, SimpleNamespace(ACTIVE_GROUPS=set()))
            old.container = True
            old.volumes = ["owned-only"]
            events = []
            with mock.patch.object(old, "inspect_owned", side_effect=lambda *args, **kwargs: events.append(("inspect", args[0])) or {"State": {"Running": False}}), mock.patch.object(old, "docker", side_effect=lambda *args: events.append(args)):
                old.cleanup()
            wait = events.index(("wait", old.name))
            volume_remove = events.index(("volume", "rm", "owned-only"))
            self.assertLess(wait, volume_remove)
            self.assertEqual(events[volume_remove - 1], ("inspect", "volume"))
            self.assertIn(("rm", old.name), events[wait:volume_remove])

    def test_foreign_or_unjoined_resources_prevent_removal(self):
        ctx = {"run_id": "test", "project": "eaccept-test-sharedfile", "owner_label": "io.attune.acceptance.owner", "compose_file": "/not-used/compose.private.json"}
        old = skew.OldAPI(ctx, SimpleNamespace(ACTIVE_GROUPS={1234}))
        old.container = True
        with mock.patch.object(old, "docker") as docker:
            with self.assertRaisesRegex(FixtureError, "not joined"):
                old.cleanup()
            docker.assert_not_called()
        old.driver.ACTIVE_GROUPS.clear()
        with mock.patch.object(old, "docker", return_value=json.dumps([{"Config": {"Labels": {}}}])) as docker:
            with self.assertRaisesRegex(FixtureError, "foreign"):
                old.cleanup()
            self.assertEqual(docker.call_count, 1)


if __name__ == "__main__":
    unittest.main()
