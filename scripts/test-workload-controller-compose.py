#!/usr/bin/env python3
"""Offline config/model contracts only, not Engine/CP/TLS startup evidence."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
GENERATOR = ROOT / "scripts/generate-workload-controller-config.py"


def arguments(output):
    return ["python3", str(GENERATOR), "--api-origin", "https://cp.example.invalid/",
            "--engine-id", "17", "--cache-volume", "owned-cache", "--workload-network", "owned-workloads",
            "--probe-image", "sha256:" + "a" * 64, "--allow-image", "example.invalid/runtime@sha256:" + "b" * 64,
            "--output", str(output)]


class ControllerConfigTests(unittest.TestCase):
    def test_every_compiled_proxy_temp_module_uses_private_bounded_scratch(self):
        source = (ROOT / "docker/workload-launch-proxy.conf").read_text()
        for directive, path in [("client_body_temp_path", "client-body"), ("proxy_temp_path", "proxy"),
                                ("fastcgi_temp_path", "fastcgi"), ("uwsgi_temp_path", "uwsgi"), ("scgi_temp_path", "scgi")]:
            self.assertIn(f"{directive} /tmp/{path};", source)
        self.assertNotIn("/var/cache/nginx", source)

    def test_public_config_has_fixed_roots_and_no_credential_or_raw_plan(self):
        with tempfile.TemporaryDirectory(prefix="attune-controller-config-") as directory:
            path = Path(directory) / "config.json"
            subprocess.run(arguments(path), check=True, capture_output=True)
            config = json.loads(path.read_text())
            self.assertEqual(config["listen"], "127.0.0.1:8092")
            self.assertEqual(config["docker_socket"], "/var/run/docker.sock")
            self.assertEqual(config["session_file"], "/run/secrets/attune-controller-session")
            self.assertFalse(config["development_plain_http"])
            self.assertFalse(set(config) & {"token", "jwt_secret", "encryption_key", "database", "queue", "argv", "env", "mounts", "HostConfig"})
            self.assertEqual(len(config["allowed_images"]), 2)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertNotEqual(subprocess.run(arguments(path), capture_output=True).returncode, 0)

    def test_invalid_public_binding_and_insecure_origins_fail_before_output(self):
        with tempfile.TemporaryDirectory(prefix="attune-controller-negative-") as directory:
            path = Path(directory) / "config.json"
            for flag, value in [("--api-origin", "http://127.0.0.1/"),
                                ("--api-origin", "https://user:password@cp.example.invalid/"),
                                ("--api-origin", "https://cp.example.invalid/?token=secret"),
                                ("--engine-id", "0"), ("--engine-id", str(2**63)),
                                ("--cache-volume", "../sibling"), ("--workload-network", "host"),
                                ("--probe-image", "example.invalid/runtime:mutable")]:
                command = arguments(path)
                command[command.index(flag) + 1] = value
                self.assertNotEqual(subprocess.run(command, capture_output=True).returncode, 0)
                self.assertFalse(path.exists())

    def test_compose_separates_controller_authority_proxy_and_workload_subpaths(self):
        with tempfile.TemporaryDirectory(prefix="attune-controller-model-") as directory:
            root = Path(directory)
            # Synthetic public pins and dummy files are render fixtures, not
            # claimed images, sessions, certificates or startup prerequisites.
            for name in ["config", "session", "certificate", "key"]:
                (root / name).write_text("owned-render-fixture\n")
            env = {"PATH": os.environ["PATH"], "HOME": directory,
                   "ATTUNE_LAUNCH_CONFIG_FILE": str(root / "config"),
                   "ATTUNE_LAUNCH_SESSION_FILE": str(root / "session"),
                   "ATTUNE_LAUNCH_TLS_CERTIFICATE": str(root / "certificate"),
                   "ATTUNE_LAUNCH_TLS_PRIVATE_KEY": str(root / "key"),
                   "ATTUNE_LAUNCH_CACHE_VOLUME": "owned-cache",
                   "ATTUNE_LAUNCH_STATE_VOLUME": "owned-state",
                   "ATTUNE_LAUNCH_NETWORK": "owned-workloads",
                   "ATTUNE_LAUNCH_TLS_BIND": "127.0.0.1",
                   "ATTUNE_LAUNCH_PROXY_IMAGE": "example.invalid/nginx@sha256:" + "c" * 64}
            command = ["docker", "compose", "-f", str(ROOT / "docker-compose.workload-host.yaml"), "config", "--format", "json"]
            result = subprocess.run(command, env=env, capture_output=True, text=True, check=True)
            model = json.loads(result.stdout)
            self.assertEqual(set(model["services"]), {"launch-controller", "launch-proxy"})
            controller, proxy = (model["services"][name] for name in ["launch-controller", "launch-proxy"])
            for service in [controller, proxy]:
                self.assertTrue(service["read_only"])
                self.assertFalse(service.get("privileged", False))
                self.assertEqual(service["cap_drop"], ["ALL"])
                self.assertIn("no-new-privileges:true", service["security_opt"])
                self.assertNotEqual(service.get("pid"), "host")
                self.assertNotIn("env_file", service)
                self.assertFalse(service.get("environment"))
            self.assertEqual(set(controller["cap_add"]), {"CHOWN", "DAC_OVERRIDE", "FOWNER"})
            self.assertEqual(controller["user"], "0:0")
            self.assertEqual(controller["build"]["args"]["SERVICE"], "launch-controller")
            self.assertEqual(controller["build"]["args"]["SERVICE_USER"], "root")
            self.assertIn("-i", controller["entrypoint"])
            self.assertEqual(proxy["user"], "1000:1000")
            self.assertIn("-i", proxy["entrypoint"])
            self.assertFalse(proxy.get("cap_add"))
            self.assertEqual(proxy["network_mode"], "service:launch-controller")
            self.assertEqual({s["source"] for s in controller["secrets"]}, {"attune-controller-config", "attune-controller-session"})
            self.assertEqual({s["source"] for s in proxy["secrets"]}, {"attune-controller-certificate", "attune-controller-private-key"})
            self.assertTrue(any(v["target"] == "/var/run/docker.sock" for v in controller["volumes"]))
            self.assertFalse(any(v["target"] == "/var/run/docker.sock" for v in proxy["volumes"]))
            self.assertEqual(set(model["volumes"]), {"launch-cache", "controller-state"})
            self.assertEqual(model["volumes"]["launch-cache"]["name"], "owned-cache")
            self.assertEqual(model["networks"]["workloads"]["name"], "owned-workloads")
            self.assertEqual({port["target"] for port in controller["ports"]}, {8443})
            self.assertFalse(proxy.get("ports"))
            proxy_config = (ROOT / "docker/workload-launch-proxy.conf").read_text()
            self.assertIn("proxy_set_header Host $http_host;", proxy_config)
            self.assertIn("proxy_pass_request_headers on;", proxy_config)
            self.assertIn("proxy_pass_request_body on;", proxy_config)
            self.assertIn("proxy_request_buffering off;", proxy_config)
            self.assertIn("access_log off;", proxy_config)
            self.assertIn("error_log /dev/null;", proxy_config)
            missing = dict(env)
            missing.pop("ATTUNE_LAUNCH_PROXY_IMAGE")
            self.assertNotEqual(subprocess.run(command, env=missing, capture_output=True).returncode, 0)


if __name__ == "__main__":
    unittest.main()
