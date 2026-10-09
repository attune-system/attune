"""Keep fresh-volume RabbitMQ healthchecks out of the root cookie-creation race."""

import ast
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parent.parent


class RabbitMQHealthcheckTests(unittest.TestCase):
    def assert_service_healthcheck(self, path):
        compose = (ROOT / path).read_text()
        service = re.search(r"^  rabbitmq:\n(.*?)(?=^  \S|\Z)", compose, re.MULTILINE | re.DOTALL).group(1)
        command = re.search(r"^      test: (\[.*\])$", service, re.MULTILINE).group(1)
        self.assertEqual(ast.literal_eval(command),
                         ["CMD", "su-exec", "rabbitmq", "rabbitmq-diagnostics", "-q", "ping"])
        self.assertIn("image: rabbitmq:4.3.6-management-alpine", service)
        self.assertIn("interval: 10s", service)
        self.assertIn("timeout: 5s", service)
        self.assertIn("retries: 5", service)

    def test_local_compose_healthcheck_uses_server_os_user(self):
        self.assert_service_healthcheck("docker-compose.yaml")

    def test_distributable_healthcheck_uses_server_os_user(self):
        self.assert_service_healthcheck("docker/distributable/docker-compose.yaml")

    def test_ci_healthcheck_uses_server_os_user(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        command = re.search(r'--health-cmd "([^"]*rabbitmq-diagnostics[^"]*)"', workflow).group(1)
        self.assertEqual(command, "su-exec rabbitmq rabbitmq-diagnostics -q ping")

    def test_rust_runner_readiness_probe_uses_server_os_user(self):
        runner = (ROOT / "scripts/run-rust-integration-tests.sh").read_text()
        command = re.search(r"compose exec -T rabbitmq ([^\n]*rabbitmq-diagnostics[^\n]*)", runner).group(1)
        self.assertEqual(command, "su-exec rabbitmq rabbitmq-diagnostics -q ping >/dev/null 2>&1; then")


if __name__ == "__main__":
    unittest.main()
