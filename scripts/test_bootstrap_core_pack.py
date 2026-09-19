import importlib.util
import os
import sys
import unittest
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).with_name("bootstrap_core_pack.py")
SPEC = importlib.util.spec_from_file_location("bootstrap_core_pack", SCRIPT)
bootstrap = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bootstrap)


class BootstrapCorePackTests(unittest.TestCase):
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
