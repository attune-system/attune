#!/usr/bin/env python3
"""Actual source image/project against an owned HTTPS CP protocol fixture.

This proves packaged startup, proxy confinement and TLS/denial contracts. The
fixture has no DB, valid launches or authority effects: it is NOT proof of CP
admission, stdin consumption, artifact publication or the missing host loops.
Only an explicitly supplied disposable local Engine socket may be used.
"""
import argparse
from datetime import datetime, timedelta, timezone
import http.server
import json
import os
from pathlib import Path
import socket
import ssl
import stat
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid
from workload_startup_diagnostics import command as bounded_command

ROOT = Path(__file__).resolve().parent.parent
PROBE = "debian@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587"
PEER = "python@sha256:935c3c5b12357a7e10caa9c1e6ca5692344d97cf229832ef584be2cec9fc03df"
# Published Web image used ONLY for its standard nginx runtime with our config.
# It does not implement/attest any new workload-host or controller contract.
PROXY = "ghcr.io/attune-system/attune/web@sha256:4966b7f9f57be74a00f1c7b924c1b657aa27ffe574ac6166d0e65cbf6e82654c"
DUMMY_SESSION = "owned-controller-session-not-a-production-credential"
PRIVATE_MARKER = "owned-private-payload-must-not-be-logged"


def diagnose(docker_command, owner):
    raw, code, truncated = bounded_command([*docker_command, "ps", "-aq", "--filter", f"label=com.docker.compose.project={owner}"], 16384)
    if code or truncated:
        return [{"diagnostic": "owned_project_listing_failed"}]
    reports = []
    signatures = {
        "private_session_invalid": "invalid private controller session state",
        "controller_file_invalid": "controller file ownership/mode/size invalid",
        "tls_roots_missing": "No CA certificates were loaded from the system",
        "control_transport_failed": "Control(Transport)",
        "control_denied": "Control(Rejected(403))",
        "engine_policy_failed": "Policy(",
        "parent_protection_failed": "could not protect controller parent",
        "root_owner_required": "trusted controller requires root ownership",
    }
    for identifier in raw.splitlines()[:8]:
        data, code, truncated = bounded_command([*docker_command, "inspect", "--format",
            '{"owner":{{json (index .Config.Labels "io.attune.test.owner")}},'
            '"service":{{json (index .Config.Labels "com.docker.compose.service")}},'
            '"state":{{json .State.Status}},"exit":{{.State.ExitCode}},"oom":{{.State.OOMKilled}}}', identifier], 4096)
        try:
            info = json.loads(data) if not code and not truncated else {}
        except ValueError:
            info = {}
        if info.get("owner") != owner or info.get("service") not in {"launch-controller", "launch-proxy"}:
            continue
        logs, code, truncated = bounded_command([*docker_command, "logs", "--tail", "80", identifier])
        reports.append({"service": info["service"], "state": info.get("state"),
            "exit": info.get("exit"), "oom": info.get("oom"),
            "categories": {key: logs.count(value) for key, value in signatures.items() if value in logs},
            "log_bytes_examined": len(logs.encode()), "log_failed": code != 0, "log_truncated": truncated,
            "private_marker_present": any(value in logs for value in [DUMMY_SESSION, PRIVATE_MARKER, "owned-wrong-grant"])})
    return reports


def run(command, **kwargs):
    return subprocess.run(command, check=True, **kwargs)


def certificates(root):
    def openssl(*args):
        run(["openssl", *args], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    openssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
            "-subj", "/CN=Attune Owned Project CA", "-addext", "basicConstraints=critical,CA:TRUE",
            "-keyout", str(root / "ca.key"), "-out", str(root / "ca.crt"))
    for name, subject, names in [("cp", "host.docker.internal", "DNS:host.docker.internal"),
                                 ("proxy", "localhost", "DNS:localhost,IP:127.0.0.1")]:
        openssl("req", "-newkey", "rsa:2048", "-nodes", "-subj", f"/CN={subject}",
                "-keyout", str(root / f"{name}.key"), "-out", str(root / f"{name}.csr"))
        extension = root / f"{name}.extensions"
        extension.write_text(f"subjectAltName={names}\n")
        openssl("x509", "-req", "-in", str(root / f"{name}.csr"), "-CA", str(root / "ca.crt"),
                "-CAkey", str(root / "ca.key"), "-CAcreateserial", "-days", "1",
                "-extfile", str(extension), "-out", str(root / f"{name}.crt"))
        (root / f"{name}.key").chmod(0o400)
        (root / f"{name}.crt").chmod(0o444)


def change_owner(path, uid):
    if os.geteuid() == 0:
        os.chown(path, uid, uid)
    else:
        # These are only the fresh fixture's explicit files. No host policy,
        # socket ownership, unrelated files or security setting is changed.
        run(["sudo", "-n", "chown", f"{uid}:{uid}", str(path)])


