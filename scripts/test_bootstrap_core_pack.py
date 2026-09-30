import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).with_name("bootstrap_core_pack.py")
SPEC = importlib.util.spec_from_file_location("bootstrap_core_pack", SCRIPT)
bootstrap = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bootstrap)


class BootstrapCorePackTests(unittest.TestCase):
    def test_matching_active_release_skips_upload(self):
        with tempfile.TemporaryDirectory() as directory:
            pack_dir = Path(directory)
            (pack_dir / "pack.yaml").write_text(
                'ref: core\nversion: "1.0.2"\n', encoding="utf-8"
            )

            with (
                mock.patch.object(
                    bootstrap,
                    "get_active_release",
                    return_value={
                        "version": "1.0.2",
                        "digest": "abc",
                        "is_active": True,
                    },
                ),
                mock.patch.object(bootstrap, "upload") as upload,
            ):
                changed = bootstrap.reconcile(
                    "http://api", "token", pack_dir, "1.0.2", None
                )

        self.assertFalse(changed)
        upload.assert_not_called()

    def test_different_active_release_uploads(self):
        with tempfile.TemporaryDirectory() as directory:
            pack_dir = Path(directory)
            (pack_dir / "pack.yaml").write_text(
                'ref: core\nversion: "1.0.2"\n', encoding="utf-8"
            )

            with (
                mock.patch.object(
                    bootstrap,
                    "get_active_release",
                    side_effect=[
                        {"version": "1.0.1", "digest": "old", "is_active": True},
                        {"version": "1.0.2", "digest": "new", "is_active": True},
                    ],
                ),
                mock.patch.object(bootstrap, "upload") as upload,
            ):
                changed = bootstrap.reconcile(
                    "http://api", "token", pack_dir, "1.0.2", "new"
                )

        self.assertTrue(changed)
        upload.assert_called_once_with("http://api", "token", pack_dir)

    def test_matching_version_with_wrong_digest_fails_without_upload(self):
        with tempfile.TemporaryDirectory() as directory:
            pack_dir = Path(directory)
            (pack_dir / "pack.yaml").write_text(
                'ref: core\nversion: "1.0.2"\n', encoding="utf-8"
            )

            with (
                mock.patch.object(
                    bootstrap,
                    "get_active_release",
                    return_value={
                        "version": "1.0.2",
                        "digest": "unexpected",
                        "is_active": True,
                    },
                ),
                mock.patch.object(bootstrap, "upload") as upload,
                self.assertRaisesRegex(RuntimeError, "expected wanted"),
            ):
                bootstrap.reconcile(
                    "http://api", "token", pack_dir, "1.0.2", "wanted"
                )

        upload.assert_not_called()

    def test_chart_and_bundled_pack_versions_must_match(self):
        with tempfile.TemporaryDirectory() as directory:
            pack_dir = Path(directory)
            (pack_dir / "pack.yaml").write_text(
                'ref: core\nversion: "1.0.1"\n', encoding="utf-8"
            )

            with self.assertRaisesRegex(RuntimeError, "expected core pack version"):
                bootstrap.reconcile(
                    "http://api", "token", pack_dir, "1.0.2", None
                )

    def test_pack_release_lookup_returns_active_release(self):
        response = mock.MagicMock(status=200)
        response.__enter__.return_value = response
        response.read.return_value = json.dumps(
            {
                "data": [
                    {"version": "1.0.1", "digest": "old", "is_active": False},
                    {"version": "1.0.2", "digest": "new", "is_active": True},
                ]
            }
        ).encode()
        response.__iter__.return_value = iter(response.read.return_value.splitlines())
        with mock.patch.object(bootstrap, "request", return_value=response):
            active = bootstrap.get_active_release("http://api", "token")

        self.assertEqual(active["version"], "1.0.2")

    def test_api_wait_uses_platform_readiness(self):
        response = mock.MagicMock(status=200)
        response.__enter__.return_value = response
        with mock.patch.object(bootstrap, "request", return_value=response) as request:
            bootstrap.wait_for_api("http://api", bootstrap.time.monotonic() + 10)

        request.assert_called_once_with("http://api/health/ready")

    def test_content_wait_uses_transitional_content_health(self):
        response = mock.MagicMock(status=200)
        response.__enter__.return_value = response
        with mock.patch.object(bootstrap, "request", return_value=response) as request:
            bootstrap.wait_for_core("http://api", bootstrap.time.monotonic() + 10)

        request.assert_called_once_with("http://api/health/content")

    def test_timeout_is_capped_at_300_seconds(self):
        with (
            mock.patch.dict(
                os.environ,
                {
                    "ATTUNE_API_URL": "http://api",
                    "ATTUNE_BOOTSTRAP_TIMEOUT_SECONDS": "999",
                },
                clear=False,
            ),
            mock.patch.object(sys, "argv", [str(SCRIPT), "wait"]),
            mock.patch.object(bootstrap.time, "monotonic", return_value=100),
            mock.patch.object(bootstrap, "wait_for_api") as wait_for_api,
            mock.patch.object(bootstrap, "wait_for_core") as wait_for_core,
        ):
            bootstrap.main()

        wait_for_api.assert_called_once_with("http://api", 400)
        wait_for_core.assert_called_once_with("http://api", 400)


if __name__ == "__main__":
    unittest.main()
