#!/usr/bin/env python3
"""Exercise the current publisher against an actual pinned incompatible API image."""
import importlib.util
import json
from pathlib import Path
import re
import sys
import time
import urllib.error

from fixture_support import API, FixtureError, read_secret, require, run, write_private


def load_driver(ctx):
    path = Path(ctx["source_root"]) / "scripts/run-bootstrap-lifecycle-acceptance.py"
    spec = importlib.util.spec_from_file_location("owned_acceptance_driver", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def pins(api, access):
    return {"core": api.call("/health/bootstrap")["core"], "releases": api.call("/api/v1/packs/core/releases", access)["data"], "config": api.call("/api/v1/packs/core", access)["data"]["config"]}


def assert_incompatible(ready, expected):
    require("bootstrap_admission_protocol" in expected, "Declare the expected old readiness protocol, including explicit null if absent")
    observed = ready.get("bootstrap_admission_protocol")
    wanted = expected["bootstrap_admission_protocol"]
    require(type(observed) is type(wanted) and observed == wanted, "Old image did not return the configured incompatible protocol")
    require(type(observed) is not int or observed != 1, "Protocol 1 is not an incompatible old protocol; do not count gate-OFF as skew")
    return observed


class OldAPI:
    def __init__(self, ctx, driver):
        self.ctx, self.driver = ctx, driver
        self.name = ctx["project"] + "-old-api"
        self.volumes = []
        self.container = False
        self.environment = Path(ctx["compose_file"]).parent / "old-api-env.private"
        self.labels = {ctx["owner_label"]: ctx["run_id"], "com.docker.compose.project": ctx["project"], "io.attune.acceptance.probe": "old-api"}

    def docker(self, *args):
        return self.driver.command(["docker", "--context", self.ctx["docker_context"], *args])

    def inspect_owned(self, kind, identifier, probe=False):
        value = json.loads(self.docker(kind, "inspect", identifier))[0]
        labels = value.get("Config", {}).get("Labels", {}) if kind == "container" else value.get("Labels", {})
        expected = self.labels if probe else {self.ctx["owner_label"]: self.ctx["run_id"], "com.docker.compose.project": self.ctx["project"]}
        require(all(labels.get(key) == item for key, item in expected.items()), "Refusing foreign old-image resource")
        return value

    def start(self):
        require(re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", self.ctx["old_image"]), "An actual immutable old API image is required")
        daemon = json.loads(self.docker("context", "inspect", self.ctx["docker_context"]))[0]
        require(daemon["Endpoints"]["docker"]["Host"].startswith(("unix://", "npipe://")), "Old-image fixture requires an explicit local Docker daemon")
        self.docker("image", "inspect", self.ctx["old_image"])  # Never implicitly pull.
        require(not self.docker("ps", "-aq", "--filter", "name=^/" + self.name + "$").strip(), "Old API container already exists; do not adopt it")
        candidates = self.docker("ps", "-q", "--filter", "label=com.docker.compose.project=" + self.ctx["project"], "--filter", "label=com.docker.compose.service=api").split()
        require(len(candidates) == 1, "Need exactly one owned current API container")
        current = self.inspect_owned("container", candidates[0])
        networks = list(current["NetworkSettings"]["Networks"])
        require(len(networks) == 1, "Old-image fixture requires one explicit owned platform network")
        self.inspect_owned("network", networks[0])
        model = json.loads(Path(self.ctx["compose_file"]).read_text())
        service = model["services"]["api"]
        # The only environment copied belongs to the disposable platform API,
        # never a human or installer access/integration credential.
        environment = service["environment"]
        require(not any(key in environment for key in ("ATTUNE_BOOTSTRAP_TOKEN", "ATTUNE_ACCESS_TOKEN", "ATTUNE_TOKEN")), "Do not copy operator credentials into an old API image")
        require(all("\n" not in str(value) and "\r" not in str(value) for value in environment.values()), "Unsafe env-file value")
        write_private(self.environment, "\n".join(key + "=" + str(value) for key, value in environment.items()))
        args = ["create", "--pull", "never", "--name", self.name, "--network", networks[0], "--env-file", str(self.environment), "--publish", "127.0.0.1::8080"]
        for key, value in self.labels.items():
            args += ["--label", key + "=" + value]
        for index, mount in enumerate(service.get("volumes", [])):
            parts = mount.split(":")
            require(len(parts) in (2, 3), "Unsupported API mount in old-image fixture")
            source, target = parts[:2]
            if source in model.get("volumes", {}):
                # Empty isolated pack/cache/body volumes, never give an old image
                # the current deployment's writable release/artifact directories.
                volume = self.name + "-" + str(index)
                require(not self.docker("volume", "ls", "-q", "--filter", "name=^" + volume + "$").strip(), "Old API volume exists; no adoption")
                flags = ["volume", "create"]
                for key, value in self.labels.items():
                    flags += ["--label", key + "=" + value]
                self.docker(*flags, volume)
                self.volumes.append(volume)
                self.inspect_owned("volume", volume, probe=True)
                args += ["--mount", "type=volume,source=" + volume + ",target=" + target]
            else:
                actual = [entry for entry in current["Mounts"] if entry["Destination"] == target and entry["Type"] == "bind" and not entry["RW"]]
                require(len(actual) == 1, "Missing current API read-only source mount")
                path = Path(actual[0]["Source"]).resolve()
                require(len(parts) == 3 and parts[2] == "ro" and path.is_relative_to(Path(self.ctx["source_root"]).resolve()), "Only owned source read-only binds can enter an old API image")
                args += ["--mount", "type=bind,source=" + str(path) + ",target=" + target + ",readonly"]
        self.docker(*args, self.ctx["old_image"])
        self.container = True
        self.inspect_owned("container", self.name, probe=True)
        self.docker("start", self.name)
        endpoint = self.docker("port", self.name, "8080/tcp").strip()
        require(re.fullmatch(r"127\.0\.0\.1:[0-9]+", endpoint), "Old API did not receive a loopback-only ephemeral port")
        return "http://" + endpoint

    def cleanup(self):
        require(not self.driver.ACTIVE_GROUPS, "Old-image subprocess group has not joined; retain volumes")
        if self.container:
            self.inspect_owned("container", self.name, probe=True)
            self.docker("stop", "--time", "30", self.name)
            self.docker("wait", self.name)
            value = self.inspect_owned("container", self.name, probe=True)
            require(not value["State"]["Running"] and not value["State"].get("Restarting"), "Old API writer has not stopped")
            self.docker("rm", self.name)
        for volume in self.volumes:
            self.inspect_owned("volume", volume, probe=True)
            self.docker("volume", "rm", volume)
        if self.environment.exists():
            self.environment.unlink()


def probe(ctx, driver=None, api=None):
    driver = driver or load_driver(ctx)
    api = api or API(ctx["api_url"])
    assert_incompatible(ctx["old_expected_contract"], ctx["old_expected_contract"])
    publisher_spec = importlib.util.spec_from_file_location("actual_bootstrap_publisher", Path(ctx["source_root"]) / "scripts/bootstrap_core_pack.py")
    publisher = importlib.util.module_from_spec(publisher_spec)
    publisher_spec.loader.exec_module(publisher)
    try:
        publisher.require_contract(api.call("/health/ready"))
    except RuntimeError:
        raise FixtureError("Current API contract is gate-OFF/unconverted; cannot count that as old-image acceptance") from None
    installer = api.call("/auth/token-login", body={"token": read_secret(ctx["credential_output"]).strip()}, method="POST")["data"]["access_token"]
    before = pins(api, installer)
    old = OldAPI(ctx, driver)
    calls = []
    try:
        base = old.start()
        observed_api = API(base)
        deadline = time.monotonic() + 120
        while True:
            try:
                ready = observed_api.call("/health/ready")
                break
            except (OSError, urllib.error.URLError, FixtureError):
                value = old.inspect_owned("container", old.name, probe=True)
                require(value["State"]["Running"], "Old API crashed; this is not protocol-skew acceptance")
                require(time.monotonic() < deadline, "Old API never returned HTTP 200 readiness; not skew acceptance")
                time.sleep(0.25)
        observed = assert_incompatible(ready, ctx["old_expected_contract"])

        def recorded_request(url, **kwargs):
            require(url.startswith(base + "/"), "Publisher attempted a foreign API")
            calls.append((kwargs.get("method", "GET"), url[len(base):]))
            # Refuse any effect, but count an attempted effect as a failing probe.
            require(calls[-1] == ("GET", "/health/ready"), "Publisher attempted an effect before rejecting old-image protocol")
            return driver.api_request(url, **kwargs)

        publisher.request = recorded_request
        rejected = False
        try:
            publisher.wait_for_api(base, time.monotonic() + 15)
        except RuntimeError as error:
            require(str(error).startswith("API publication contract is unavailable"), "Publisher failed for an unrelated reason")
            rejected = True
        require(rejected and calls == [("GET", "/health/ready")], "Actual publisher did not reject at the old readiness boundary")
    finally:
        old.cleanup()  # stop, wait, inspect, remove before any volume deletion
    require(pins(api, installer) == before, "Old-image exercise changed actual core release/config/policy inventory")
    proof = {"actual_old_image": True, "image": ctx["old_image"], "rejection_stage": "api_protocol", "observed_http_status": 200, "observed_protocol": observed, "publication_requests": sum(path == "/api/v1/packs/upload" for _, path in calls), "publisher_requests": calls, "rejected_before_publication": rejected, "current_inventory_unchanged": True, "old_writer_joined": True}
    write_private(Path(ctx["compose_file"]).parent / "old-image-evidence.json", proof)
    return proof


if __name__ == "__main__":
    run(probe)