class FixtureCp(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def reply(self, code, value):
        body = json.dumps(value, separators=(",", ":")).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.headers.get("Authorization") != "Bearer " + DUMMY_SESSION:
            return self.reply(403, {})
        if self.path == "/api/v1/workload-launches/engine":
            return self.reply(200, self.server.binding)
        self.reply(403, {})

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        if length > 12 * 1024 * 1024:
            return self.reply(413, {})
        self.rfile.read(length)
        if self.headers.get("Authorization") != "Bearer " + DUMMY_SESSION:
            return self.reply(403, {})
        if self.path == "/api/v1/workload-launches/reconcile":
            return self.reply(200, {"items": [], "next_cursor": None})
        # No fake successful dispatch, lease, invocation, stdin or publication.
        self.reply(403, {})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--owned-engine-socket", required=True, type=Path)
    args = parser.parse_args()
    endpoint = args.owned_engine_socket.resolve(strict=True)
    if not endpoint.is_absolute() or not stat.S_ISSOCK(endpoint.stat().st_mode):
        raise SystemExit("explicit owned local Engine socket is required")
    owner = "attune-project-" + uuid.uuid4().hex
    image = "attune-controller-owned:" + owner
    engine_id = int(uuid.uuid4().hex[:15], 16) + 1
    cache_name, state_name, network = (owner + suffix for suffix in ["-cache", "-state", "-workloads"])
    docker_command = ["docker", "--host", f"unix://{endpoint}"]
    def docker(*values, **kwargs):
        return run([*docker_command, *values], **kwargs)
    print(json.dumps({"owner": owner, "engine_id": engine_id, "image": image,
                      "cache_volume": cache_name, "state_volume": state_name}), flush=True)
    diagnostics = ROOT / ".scratch/e2e-diagnostics"
    diagnostics.mkdir(parents=True, exist_ok=True)
    run(["python3", str(ROOT / "scripts/capture_build_diagnostics.py"),
           "--output", str(diagnostics / (owner + ".build.jsonl")), "--group", "controller-project", "--",
           *docker_command, "build", "--progress=plain", "--label", f"io.attune.test.owner={owner}",
           "--build-arg", "SERVICE=launch-controller", "--build-arg", "SERVICE_USER=root",
           "--build-arg", "CARGO_BUILD_JOBS=2", "-f", str(ROOT / "docker/Dockerfile.optimized"), "-t", image, str(ROOT)])
    for selected in [PROBE, PEER, PROXY]:
        docker("pull", selected)
    info = json.loads(docker("info", "--format", "{{json .}}", capture_output=True, text=True).stdout)
    architecture = {"x86_64": "amd64", "amd64": "amd64", "aarch64": "arm64", "arm64": "arm64"}.get(info["Architecture"])
    if info["OSType"] != "linux" or architecture is None:
        raise SystemExit("owned Engine must provide a supported actual Linux platform")
    with tempfile.TemporaryDirectory(prefix=owner + "-") as directory:
        root = Path(directory)
        certificates(root)
        cp = http.server.ThreadingHTTPServer(("0.0.0.0", 0), FixtureCp)
        cp.binding = {"engine_id": engine_id, "daemon_id": info["ID"], "platform": "linux/" + architecture}
        cp_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        cp_context.load_cert_chain(root / "cp.crt", root / "cp.key")
        cp.socket = cp_context.wrap_socket(cp.socket, server_side=True)
        cp_thread = threading.Thread(target=cp.serve_forever, kwargs={"poll_interval": 0.05})
        cp_thread.start()
        source = root / "project.json"
        compose = [*docker_command, "compose", "-p", owner, "-f", str(source)]
        started = False
        ready = False
        test_error = None
        cleanup_errors = []
        phase = "public-config-and-private-session"
        try:
            config = root / "config.json"
            run(["python3", str(ROOT / "scripts/generate-workload-controller-config.py"),
                 "--api-origin", f"https://host.docker.internal:{cp.server_port}/", "--engine-id", str(engine_id),
                 "--cache-volume", cache_name, "--workload-network", network, "--probe-image", PROBE, "--output", str(config)])
            session = root / "session.json"
            session.write_text(json.dumps({"token": DUMMY_SESSION,
                "expires_at": (datetime.now(timezone.utc) + timedelta(seconds=240)).isoformat().replace("+00:00", "Z")}))
            session.chmod(0o600)
            change_owner(config, 0)
            change_owner(session, 0)
            change_owner(root / "proxy.key", 1000)
            environment = {"PATH": os.environ["PATH"], "HOME": directory,
                "ATTUNE_LAUNCH_CONFIG_FILE": str(config), "ATTUNE_LAUNCH_SESSION_FILE": str(session),
                "ATTUNE_LAUNCH_CP_CA_BUNDLE": str(root / "ca.crt"),
                "ATTUNE_LAUNCH_DOCKER_SOCKET": str(endpoint),
                "ATTUNE_LAUNCH_TLS_CERTIFICATE": str(root / "proxy.crt"), "ATTUNE_LAUNCH_TLS_PRIVATE_KEY": str(root / "proxy.key"),
                "ATTUNE_LAUNCH_CACHE_VOLUME": cache_name, "ATTUNE_LAUNCH_STATE_VOLUME": state_name,
                "ATTUNE_LAUNCH_NETWORK": network, "ATTUNE_LAUNCH_PROXY_IMAGE": PROXY, "ATTUNE_LAUNCH_TLS_BIND": "127.0.0.1"}
            model = json.loads(run([*docker_command, "compose", "-p", owner, "-f", str(ROOT / "docker-compose.workload-host.yaml"),
                "config", "--format", "json"], env=environment, capture_output=True, text=True).stdout)
            controller = model["services"]["launch-controller"]
            controller["image"] = image
            controller["ports"][0]["published"] = "0"
            controller["extra_hosts"] = {"host.docker.internal": "host-gateway"}
            for service in model["services"].values():
                service.setdefault("labels", {})["io.attune.test.owner"] = owner
            source.write_text(json.dumps(model))
            phase = "project-start"
            started = True
            run([*compose, "up", "-d", "--no-build"], env=environment)
            ids = {name: run([*compose, "ps", "-q", name], capture_output=True, text=True).stdout.strip()
                   for name in ["launch-controller", "launch-proxy"]}
            if not all(ids.values()):
                raise RuntimeError("owned project services did not start")
            for name, identifier in ids.items():
                labels = json.loads(docker("inspect", "--format", "{{json .Config.Labels}}", identifier, capture_output=True, text=True).stdout)
                if labels.get("io.attune.test.owner") != owner:
                    raise RuntimeError("project container ownership mismatch")
            ports = json.loads(docker("inspect", "--format", "{{json .NetworkSettings.Ports}}", ids["launch-controller"], capture_output=True, text=True).stdout)
            if set(key for key, value in ports.items() if value) != {"8443/tcp"}:
                raise RuntimeError("plaintext or unexpected listener was published")
            port = int(ports["8443/tcp"][0]["HostPort"])
            context = ssl.create_default_context(cafile=str(root / "ca.crt"))
            client = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context))
            origin = f"https://127.0.0.1:{port}"
            deadline = time.monotonic() + 120
            phase = "actual-startup-and-binding"
            while time.monotonic() < deadline:
                try:
                    if client.open(origin + "/health", timeout=3).status == 200:
                        ready = True
                        break
                except (OSError, urllib.error.URLError):
                    pass
                time.sleep(0.1)
            if not ready:
                raise RuntimeError("actual controller/proxy startup prerequisites failed")
            phase = "tls-and-caller-denials"
            # A trust failure must happen before any HTTP/caller credential.
            for trusted, hostname in [(False, "127.0.0.1"), (True, "wrong-owned-hostname.invalid")]:
                tls = ssl.create_default_context(cafile=str(root / "ca.crt")) if trusted else ssl.create_default_context()
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=3) as transport:
                        with tls.wrap_socket(transport, server_hostname=hostname):
                            raise RuntimeError("untrusted CA/hostname unexpectedly accepted")
                except ssl.SSLCertVerificationError:
                    pass
            request_body = json.dumps({"launch_id": 1, "admitted_digest": "a" * 64}).encode()
            for headers in [{}, {"x-attune-launch-dispatch": "owned-wrong-grant"},
                            {"Authorization": "Bearer " + PRIVATE_MARKER, "x-attune-launch-dispatch": "owned-wrong-grant"}]:
                request = urllib.request.Request(origin + "/v1/launches/1/status", data=request_body,
                    headers={"Content-Type": "application/json", **headers}, method="POST")
                try:
                    client.open(request, timeout=5)
                    raise RuntimeError("missing/wrong caller grant was accepted")
                except urllib.error.HTTPError as error:
                    if error.code not in {401, 403}:
                        raise RuntimeError("caller denial changed into an unrelated response") from error
            mounts = {name: json.loads(docker("inspect", "--format", "{{json .Mounts}}", identifier, capture_output=True, text=True).stdout)
                      for name, identifier in ids.items()}
            if any(m["Destination"] in {"/var/run/docker.sock", "/var/lib/attune/launch", "/var/lib/attune/controller", "/run/secrets/attune-controller-session"}
                   for m in mounts["launch-proxy"]):
                raise RuntimeError("proxy received controller authority/storage")
            if any(m["Destination"] == "/etc/attune/controller.key" for m in mounts["launch-controller"]):
                raise RuntimeError("controller received proxy TLS private key")
            if not any(m["Destination"] == "/etc/ssl/certs/ca-certificates.crt" and not m["RW"]
                       for m in mounts["launch-controller"]):
                raise RuntimeError("public CP trust roots were not read-only")
            for name, identifier in ids.items():
                policy = json.loads(docker("inspect", "--format", "{{json .HostConfig}}", identifier, capture_output=True, text=True).stdout)
                if policy["Privileged"] or not policy["ReadonlyRootfs"] or policy["PidMode"] == "host":
                    raise RuntimeError("project process confinement changed")
                allowed = {"CHOWN", "DAC_OVERRIDE", "FOWNER"} if name == "launch-controller" else set()
                if set(policy["CapAdd"] or []) != allowed or set(policy["CapDrop"] or []) != {"ALL"}:
                    raise RuntimeError("project capabilities changed")
                if not any(value.startswith("no-new-privileges") for value in policy["SecurityOpt"] or []) or any("unconfined" in value for value in policy["SecurityOpt"] or []):
                    raise RuntimeError("project security policy was relaxed")
                entrypoint = json.loads(docker("inspect", "--format", "{{json .Config.Entrypoint}}", identifier, capture_output=True, text=True).stdout)
                if "-i" not in entrypoint:
                    raise RuntimeError("project process inherited extra environment")
                if any(value.split("=", 1)[0] in {"ATTUNE_CONFIG", "JWT_SECRET", "ENCRYPTION_KEY", "DB_PASSWORD", "RABBITMQ_PASSWORD"} for value in entrypoint):
                    raise RuntimeError("project entrypoint received general manager configuration")
            networks = json.loads(docker("inspect", "--format", "{{json .NetworkSettings.Networks}}", ids["launch-controller"], capture_output=True, text=True).stdout)
            address = networks[owner + "_default"]["IPAddress"]
            phase = "external-plaintext-negative-control"
            docker("run", "--rm", "--name", owner + "-peer", "--label", f"io.attune.test.owner={owner}",
                   "--network", owner + "_default", "--user", "1000:1000", "--read-only", "--cap-drop=ALL",
                   "--security-opt=no-new-privileges:true", "--entrypoint", "python3", PEER, "-c",
                   "import socket,sys; s=socket.socket(); s.settimeout(3); result=s.connect_ex((sys.argv[1],8092)); s.close(); assert result!=0, 'plaintext backend reachable outside proxy netns'", address)
            for identifier in ids.values():
                logs, code, truncated = bounded_command([*docker_command, "logs", "--tail", "80", identifier])
                if code or truncated:
                    raise RuntimeError("private log absence cannot be established from incomplete diagnostics")
                if any(value in logs for value in [DUMMY_SESSION, PRIVATE_MARKER, "owned-wrong-grant"]):
                    raise RuntimeError("private marker appeared in project logs")
            print("actual source controller/project TLS, confinement and caller-denial checks passed; CP effects/host loops not proved", flush=True)
        except BaseException as error:
            test_error = error
            print(json.dumps({"phase": phase, "owned_project_diagnostics": diagnose(docker_command, owner)}), flush=True)
        finally:
            if started:
                try:
                    run([*compose, "down", "--remove-orphans", "--timeout", "120"])
                except BaseException as error:
                    cleanup_errors.append(type(error).__name__)
            if ready and not cleanup_errors:
                # Ready means the actual binding probe acknowledged Wait+Remove.
                # Unknown startup create outcomes retain volumes; never infer
                # absence from an empty lookup/timeout just to make cleanup pass.
                for name in [cache_name, state_name]:
                    try:
                        labels = json.loads(docker("volume", "inspect", "--format", "{{json .Labels}}", name, capture_output=True, text=True).stdout)
                        if labels.get("com.docker.compose.project") != owner:
                            raise RuntimeError("owned volume labels changed")
                        docker("volume", "rm", name)
                    except BaseException as error:
                        cleanup_errors.append(type(error).__name__)
            cp.shutdown()
            cp_thread.join(timeout=5)
            cp.server_close()
            if cp_thread.is_alive():
                cleanup_errors.append("CP fixture did not join")
            try:
                labels = json.loads(docker("image", "inspect", "--format", "{{json .Config.Labels}}", image, capture_output=True, text=True).stdout)
                if labels.get("io.attune.test.owner") != owner:
                    raise RuntimeError("owned image labels changed")
                docker("image", "rm", image)
            except BaseException as error:
                cleanup_errors.append(type(error).__name__)
        if test_error is not None or cleanup_errors:
            # No raw logs, environment, request bodies, token values or TLS key.
            raise SystemExit(f"owned project validation failed ({type(test_error).__name__}); cleanup={cleanup_errors}; owner={owner}; retained volumes={not ready}")


if __name__ == "__main__":
    main()
