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
from workload_proxy_diagnostics import classify as classify_proxy
from workload_project_probes import assertion_category, caller_rejection, capability_evidence, exception_category, tls_rejection

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
        categories = {key: logs.count(value) for key, value in signatures.items() if value in logs}
        if info["service"] == "launch-proxy":
            categories.update(classify_proxy(logs))
        reports.append({"service": info["service"], "state": info.get("state"),
            "exit": info.get("exit"), "oom": info.get("oom"),
            "categories": categories,
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


def proxy_preflight(docker_command):
    """Actual pinned nginx/UID/key/config prerequisite before a cold build."""
    with tempfile.TemporaryDirectory(prefix="attune-proxy-prerequisite-") as directory:
        root = Path(directory)
        certificates(root)
        change_owner(root / "proxy.key", 1000)
        base = [*docker_command, "run", "--network=none", "--user=1000:1000",
                "--read-only", "--cap-drop=ALL", "--security-opt=no-new-privileges:true",
                "--tmpfs", "/tmp:rw,nosuid,nodev,size=16777216,uid=1000,gid=1000,mode=0700",
                "--mount", f"type=bind,source={ROOT / 'docker/workload-launch-proxy.conf'},target=/etc/attune/launch-proxy.conf,readonly",
                "--mount", f"type=bind,source={root / 'proxy.crt'},target=/etc/attune/controller.crt,readonly",
                "--mount", f"type=bind,source={root / 'proxy.key'},target=/etc/attune/controller.key,readonly"]
        # Distinguish bound-key fixture metadata from nginx configuration. The
        # private bytes never pass through stdout, arguments or diagnostics.
        for name, command in [
            ("key_readability", ["/bin/sh", "-c", "test -r /etc/attune/controller.key && test -r /etc/attune/controller.crt"]),
            ("nginx_config", ["/usr/sbin/nginx", "-t", "-c", "/etc/attune/launch-proxy.conf"]),
            ("nginx_start", ["/usr/sbin/nginx", "-c", "/etc/attune/launch-proxy.conf", "-g", "daemon off;"]),
        ]:
            owner = "attune-proxy-prerequisite-" + uuid.uuid4().hex
            detached = ["--detach"] if name == "nginx_start" else []
            logs, code, truncated = bounded_command([*base, *detached, "--name", owner, "--label", f"io.attune.test.owner={owner}",
                "--entrypoint=/usr/bin/env", PROXY, "-i", "PATH=/usr/sbin:/usr/bin:/bin", "HOME=/tmp", "TMPDIR=/tmp", *command])
            if name == "nginx_start" and code == 0 and not truncated:
                # nginx writes its private pid after configuration, listen and
                # path initialization. An exec also requires the actual master
                # to remain running. Join/remove it below before continuing.
                deadline = time.monotonic() + 20
                code = 1
                while time.monotonic() < deadline:
                    _, checked, overflow = bounded_command([*docker_command, "exec", owner,
                        "/usr/bin/env", "-i", "PATH=/usr/sbin:/usr/bin:/bin", "/bin/sh", "-c", "test -s /tmp/nginx.pid"], 4096)
                    if checked == 0 and not overflow:
                        code = 0
                        break
                    state, inspected, overflow = bounded_command([*docker_command, "inspect", "--format", "{{.State.Status}}", owner], 4096)
                    if inspected or overflow or state.strip() != "running":
                        break
                    time.sleep(0.1)
                logs, collected, overflow = bounded_command([*docker_command, "logs", "--tail", "40", owner])
                truncated = truncated or overflow
                if collected:
                    code = 1
            # The probe has no pack code or launch authority, but still owns
            # and settles its exact container. A failed lookup is not absence.
            label, inspected, overflow = bounded_command([*docker_command, "inspect", "--format", '{{index .Config.Labels "io.attune.test.owner"}}', owner], 4096)
            settled = inspected == 0 and not overflow and label.strip() == owner
            if settled:
                for operation in [["stop", "--time", "2", owner], ["wait", owner], ["rm", owner]]:
                    _, status, overflow = bounded_command([*docker_command, *operation], 4096)
                    if status or overflow:
                        settled = False
                        break
            proxy_report({"proxy_prerequisite": name, "exit": code, "truncated": truncated,
                          "categories": classify_proxy(logs), "owned_container": owner, "cleanup_acknowledged": settled})
            if code or truncated or not settled:
                raise SystemExit("owned uid1000 proxy prerequisite failed before service build")


def proxy_report(report):
    # Persist only the already-sanitized fixed fields, including early failures
    # before service build or stack start. Never spool the raw nginx output.
    directory = ROOT / ".scratch/e2e-diagnostics"
    directory.mkdir(parents=True, exist_ok=True)
    destination = directory / ("proxy-" + uuid.uuid4().hex + ".jsonl")
    descriptor = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w") as output:
        output.write(json.dumps(report, sort_keys=True) + "\n")
        output.flush()
        os.fsync(output.fileno())
    print(json.dumps(report, sort_keys=True), flush=True)


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
    docker("pull", PROXY)
    proxy_preflight(docker_command)
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
        subphase = "none"
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
            for trusted, hostname, subphase in [(False, "127.0.0.1", "untrusted_ca"),
                                               (True, "wrong-owned-hostname.invalid", "wrong_hostname")]:
                tls = ssl.create_default_context(cafile=str(root / "ca.crt")) if trusted else ssl.create_default_context()
                tls_rejection(port, tls, hostname, subphase, proxy_report)
            request_body = json.dumps({"launch_id": 1, "admitted_digest": "a" * 64}).encode()
            for subphase, headers in [("missing_grant", {}),
                                     ("wrong_grant", {"x-attune-launch-dispatch": "owned-wrong-grant"}),
                                     ("broad_bearer", {"Authorization": "Bearer " + PRIVATE_MARKER, "x-attune-launch-dispatch": "owned-wrong-grant"})]:
                caller_rejection(client, origin, request_body, headers, subphase, proxy_report)
            phase = "authority-mounts-and-process-policy"
            subphase = "none"
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
                    proxy_report({"phase": phase, "subphase": "capability_ceiling", "service": name,
                                  **capability_evidence(policy)})
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
            proxy_report({"phase": phase, "subphase": subphase,
                          "exception_category": exception_category(error),
                          "assertion_category": assertion_category(error),
                          "owned_project_diagnostics": diagnose(docker_command, owner)})
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
