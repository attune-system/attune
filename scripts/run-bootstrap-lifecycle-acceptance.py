#!/usr/bin/env python3
"""Owned, fail-closed acceptance driver. Default operation prints a plan only."""
import argparse
import ast
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import subprocess
import sys
import stat
import time
from types import SimpleNamespace
import urllib.error
import urllib.request
import urllib.parse

import yaml

OWNER = "io.attune.acceptance.owner"
BACKENDS = ("SharedFile", "ObjectSegments")
F_CASES = (
    "pre_06005_upgrade_parks_and_restores_original_reservations_without_rewriting_history",
    "crash_after_native_put_and_restart_before_receipt_keeps_pins_until_provider_fence",
    "readmission_migration_preserves_orphan_object_and_staging_ownership_without_checksum_edits",
    "missing_upgrade_graph_is_located_and_quarantined_without_losing_cleanup_ownership",
    "requested_cleanup_closure_handles_self_cycles_without_absorbing_outside_files",
    "capture_gate_serializes_reclamation_and_rejects_late_upgrade",
    "exact_event_pin_blocks_leaf_drop_but_not_bounded_unpinned_row_cleanup",
)
PROBE_CHECKS = (
    "backend_observed", "real_worker_dispatch", "pinned_execution_retained",
    "provider_cleanup_retry", "exact_provider_version", "staging_cleanup",
    "quarantine_denies_authority", "writers_joined",
)
F_INPUTS = (
    "scripts/prepare-file-evidence-upgrade.sql",
    "migrations/20261015006005_file_evidence_retention.sql",
    "migrations/20261015006006_file_evidence_readmission.sql",
    "migrations/20261015006007_file_put_settlement.sql",
    "migrations/20261015006008_file_purpose_receipt_retention.sql",
    "crates/common/tests/file_evidence_retention_tests.rs",
)
ACTIVE_GROUPS = set()


class AcceptanceError(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise AcceptanceError(message)


def digest(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def fixture_command(args, command, binding=None):
    return [part.replace("{python}", sys.executable).replace("{source_root}", str(args.source_root)).replace("{storage_binding}", str(binding) if binding is not None else "{storage_binding}") for part in command]


def interoperability_module():
    spec = importlib.util.spec_from_file_location("owned_interop_fixture", Path(__file__).resolve().parent / "acceptance/interop_fixture.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def fixture_tools():
    return SimpleNamespace(private_input=private_input, require=require, digest=digest, command=command, fixture=fixture, private_json=private_json, sys=sys)


def private_input(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd) as source:
        metadata = os.fstat(source.fileno())
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == os.getuid() and metadata.st_mode & 0o077 == 0 and metadata.st_size <= 16384, "Fixture input must be a small user-owned private regular file")
        return json.load(source)


def validate_storage_probe(args, backend, profile):
    settings = profile.get("storage_probe", {})
    require(all(settings.get(key) is True for key in ("allow_shell_invocation", "allow_maintenance_management", "allow_native_faults")), "Explicit owned-fixture shell, maintenance management and native fault consent is required")
    script = args.source_root / "scripts/acceptance/storage_probe.py"
    require(fixture_command(args, profile.get("storage_probe_command", []), "BINDING") == [sys.executable, str(script), "--binding", "BINDING", "--execute"], "Use the concrete repo F probe command and generated private binding, not an external adapter")
    producer = script.with_name("storage_probe_producer.py")
    require(script.is_file() and producer.is_file(), "Integrate F's concrete storage_probe.py and producer before execution; no external PASS adapter")
    require(digest(script) == settings.get("source_sha256") and digest(producer) == settings.get("producer_sha256"), "Pin the actual reviewed F probe and producer source hashes")
    tree = ast.parse(script.read_text())
    require(not any(isinstance(node, ast.Constant) and isinstance(node.value, str) and "/api/v1/auth/" in node.value for node in ast.walk(tree)), "F probe still uses unregistered /api/v1/auth paths; current supported auth routes are /auth/token-login and /auth/me")
    require(not any(isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == "sql" for node in ast.walk(tree)) and not any(isinstance(node, ast.Constant) and isinstance(node.value, str) and (node.value.strip().upper().startswith("SELECT ") or node.value == "psql") for node in ast.walk(tree)), "F probe still contains Python SQL inspection; integrate its repository-backed inspector revision, never run ad hoc SQL")
    fields = {node.value for node in ast.walk(tree) if isinstance(node, ast.Constant) and isinstance(node.value, str)}
    require({"inspector_binary", "inspector_sha256", "inspector_context_file"} <= fields, "F probe does not consume the typed inspector binding; integrate the repository-backed F revision")
    require({"kind", "password", "/auth/login"} <= fields, "F probe must support the exclusive maintenance password JSON credential through canonical /auth/login; no integration/operator fallback")
    inspector = settings.get("repository_inspector", {})
    binary = Path(inspector.get("binary", ""))
    require(binary.is_absolute() and binary.is_file() and not binary.is_symlink() and binary.stat().st_uid in (0, os.getuid()) and not binary.stat().st_mode & 0o022 and os.access(binary, os.X_OK) and digest(binary) == inspector.get("sha256"), "Supply the actual centrally precompiled repository inspector and measured binary hash")
    observer = private_input(settings.get("observer_credential_file", ""))
    project = "eaccept-" + args.run_id + "-" + backend.lower()
    require((observer.get("run_id"), observer.get("project")) == (args.run_id, project), "Database observer credential is not owned by this run/project")
    require(observer.get("user") not in (None, "", "attune", "postgres") and isinstance(observer.get("password"), str) and observer["password"] and observer.get("database") == "attune" and observer.get("schema") == profile.get("database_schema", "attune"), "Supply a separately provisioned SELECT-only observer DB principal for this application's schema, not its admin/application user")
    require(settings.get("inspection_service") in profile["writers"], "Inspection service must be a declared run-owned writer")
    require(settings.get("writer_services") and set(settings["writer_services"]) <= set(profile["writers"]) and profile["action_host"] in settings["writer_services"], "List every possible action writer service, including the selected action host")
    require(30 <= settings.get("poll_seconds", 0) <= 900 and 120 <= settings.get("hold_seconds", 0) <= 3600, "Set bounded real storage probe polling/producer lifetimes")
    legacy = private_input(settings.get("legacy_fixture_file", ""))
    project = "eaccept-" + args.run_id + "-" + backend.lower()
    require((legacy.get("run_id"), legacy.get("project"), legacy.get("backend")) == (args.run_id, project, backend), "Real legacy-writer evidence belongs to a different run/project/backend")
    require(type(legacy.get("version_id")) is int and legacy["version_id"] > 0 and re.fullmatch(r"core\.[a-zA-Z0-9_]+", legacy.get("artifact_ref", "")) and re.fullmatch(r"core\.[a-zA-Z0-9_]+", legacy.get("owner_ref", "")), "Supply real old-writer version/artifact/action-owner identities; no fabricated legacy rows or unknown-settlement flags")
    if backend == "ObjectSegments":
        require(Path(settings.get("aws", "")).is_file() and os.access(settings["aws"], os.X_OK), "ObjectSegments needs the real host AWS CLI for the owned provider inventory")
    return legacy


def bind_storage_probe(stack, maintenance_credential, invoker_identity):
    settings = stack.profile["storage_probe"]
    legacy = validate_storage_probe(stack.args, stack.backend, stack.profile)
    binding = {key: settings[key] for key in ("inspection_service", "writer_services", "poll_seconds", "hold_seconds")}
    require(type(invoker_identity) is int and invoker_identity > 0, "Bind the inspector to the ordinary invoker ID returned by real public provisioning")
    observer = private_input(settings["observer_credential_file"])
    database_file = stack.root / "storage-inspector.private.json"
    private_json(database_file, {"database_url": "postgresql://" + urllib.parse.quote(observer["user"], safe="") + ":" + urllib.parse.quote(observer["password"], safe="") + "@" + stack.endpoint("postgres", 5432) + "/attune", "schema": observer["schema"], "access": {"run_id": stack.args.run_id, "project": stack.project, "invoker_identity": invoker_identity, "legacy_version_ids": [legacy["version_id"]], "identity_key": secrets.token_hex(32)}})
    binding.update(run_id=stack.args.run_id, project=stack.project, backend=stack.backend, maintenance_credential_file=str(maintenance_credential), allow_native_faults=settings["allow_native_faults"], python="python3", artifacts_root="/opt/attune/artifacts", blob_root="/opt/attune/blobs", legacy_version_id=legacy["version_id"], inspector_binary=settings["repository_inspector"]["binary"], inspector_sha256=settings["repository_inspector"]["sha256"], inspector_context_file=str(database_file))
    if stack.backend == "ObjectSegments":
        provider = stack.profile["provider_service"]
        port = stack.profile.get("provider_port", 9000)
        endpoint = "http://" + stack.endpoint(provider, port)
        provider_file = stack.root / "storage-provider.private.json"
        private_json(provider_file, {"project": stack.project, "bucket": stack.bucket, "endpoint": endpoint, "region": "us-east-1", "access_key_id": stack.storage_access, "secret_access_key": stack.storage_secret})
        binding.update(provider_credentials_file=str(provider_file), provider_service=provider, provider_port=port, provider_endpoint=endpoint, object_prefix=stack.project + "/", aws=settings["aws"])
    path = stack.root / "storage-probe.private.json"
    private_json(path, binding)
    consent = {"allow_shell_invocation": True, "allow_maintenance_management": True, "legacy_artifact_ref": legacy["artifact_ref"], "legacy_owner_ref": legacy["owner_ref"]}
    return path, consent


def cli(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, epilog="Examples:\n  %(prog)s --manifest scripts/acceptance/bootstrap-lifecycle.example.json\n  %(prog)s --manifest /owned/fixture.json --source-root /owned/integrated --execute\n  %(prog)s --manifest /owned/fixture.json --execute --build-tools --permit-build", formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--manifest", type=Path, default=Path(__file__).resolve().parent / "acceptance/bootstrap-lifecycle.example.json", help="Explicit central fixture/artifact manifest; default example is PLAN ONLY, no credential values on argv")
    parser.add_argument("--source-root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--run-id", default="ea" + secrets.token_hex(6))
    parser.add_argument("--work-root", type=Path, help="Must not exist; retained evidence directory, never recursively removed")
    parser.add_argument("--execute", action="store_true", help="Explicitly permit creation of this run's disposable infrastructure")
    parser.add_argument("--build-tools", action="store_true", help="Build only the archive helper, with Cargo -j 2 in an owned target directory")
    parser.add_argument("--permit-build", action="store_true", help="Central build scheduler permission; required with --build-tools")
    args = parser.parse_args(argv)
    require(bool(re.fullmatch(r"[a-z][a-z0-9-]{0,19}", args.run_id)), "--run-id must be 1..20 lowercase letters/digits/hyphens, starting with a letter")
    require(not args.build_tools or args.permit_build, "--build-tools requires --permit-build from the central two-job lane")
    args.source_root = args.source_root.resolve()
    args.work_root = (args.work_root or Path("/tmp/opencode") / ("bootstrap-acceptance-" + args.run_id)).resolve()
    return args


def plan(args, manifest):
    projects = {backend: "eaccept-" + args.run_id + "-" + backend.lower() for backend in BACKENDS}
    commands = {}
    for backend, project in projects.items():
        profile = manifest.get("profiles", {}).get(backend, {})
        base = ["docker", "--context", manifest.get("docker_context", "<explicit-local-context>"), "compose", "--project-name", project, "--env-file", "/dev/null", "-f", str(args.work_root / backend / "compose.private.json")]
        commands[backend] = [base + ["up", "-d", "--no-build", "--pull", "never", "--no-deps", *profile.get("foundation", [])], base + ["run", "--pull", "never", "--rm", "--no-deps", profile.get("migrations", "<ordinary-migrator>")], base + ["up", "-d", "--no-build", "--pull", "never", "--no-deps", *profile.get("writers", [])], base + ["down", "--timeout", "30"]]
    return {
        "mode": "execute" if args.execute else "DRY_RUN", "run_id": args.run_id,
        "work_root": str(args.work_root), "max_build_jobs": 2,
        "profiles_concurrent": 1, "f_test_threads": 4,
        "projects": projects, "planned_compose_commands": commands,
        "resource_contract": {"label": OWNER + "=" + args.run_id, "host_ports": "engine-assigned loopback API/PG and owned ObjectSegments provider only", "volumes": "new project-owned named volumes only", "cleanup": "join foreground probes; stop/wait writers; verify every resource label; down without volumes; remove exact owned volumes only after no containers remain"},
        "storage_probe_commands": {backend: fixture_command(args, manifest.get("profiles", {}).get(backend, {}).get("storage_probe_command", []), args.work_root / backend / "storage-probe.private.json") for backend in BACKENDS},
        "build_commands": [["cargo", "build", "--locked", "-p", "attune-common", "--example", "bootstrap_acceptance_archive", "-j", "2", "--target-dir", str(args.work_root / "tool-build")]] if args.build_tools else [],
        "required_profiles": list(BACKENDS),
        "stages": ["verify immutable current image/build provenance", "extract actual packaged core; verify real native ELF inputs; canonical PackStorage archive and measured digest", "fresh isolated platform; reject unconverted bootstrap contract", "ordinary operator fixture supplies existing integration token", "real API fresh upload and override=77", "wrong-pin and unapproved external upgrade denial preserve release/policy/config", "successful upgrade preserves override; ordinary readmission rollback restores original pins/defaults", "required actual backend/worker/physical cleanup probe", "required real old-image prepublication-skew probe", "STOP and WAIT every application writer", "run exact F pre-06005/06006/quarantine/physical-settlement/event-twin tests on the owned PG18 cluster", "verified owned teardown"],
        "f_cases": list(F_CASES), "required_integrated_sources": list(F_INPUTS),
        "external_prerequisites": ["Docker Compose v2, immutable compatible service/init images and central source provenance", "explicit existing operator credential for the owned deployment; repo public-API provisioner supplies no first-admin bypass", "integrated F repository-backed probe/producer plus matching precompiled inspector and SELECT-only owned DB observer credential", "real run-owned legacy-writer evidence; no SQL row manufacture", "F consumes the separate maintenance password JSON through canonical /auth/login; audit unscoped-Access guard stays unchanged", "actual incompatible API image and expected protocol for the repo old-image probe", "matching precompiled F test binary and SHA-256 from the central build", "coordinated converted API image; runner NEVER flips release gates"],
        "helm": {"render_requested": bool(manifest.get("helm")), "live_status": "EXTERNAL_PREREQUISITE", "prerequisites": ["explicit chart path and values file for offline render", "external disposable cluster/context and namespace owner for live acceptance", "compatible images/credential Secret", "helm --wait platform-first readiness; post-install publication/content hooks"]},
        "overall_full_sow_acceptance": "NOT_CLAIMED; live Helm/SDK/other owned matrices are separate",
    }


def validate(args, manifest):
    require(os.name == "posix", "Execution requires a POSIX host for owned process-group teardown")
    require(not args.work_root.exists(), "Work root exists. Choose a new run ID; no adoption or reset is allowed")
    require(manifest.get("schema_version") == 1, "Fixture manifest schema_version must be 1")
    require(isinstance(manifest.get("docker_context"), str) and manifest["docker_context"], "Supply an explicit LOCAL Docker context; never inherit a production context")
    require(set(manifest.get("profiles", {})) == set(BACKENDS), "Both SharedFile and ObjectSegments profiles are REQUIRED; none may be skipped")
    require(re.fullmatch(r"[0-9a-f]{40}", manifest.get("source_revision", "")), "Supply centralized source_revision build provenance")
    require(re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", manifest.get("init_packs_image", "")), "Supply an actual immutable current init-packs image reference")
    for relative in F_INPUTS:
        path = args.source_root / relative
        require(path.is_file(), "Integrated source prerequisite is missing: " + relative)
        require(manifest.get("migration_sha256", {}).get(relative) == digest(path), "Missing/mismatched actual source digest for " + relative)
    binary = Path(manifest.get("f_test_binary", ""))
    require(binary.is_file() and os.access(binary, os.X_OK), "Supply matching precompiled F test binary; this driver never silently builds it")
    require(digest(binary) == manifest.get("f_test_sha256"), "F test binary digest mismatch")
    if not args.build_tools:
        tool = Path(manifest.get("archive_tool", ""))
        require(tool.is_file() and os.access(tool, os.X_OK), "Supply central prebuilt bootstrap_acceptance_archive, or explicitly permit its -j 2 build")
        require(digest(tool) == manifest.get("archive_tool_sha256"), "Archive helper digest mismatch; use the central actual binary")
    if manifest.get("helm"):
        require(shutil.which("helm") is not None and Path(manifest["helm"]["chart"]).is_dir() and Path(manifest["helm"]["values"]).is_file(), "Explicit optional Helm render prerequisites are missing")
    for backend, profile in manifest["profiles"].items():
        expected = profile.get("old_expected_contract", {})
        require("bootstrap_admission_protocol" in expected and (type(expected["bootstrap_admission_protocol"]) is not int or expected["bootstrap_admission_protocol"] != 1), "Explicit incompatible old-image protocol is required; gate-OFF protocol 1 is not skew")
        operator_file = Path(profile.get("operator_credential_file", ""))
        require(operator_file.is_file(), backend + ": provide a private existing operator credential for this owned deployment; there is no first-admin API bypass")
        require(re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", profile.get("old_image", "")), backend + ": supply an actual immutable incompatible old image for the skew fixture")
        require(not any("ADMISSION_ENABLED" in key or "RECORDED_WORKER" in key or (key.endswith("SECURITY__ENABLE_AUTH") and str(value).lower() == "false") for key, value in profile.get("environment", {}).items()), "Do not bypass auth or independently flip admission/recorded-worker gates; use a centrally coordinated image")
        require(Path(profile.get("compose_file", "")).is_file(), backend + ": supply a real standalone/current Compose fixture")
        writers = profile.get("writers", [])
        groups = profile.get("foundation", []) + profile.get("initializers", []) + writers + [profile.get("migrations")]
        require(all(isinstance(name, str) and re.fullmatch(r"[a-z][a-z0-9_-]*", name) for name in groups), "Use explicit safe service names, never command arguments")
        require(len(groups) == len(set(groups)) and {"postgres", "rabbitmq"} <= set(profile.get("foundation", [])), backend + ": service roles must be disjoint with owned postgres/rabbitmq foundations")
        require(all(re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", profile.get("images", {}).get(name, "")) for name in groups), backend + ": supply every actual immutable fixture image before building or starting")
        require({"api", "executor", "supervisor", "notifier", "web", profile.get("action_host"), profile.get("sensor_host")} <= set(writers), backend + ": declare every API/worker/sensor/consumer writer, including UI")
        require(profile.get("foundation") and profile.get("migrations"), backend + ": explicit foundation and ordinary migration service are required")
        require(backend != "ObjectSegments" or (profile.get("versioned_provider") is True and profile.get("provider_service") in profile["foundation"]), "ObjectSegments requires a run-owned versioned provider service and probe")
        validate_storage_probe(args, backend, profile)
        interoperability_module().validate(args, profile, fixture_tools(), backend)
        for hook in ("provision_command", "storage_probe_command", "skew_probe_command"):
            argv = profile.get(hook)
            require(isinstance(argv, list) and argv and all(isinstance(part, str) for part in argv), backend + ": missing required foreground " + hook)
            resolved = fixture_command(args, argv)
            require(Path(resolved[0]).is_file() and os.access(resolved[0], os.X_OK), backend + ": required fixture executable unavailable: " + hook)


def command(argv, *, data=None, env=None, timeout=300, cwd=None):
    """One foreground process group; await termination even on cancellation."""
    if env is None:
        env = {key: os.environ[key] for key in ("PATH", "HOME", "DOCKER_CONFIG", "XDG_RUNTIME_DIR") if key in os.environ}
    process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env, cwd=cwd, start_new_session=True)
    ACTIVE_GROUPS.add(process.pid)
    communicated = False
    try:
        out, _ = process.communicate(data, timeout=timeout)
        communicated = True
        require(process.returncode == 0, "Command failed, exit " + str(process.returncode) + ": " + str(argv[0]) + " (raw output suppressed to protect credentials)")
        return out
    finally:
        # Kill descendants left by a fixture as well; foreground fixture hooks
        # must not daemonize/escape their process group.
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        if not communicated:
            try:
                process.communicate(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()
        process.wait()
        deadline = time.monotonic() + 10
        while True:
            try:
                os.killpg(process.pid, 0)
            except ProcessLookupError:
                ACTIVE_GROUPS.discard(process.pid)
                break
            os.killpg(process.pid, signal.SIGKILL)
            require(time.monotonic() < deadline, "Fixture process group has not joined; refusing volume cleanup")
            time.sleep(0.05)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        return None


def api_request(url, *, data=None, headers=None, method=None):
    require(urllib.parse.urlsplit(url).hostname == "127.0.0.1", "Acceptance HTTP must target the owned loopback API")
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    return opener.open(urllib.request.Request(url, data=data, headers=headers or {}, method=method), timeout=15)


def http(base, path, token=None, body=None):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    with api_request(base + path, data=json.dumps(body).encode() if body is not None else None, headers=headers, method="PUT" if body is not None else "GET") as response:
        return json.load(response)


def snapshot(base, token):
    core = http(base, "/health/bootstrap")["core"]
    require(all(core.get(key) for key in ("release_id", "policy_revision", "digest", "version")), "Bootstrap proof lacks exact release/policy pins")
    releases = http(base, "/api/v1/packs/core/releases", token)["data"]
    require(isinstance(releases, list) and releases, "Release inventory is missing")
    pins = {str(release["id"]): {key: release[key] for key in ("version", "digest")} for release in releases}
    require(str(core["release_id"]) in pins, "Active release is absent from retained inventory")
    return {"core": core, "config": http(base, "/api/v1/packs/core", token)["data"]["config"], "releases": pins}


def check_snapshot(actual, expected):
    require(actual == expected, "A rejected upgrade mutated persisted release/policy/config pins")


def upload_outcome(root, label, publisher, base, token, source, expected_digest, before):
    """Observe once; never retry an HTTP failure that may have committed."""
    record = {"before": before, "response": "unknown", "observed_state": "unavailable"}
    try:
        publisher.upload(base, token, source, expected_digest)
        record["response"] = "accepted"
        after = snapshot(base, token)
        record.update(after=after, observed_state="unchanged" if after == before else "changed")
        return after
    except Exception as error:
        record["error_type"] = type(error).__name__  # Never persist secret-bearing HTTP bodies.
        if record["response"] == "unknown":
            record["response"] = "error"
            try:
                after = snapshot(base, token)
                record.update(after=after, observed_state="unchanged" if after == before else "changed")
            except Exception:
                pass  # Unavailable observation stays UNKNOWN, not rollback or success.
        raise
    finally:
        private_json(root / (label + ".outcome.private.json"), record)


def require_external_approval_denial(error):
    status = error.code
    try:
        message = error.read(1024 * 1024).decode(errors="replace")
    finally:
        error.close()
    require(status == 403 and "External runtime scope requires separate approval" in message, "Failed upgrade denied for an unrelated reason; require the actual fixed-external-approval denial")


def private_json(path, value):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as target:
        json.dump(value, target)


class Stack:
    def __init__(self, args, manifest, backend, profile):
        self.args, self.manifest, self.backend, self.profile = args, manifest, backend, profile
        self.project = plan(args, manifest)["projects"][backend]
        self.root = args.work_root / backend
        self.file = self.root / "compose.private.json"
        self.password = secrets.token_hex(24)
        self.jwt_secret, self.encryption_key = secrets.token_hex(32), secrets.token_hex(32)
        self.storage_access, self.storage_secret = secrets.token_hex(12), secrets.token_hex(32)
        self.mq_password = secrets.token_hex(24)
        self.bucket = "eaccept-" + args.run_id + "-" + backend.lower()
        self.created = False
        self.external_stage_unjoined = False

    def docker(self, *args):
        return command(["docker", "--context", self.manifest["docker_context"], *args])

    def compose(self, *args):
        return command(["docker", "--context", self.manifest["docker_context"], "compose", "--project-name", self.project, "--project-directory", str(Path(self.profile["compose_file"]).resolve().parent), "--env-file", "/dev/null", "-f", str(self.file), *args], timeout=900)

    def ids(self, kind, owned_only=True):
        flag = ["ps", "-aq"] if kind == "container" else [kind, "ls", "-q"]
        owner = ["--filter", "label=" + OWNER + "=" + self.args.run_id] if owned_only else []
        return self.docker(*flag, *owner, "--filter", "label=com.docker.compose.project=" + self.project).split()

    def assert_owned(self, kind, identifier):
        value = json.loads(self.docker(kind, "inspect", identifier))[0]
        labels = value.get("Config", {}).get("Labels", {}) if kind == "container" else value.get("Labels", {})
        require(labels.get(OWNER) == self.args.run_id and labels.get("com.docker.compose.project") == self.project, "Refusing cleanup of foreign " + kind)
        return value

    def prepare(self):
        # Check project name too, not only our random owner label.
        for kind in ("container", "volume", "network"):
            flags = ["ps", "-aq"] if kind == "container" else [kind, "ls", "-q"]
            require(not self.docker(*flags, "--filter", "label=com.docker.compose.project=" + self.project).strip(), "Project already exists; refuse adoption")
        self.docker("image", "inspect", self.profile["old_image"])
        self.root.mkdir(mode=0o700)
        model = yaml.safe_load(Path(self.profile["compose_file"]).read_text())
        require(not model.get("include"), "Compose includes must be resolved into the explicit owned fixture")
        selected = set(self.profile["foundation"] + self.profile.get("initializers", []) + self.profile["writers"] + [self.profile["migrations"]])
        require(selected <= set(model["services"]), "Fixture lacks declared services")
        require("init-user" not in selected, "SQL application-user bootstrap is not an acceptance credential fixture; use the ordinary operator provisioner")
        model["services"] = {name: model["services"][name] for name in selected}
        model.pop("name", None)
        if any(not service.get("networks") for service in model["services"].values()):
            model.setdefault("networks", {}).setdefault("default", {})
        for category in ("volumes", "networks"):
            for value in model.get(category, {}).values():
                require(isinstance(value, dict), "Declare explicit resource maps in the acceptance fixture")
                require(not value.get("external"), "External resources are forbidden")
                require(not value.get("driver_opts") and value.get("driver", "local" if category == "volumes" else "bridge") in ("local", "bridge"), "Host-bind or nonlocal resource drivers are forbidden")
                value.pop("name", None)
                value.pop("ipam", None)
                value["labels"] = {OWNER: self.args.run_id}
        for name, service in model["services"].items():
            require(not any(service.get(key) for key in ("extends", "env_file", "volumes_from", "privileged", "devices", "network_mode", "external_links", "secrets", "configs", "use_api_socket")) and service.get("pid") != "host" and service.get("ipc") != "host", "Fixture service escapes isolation or inherits unowned credentials: " + name)
            image = self.profile.get("images", {}).get(name, "")
            require(re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", image), "Missing immutable fixture image: " + name)
            info = json.loads(self.docker("image", "inspect", image))[0]
            if name in self.profile["writers"] or name == "init-agent":
                # Stock worker hosts may wrap an independently pinned agent image;
                # central provenance must explicitly attest their current injection.
                require(name in (self.profile["action_host"], self.profile["sensor_host"]) or info.get("Config", {}).get("Labels", {}).get("org.opencontainers.image.revision") == self.manifest["source_revision"], "Current source provenance mismatch: " + name)
            service["image"] = image
            for key in ("build", "container_name", "depends_on", "ports", "restart", "profiles"):
                service.pop(key, None)
            service["restart"] = "no"
            service["labels"] = {OWNER: self.args.run_id}
            for mount in service.get("volumes", []):
                require(isinstance(mount, str), "Use explicit short read-only source mounts or owned named volumes")
                source = mount.split(":", 1)[0]
                if source.startswith(("/", ".")):
                    resolved = (Path(self.profile["compose_file"]).resolve().parent / source).resolve()
                    require(mount.endswith(":ro") and resolved.is_relative_to(self.args.source_root), "Unowned writable/out-of-source bind mount is forbidden")
                else:
                    require(source in model.get("volumes", {}), "Anonymous or undeclared volumes are forbidden")
            environment = service.setdefault("environment", {})
            require(isinstance(environment, dict), "Fixture environment must be an explicit map")
            environment.update(self.profile.get("environment", {}))
            if name in self.profile["writers"]:
                environment["ATTUNE__ARTIFACTS__TRANSPORT"] = "volume" if self.backend == "SharedFile" else "api"
                environment["ATTUNE__ARTIFACTS_DIR"] = "/opt/attune/artifacts"
                environment["ATTUNE_API_URL"] = "http://api:8080"
                if name != "web":
                    environment["ATTUNE__MESSAGE_QUEUE__URL"] = "amqp://attune:" + self.mq_password + "@rabbitmq:5672/attune_" + self.args.run_id
                    environment["ATTUNE_MQ_URL"] = environment["ATTUNE__MESSAGE_QUEUE__URL"]
            if name == "web":
                # Browser requests use the real web origin's nginx proxy.
                environment.update(API_URL="", WS_URL="")
            if name == "rabbitmq":
                environment.update(RABBITMQ_DEFAULT_USER="attune", RABBITMQ_DEFAULT_PASS=self.mq_password, RABBITMQ_DEFAULT_VHOST="attune_" + self.args.run_id)
            if name in ("api", "executor", "supervisor", "notifier"):
                environment.update({"ATTUNE__DATABASE__URL": "postgresql://attune:" + self.password + "@postgres:5432/attune", "ATTUNE__SECURITY__JWT_SECRET": self.jwt_secret, "ATTUNE__SECURITY__ENCRYPTION_KEY": self.encryption_key, "ATTUNE__SECURITY__ENABLE_AUTH": "true"})
                if self.backend == "SharedFile":
                    environment.update(ATTUNE__STORAGE__PROVIDER="filesystem", ATTUNE__STORAGE__ROOT="/opt/attune/blobs")
                else:
                    provider = self.profile["provider_service"]
                    endpoint = self.profile.get("provider_endpoint", "http://" + provider + ":9000")
                    require(urllib.parse.urlsplit(endpoint).hostname == provider, "Object endpoint must be the run-owned provider service, never a remote bucket")
                    environment.update(ATTUNE__STORAGE__PROVIDER="s3", ATTUNE__STORAGE__ENDPOINT=endpoint, ATTUNE__STORAGE__BUCKET=self.bucket, ATTUNE__STORAGE__PREFIX=self.project + "/", ATTUNE__STORAGE__REGION="us-east-1", AWS_ACCESS_KEY_ID=self.storage_access, AWS_SECRET_ACCESS_KEY=self.storage_secret)
            if self.backend == "ObjectSegments" and name == self.profile["provider_service"]:
                environment.update(RUSTFS_ACCESS_KEY=self.storage_access, RUSTFS_SECRET_KEY=self.storage_secret)
            if self.backend == "ObjectSegments" and name in self.profile.get("initializers", []):
                environment.update(AWS_ACCESS_KEY_ID=self.storage_access, AWS_SECRET_ACCESS_KEY=self.storage_secret, AWS_DEFAULT_REGION="us-east-1", AWS_ENDPOINT_URL=self.profile.get("provider_endpoint", "http://" + self.profile["provider_service"] + ":9000"), ATTUNE_ACCEPTANCE_BUCKET=self.bucket)
            if name == self.profile["migrations"]:
                environment.update(DB_HOST="postgres", DB_PORT="5432", DB_USER="attune", DB_PASSWORD=self.password, DB_NAME="attune", DB_SCHEMA="attune", DATABASE_URL="postgresql://attune:" + self.password + "@postgres:5432/attune")
        pg = model["services"]["postgres"]
        require("postgres:18" in pg["image"], "Owned database must be PostgreSQL 18")
        pg["environment"].update(POSTGRES_USER="attune", POSTGRES_PASSWORD=self.password, POSTGRES_DB="attune")
        def named_mount(name, target):
            for mount in model["services"][name].get("volumes", []):
                parts = mount.split(":")
                if len(parts) >= 2 and parts[1] == target and parts[0] in model.get("volumes", {}) and (len(parts) == 2 or parts[2] != "ro"):
                    return parts[0]
            raise AcceptanceError("Missing run-owned writable named volume for " + name + ":" + target)

        if self.backend == "SharedFile":
            blob_volume = named_mount("api", "/opt/attune/blobs")
            for name in ("executor", "supervisor"):
                if name in model["services"]:
                    require(named_mount(name, "/opt/attune/blobs") == blob_volume, "Filesystem API/executor/supervisor must share the OWNED blob provider volume")
            artifact_volume = named_mount("api", "/opt/attune/artifacts")
            for name in (self.profile["action_host"], self.profile["sensor_host"]):
                require(named_mount(name, "/opt/attune/artifacts") == artifact_volume, "SharedFile API/action/sensor hosts must share the OWNED artifact volume")
        for name, port in (("api", 8080), ("postgres", 5432), ("web", 80)):
            model["services"][name]["ports"] = [{"target": port, "published": "0", "host_ip": "127.0.0.1"}]
        if self.profile.get("storage_probe"):
            inspection = self.profile["storage_probe"]["inspection_service"]
            require(named_mount(inspection, "/opt/attune/artifacts") == named_mount("api", "/opt/attune/artifacts"), "F inspection service must see the exact owned API staging volume")
            if self.backend == "SharedFile":
                require(named_mount(inspection, "/opt/attune/blobs") == named_mount("api", "/opt/attune/blobs"), "F inspection service must see the exact owned filesystem blob volume")
            else:
                port = self.profile.get("provider_port", 9000)
                require(type(port) is int and 1 <= port <= 65535, "Invalid owned provider port")
                model["services"][self.profile["provider_service"]]["ports"] = [{"target": port, "published": "0", "host_ip": "127.0.0.1"}]
        private_json(self.file, model)
        self.created = True  # Partial starts still require verified teardown.

    def up(self, services):
        if services:
            self.compose("up", "-d", "--no-build", "--pull", "never", "--no-deps", *services)

    def endpoint(self, name, port):
        result = self.compose("port", name, str(port)).strip()
        require(re.fullmatch(r"127\.0\.0\.1:[0-9]+", result), "Expected an engine-assigned loopback port")
        return result

    def stop_writers(self):
        writers = []
        for identifier in self.ids("container", owned_only=False):
            value = self.assert_owned("container", identifier)
            if value.get("Config", {}).get("Labels", {}).get("com.docker.compose.service") not in self.profile["foundation"]:
                writers.append(identifier)
        if writers:
            self.docker("stop", "--time", "30", *writers)
        for identifier in writers:
            self.docker("wait", identifier)
            value = self.assert_owned("container", identifier)
            require(not value["State"]["Running"] and not value["State"].get("Restarting"), "Writer has not stopped; no preflight or volume deletion is allowed")
        require(not ACTIVE_GROUPS, "Foreground fixture writers have not joined")

    def cleanup(self):
        if not self.created:
            return
        self.stop_writers()
        require(not ACTIVE_GROUPS, "Fixture writers have not joined; retain DB/provider volumes")
        require(not self.external_stage_unjoined, "G interoperability failed/timed out without complete joined-child/browser evidence; retain owned volumes and private recovery state")
        for kind in ("container", "volume", "network"):
            for identifier in self.ids(kind, owned_only=False):
                self.assert_owned(kind, identifier)
        volumes = self.ids("volume")
        self.compose("down", "--timeout", "30")  # NEVER --volumes here.
        for identifier in self.ids("container", owned_only=False):
            self.assert_owned("container", identifier)
            self.docker("stop", "--time", "30", identifier)
            self.docker("wait", identifier)
            value = self.assert_owned("container", identifier)
            require(not value["State"]["Running"], "Owned orphan writer still running")
            self.docker("rm", identifier)
        require(not self.ids("container", owned_only=False), "Containers remain; refusing to delete DB volumes")
        for network in self.ids("network", owned_only=False):
            self.assert_owned("network", network)
            self.docker("network", "rm", network)
        for volume in volumes:
            self.assert_owned("volume", volume)
            self.docker("volume", "rm", volume)
        require(not self.ids("volume") and not self.ids("network"), "Owned teardown incomplete; evidence retained")
        self.file.unlink()  # Exact private file only; retain all non-secret evidence.
        for name in ("storage-probe.private.json", "storage-inspector.private.json", "storage-provider.private.json", "g-interoperability-binding.private.json", "g-interop-principals.private.json", "g-runtime-observer-template.private.json", *["g-interop-" + role + ".private.json" for role in ("deployer", "maintenance", "runtimeconsumer", "reader", "sponsor")]):
            path = self.root / name
            if path.exists():
                path.unlink()


def archive(tool, source, output):
    result = json.loads(command([str(tool), str(source), str(output)], timeout=900))
    path = Path(result["archive_path"])
    require(path.is_relative_to(output.resolve()) and path.is_file(), "Archive helper returned a foreign/missing path")
    require(digest(path) == result["digest"], "Actual canonical archive digest mismatch")
    return result


def fixture(argv, context, timeout=1800):
    result = json.loads(command(argv, data=json.dumps(context), timeout=timeout))
    require(isinstance(result, dict), "Fixture must return one bounded JSON object")
    return result


def required_f_cases(binary, env):
    listed = command([binary, "--list"])
    require(all(name + ": test" in listed.splitlines() for name in F_CASES), "Precompiled F inventory is incomplete; refusing zero-test success")
    for name in F_CASES:
        output = command([binary, name, "--exact", "--test-threads=4"], env=env, timeout=1800)
        require("1 passed; 0 failed; 0 ignored;" in output, "Required F case did not execute exactly once: " + name)


def lifecycle(stack, tool, source, published, publisher):
    profile, root = stack.profile, stack.root
    stack.up(profile["foundation"])
    deadline = time.monotonic() + 120
    while True:
        try:
            stack.compose("exec", "-T", "postgres", "pg_isready", "-U", "attune", "-d", "attune")
            break
        except AcceptanceError:
            require(time.monotonic() < deadline, "Owned PostgreSQL 18 did not become ready")
            time.sleep(0.25)
    pg_version = stack.compose("exec", "-T", "postgres", "psql", "-X", "-U", "attune", "-d", "attune", "-Atqc", "SHOW server_version_num").strip()
    require(pg_version.isdigit() and int(pg_version) // 10000 == 18, "Owned server is not actually PostgreSQL 18")
    stack.compose("run", "--pull", "never", "--rm", "--no-deps", profile["migrations"])
    for initializer in profile.get("initializers", []):
        stack.compose("run", "--pull", "never", "--rm", "--no-deps", initializer)
    stack.up(profile["writers"])
    base = "http://" + stack.endpoint("api", 8080)
    deadline = time.monotonic() + 180
    while True:
        try:
            ready = http(base, "/health/ready")
            break
        except (OSError, urllib.error.URLError):
            require(time.monotonic() < deadline, "API did not reach platform readiness")
            time.sleep(0.25)
    try:
        publisher.require_contract(ready)
    except RuntimeError:
        raise AcceptanceError("Unconverted bootstrap readiness contract. Supply the centrally coordinated compatible image; this runner never flips gates") from None
    credential = root / "operator-token.private"
    invoker_credential = root / "invoker-token.private"
    maintenance_credential = root / "maintenance-credential.private.json"
    binding_path = root / "storage-probe.private.json"
    legacy = validate_storage_probe(stack.args, stack.backend, profile)
    storage_consent = {"allow_shell_invocation": True, "allow_maintenance_management": True, "legacy_artifact_ref": legacy["artifact_ref"], "legacy_owner_ref": legacy["owner_ref"]}
    context = {"run_id": stack.args.run_id, "project": stack.project, "owner_label": OWNER, "docker_context": stack.manifest["docker_context"], "backend": stack.backend, "api_url": base, "credential_output": str(credential), "invoker_credential_output": str(invoker_credential), "operator_credential_file": profile["operator_credential_file"], "compose_file": str(stack.file), "source_root": str(stack.args.source_root), "old_image": profile["old_image"], "old_expected_contract": profile["old_expected_contract"], "owned_bucket": stack.bucket if stack.backend == "ObjectSegments" else None}
    context.update(maintenance_credential_output=str(maintenance_credential), storage_fixture=storage_consent)
    def invoke(hook, values):
        return fixture(fixture_command(stack.args, profile[hook], binding_path), values)
    try:
        provisioned = invoke("provision_command", context)
        ids = [provisioned["identities"][role]["id"] for role in ("installer", "invoker", "maintenance")]
        require(all(type(identity) is int and identity > 0 for identity in ids) and len(set(ids)) == 3, "Real provisioner must return distinct installer/invoker/maintenance identity IDs")
        binding_path, _ = bind_storage_probe(stack, maintenance_credential, ids[1])
        context.pop("operator_credential_file")
        context.pop("storage_fixture")
        require(provisioned.get("ordinary_operator_provisioning") is True and credential.is_file(), "Operator fixture did not supply an ordinary existing integration credential")
        require(not credential.is_symlink() and credential.stat().st_uid == os.getuid() and credential.stat().st_mode & 0o077 == 0 and credential.stat().st_size <= 4096, "Credential must be a small run-owned regular mode-0600 file")
        token = publisher.token_login(base, credential.read_text().strip())
        publisher.upload(base, token, source, published["digest"])
        first = snapshot(base, token)
        require(first["core"]["digest"] == published["digest"] and first["core"]["version"] == published["version"], "Fresh admission pins differ from actual archive")
        http(base, "/api/v1/packs/core", token, {"config": {"max_action_timeout": 77}})
        baseline = snapshot(base, token)
        require(baseline["config"]["max_action_timeout"] == 77, "Override not persisted")
        probe_state = root / "probe-state.private.json"
        context["probe_state"] = str(probe_state)
        started = invoke("storage_probe_command", dict(context, phase="begin", expected_pins={"fresh": baseline["core"]}))
        require(type(started.get("execution_id")) is int and started["execution_id"] > 0 and started.get("backend") == stack.backend, "Backend probe must start a REAL pinned execution before upgrade")
        candidate = root / "candidate" / "core"
        shutil.copytree(source, candidate)
        metadata = yaml.safe_load((candidate / "pack.yaml").read_text())
        metadata["version"] = published["version"] + "-acceptance." + stack.args.run_id
        metadata["config"].update(max_action_timeout=600, enable_debug_logging=True)
        (candidate / "pack.yaml").write_text(yaml.safe_dump(metadata))
        negative = root / "negative" / "core"
        shutil.copytree(candidate, negative)
        (negative / "permission_sets/e_acceptance_external.yaml").write_text("ref: core.acceptance_external\nscope: external\ngrants:\n  - resource: keys\n    actions: [read]\n")
        try:
            upload_outcome(root, "unapproved-upgrade", publisher, base, token, negative, None, baseline)
        except urllib.error.HTTPError as error:
            require_external_approval_denial(error)
        else:
            raise AcceptanceError("Unapproved external upgrade was accepted")
        check_snapshot(snapshot(base, token), baseline)
        upgrade = archive(tool, candidate, root / "upgrade-archive")
        wrong = ("0" if upgrade["digest"][0] != "0" else "1") + upgrade["digest"][1:]
        try:
            upload_outcome(root, "wrong-pin-upgrade", publisher, base, token, candidate, wrong, baseline)
        except urllib.error.HTTPError as error:
            status = error.code
            error.close()
            require(status == 409, "Wrong archive pin must fail before publication")
        else:
            raise AcceptanceError("Negative archive pin was accepted")
        check_snapshot(snapshot(base, token), baseline)
        upgraded = upload_outcome(root, "upgrade", publisher, base, token, candidate, upgrade["digest"], baseline)
        require(upgraded["core"]["digest"] == upgrade["digest"] and upgraded["core"]["policy_revision"] != baseline["core"]["policy_revision"], "Upgrade did not admit exact new release/policy")
        require(upgraded["config"]["max_action_timeout"] == 77 and upgraded["config"]["enable_debug_logging"] is True, "Upgrade lost override or authored defaults")
        require(all(upgraded["releases"].get(identifier) == pin for identifier, pin in baseline["releases"].items()), "Upgrade rewrote retained release pins")
        retained = invoke("storage_probe_command", dict(context, phase="upgraded", execution_id=started["execution_id"], expected_pins={"fresh": baseline["core"], "upgrade": upgraded["core"]}))
        require(retained.get("pinned_execution_retained") is True, "Old running execution was not proven against retained pins during upgrade")
        restored = upload_outcome(root, "rollback", publisher, base, token, source, published["digest"], upgraded)
        require(restored["core"]["release_id"] == baseline["core"]["release_id"] and restored["core"]["digest"] == baseline["core"]["digest"], "Rollback did not readmit retained release")
        require(restored["config"]["max_action_timeout"] == 77 and restored["config"]["enable_debug_logging"] is False, "Rollback lost persisted override/original defaults")
        require(all(restored["releases"].get(identifier) == pin for identifier, pin in upgraded["releases"].items()), "Rollback removed or rewrote retained release pins")
        require(digest(Path(published["archive_path"])) == published["digest"], "Retained original archive changed during lifecycle")
        proof = invoke("storage_probe_command", dict(context, phase="finish", execution_id=started["execution_id"], expected_pins={"fresh": baseline["core"], "upgrade": upgraded["core"], "rollback": restored["core"]}))
        require(proof.get("backend") == stack.backend and all(proof.get("checks", {}).get(check) is True for check in PROBE_CHECKS), "Actual backend/physical cleanup probe is missing required checks; no skip allowed")
        require(stack.backend != "ObjectSegments" or proof.get("bucket_versioning") == "Enabled", "Object provider versioning was not observed")
        skew = invoke("skew_probe_command", context)
        require(skew.get("actual_old_image") is True and skew.get("publication_requests") == 0 and skew.get("rejected_before_publication") is True, "Actual old-image protocol skew was not proven")
        require(skew.get("image") == profile["old_image"], "Skew fixture did not use the pinned old image")
        if skew.get("rejection_stage") == "api_protocol":
            require(skew.get("observed_http_status") == 200 and "observed_protocol" in skew and (type(skew["observed_protocol"]) is not int or skew["observed_protocol"] != 1), "An unrelated API crash is not protocol-skew acceptance")
        elif skew.get("rejection_stage") == "client_contract":
            require("observed_client_version" in skew and (type(skew["observed_client_version"]) is not int or skew["observed_client_version"] != 1), "Old client contract was not observed")
        else:
            raise AcceptanceError("Unknown old-image rejection stage")
        private_json(root / "pins.json", {"fresh": baseline, "upgrade": upgraded, "rollback": restored})
        # Concrete G provisioning, canonical assembly, all real transports and
        # browser are foreground and MUST complete before any writer teardown.
        stack.external_stage_unjoined = True
        interop = interoperability_module().stage(stack, context, fixture_tools())
        stack.external_stage_unjoined = False
        private_json(root / "g-stage-result.private.json", interop)
    finally:
        if credential.exists():
            credential.unlink()
        if invoker_credential.exists():
            invoker_credential.unlink()
        if maintenance_credential.exists():
            maintenance_credential.unlink()
    stack.stop_writers()  # F preflight MUST NOT run with application writers alive.
    env = {key: os.environ[key] for key in ("PATH", "HOME") if key in os.environ}
    env.update(ATTUNE_TEST_RUN_ID=stack.args.run_id, ATTUNE__DATABASE__URL="postgresql://attune:" + stack.password + "@" + stack.endpoint("postgres", 5432) + "/attune")
    required_f_cases(stack.manifest["f_test_binary"], env)
    return {"backend": stack.backend, "bootstrap_lifecycle": "PASS", "f_exact_cases": list(F_CASES), "writers_stopped_before_f_preflight": True}


def execute(args, manifest):
    validate(args, manifest)
    require(shutil.which("docker") is not None, "Install Docker Compose v2 before --execute")
    docker = ["docker", "--context", manifest["docker_context"]]
    inspected = json.loads(command([*docker, "context", "inspect", manifest["docker_context"]]))[0]
    require(inspected["Endpoints"]["docker"]["Host"].startswith(("unix://", "npipe://")), "Only an explicit local Docker daemon is allowed")
    command([*docker, "compose", "version"])
    for kind, flags in (("container", ["ps", "-aq"]), ("volume", ["volume", "ls", "-q"]), ("network", ["network", "ls", "-q"])):
        require(not command([*docker, *flags, "--filter", "label=" + OWNER + "=" + args.run_id]).strip(), "Owner run ID already has " + kind + " resources; choose a new run, never adopt")
    args.work_root.mkdir(mode=0o700)
    private_json(args.work_root / "owner.json", {"run_id": args.run_id, "projects": plan(args, manifest)["projects"], "fixture_manifest": str(args.manifest.resolve()), "source_root": str(args.source_root)})
    tool = Path(manifest.get("archive_tool", ""))
    if args.build_tools:
        build_env = {key: os.environ[key] for key in ("PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME") if key in os.environ}
        build_env["CARGO_BUILD_JOBS"] = "2"
        command(plan(args, manifest)["build_commands"][0], env=build_env, timeout=7200, cwd=args.source_root)
        tool = args.work_root / "tool-build/debug/examples/bootstrap_acceptance_archive"
    image_container = "eaccept-" + args.run_id + "-archive"
    created = False
    results = []
    try:
        # No launch/exec of the image: copy actual packaged files from an owned
        # stopped container, then let the canonical Rust helper verify native ELF.
        image_info = json.loads(command([*docker, "image", "inspect", manifest["init_packs_image"]]))[0]
        require(image_info.get("Config", {}).get("Labels", {}).get("org.opencontainers.image.revision") == manifest["source_revision"], "Init image does not match central source provenance")
        command([*docker, "create", "--pull", "never", "--name", image_container, "--label", OWNER + "=" + args.run_id, manifest["init_packs_image"]])
        created = True
        source = args.work_root / "core"
        command([*docker, "cp", image_container + ":/source/packs/core", str(source)])
        expected = set()
        for path in (args.source_root / "packs/core").rglob("*"):
            if path.is_file() and "__pycache__" not in path.parts:
                relative = path.relative_to(args.source_root / "packs/core")
                expected.add(str(relative))
                require(digest(path) == digest(source / relative), "Packaged core source differs from integrated source")
        actual = {str(path.relative_to(source)) for path in source.rglob("*") if path.is_file()}
        native = {"sensors/bin/" + arch + "/attune-core-timer-sensor" for arch in ("linux-amd64", "linux-arm64")}
        require(actual - expected <= native and native <= actual, "Packaged core has unreviewed extras or missing real native inputs")
        published = archive(tool.resolve(), source, args.work_root / "canonical-archive")
        spec = importlib.util.spec_from_file_location("acceptance_publisher", args.source_root / "scripts/bootstrap_core_pack.py")
        publisher = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(publisher)
        publisher.request = api_request
        for backend in BACKENDS:
            stack = Stack(args, manifest, backend, manifest["profiles"][backend])
            record = {"backend": backend, "status": "RUNNING"}
            results.append(record)
            try:
                stack.prepare()
                record.update(lifecycle(stack, tool.resolve(), source, published, publisher), status="PASS")
            except BaseException:
                record["status"] = "FAILED"
                raise
            finally:
                try:
                    stack.cleanup()
                    record["cleanup"] = "PASS"
                except BaseException:
                    record["cleanup"] = "FAILED_RETAIN_RESOURCES"
                    raise
        for flags in (["volume", "ls", "-q"], ["network", "ls", "-q"]):
            require(not command([*docker, *flags, "--filter", "label=" + OWNER + "=" + args.run_id]).strip(), "Untracked fixture resources remain; do not report completed teardown")
        containers = command([*docker, "ps", "-aq", "--filter", "label=" + OWNER + "=" + args.run_id]).split()
        require(len(containers) == 1 and json.loads(command([*docker, "inspect", containers[0]]))[0]["Name"] == "/" + image_container, "Untracked fixture containers remain")
        helm = manifest.get("helm")
        if helm:
            require(shutil.which("helm") is not None and Path(helm["chart"]).is_dir() and Path(helm["values"]).is_file(), "Explicit Helm render prerequisites are missing")
            for mode in ("sharedVolume", "object"):
                for upgrade in (False, True):
                    rendered = command(["helm", "template", "eaccept-" + args.run_id, helm["chart"], "--values", helm["values"], "--set", "storage.mode=" + mode, "--set", "corePack.version=" + published["version"], "--set", "corePack.digest=" + published["digest"], "--set", "corePack.requiredOnUpgrade=true", *(["--is-upgrade"] if upgrade else [])])
                    for workload in yaml.safe_load_all(rendered):
                        if not workload or workload.get("kind") not in ("Deployment", "StatefulSet", "DaemonSet", "Job") or workload.get("metadata", {}).get("annotations", {}).get("helm.sh/hook"):
                            continue
                        pod = workload["spec"]["template"]["spec"]
                        startup = json.dumps(pod.get("initContainers", []) + [container.get("readinessProbe", {}) for container in pod.get("containers", [])])
                        require(not any(value in startup for value in ("/health/content", "/health/bootstrap", "content-ready")), "Helm --wait ordinary readiness depends on a post-install content hook")
                    path = args.work_root / ("helm-" + mode + ("-upgrade" if upgrade else "-fresh") + ".private.yaml")
                    with path.open("x") as target:
                        os.chmod(path, 0o600)
                        target.write(rendered)
        private_json(args.work_root / "result.json", {"compose_profiles": results, "canonical_archive": published, "source_revision": manifest["source_revision"], "helm_render": "PASS" if helm else "NOT_RUN", "helm_live": "EXTERNAL_PREREQUISITE", "full_sow_acceptance": "INCOMPLETE"})
    finally:
        if created:
            info = json.loads(command([*docker, "inspect", image_container]))[0]
            require(info.get("Config", {}).get("Labels", {}).get(OWNER) == args.run_id, "Refusing foreign image-container removal")
            require(not info["State"]["Running"], "Archive container unexpectedly running")
            command([*docker, "rm", image_container])
        if not (args.work_root / "result.json").exists():
            private_json(args.work_root / "result.json", {"compose_profiles": results, "required_profiles": list(BACKENDS), "full_sow_acceptance": "INCOMPLETE", "helm_live": "EXTERNAL_PREREQUISITE"})


def main(argv=None):
    try:
        args = cli(argv)
        manifest = json.loads(args.manifest.read_text())
        print(json.dumps(plan(args, manifest), indent=2))
        if args.execute:
            def interrupted(_signum, _frame):
                raise KeyboardInterrupt
            signal.signal(signal.SIGTERM, interrupted)
            execute(args, manifest)
        return 0
    except (AcceptanceError, OSError, ValueError, KeyError, subprocess.SubprocessError):
        # Source/fixture validation errors are safe; command/HTTP output may carry
        # credentials. Keep execution failures non-successful without echoing it.
        failure = sys.exc_info()[1]
        if isinstance(failure, urllib.error.HTTPError):
            failure.close()
        if isinstance(failure, AcceptanceError):
            print(str(failure), file=sys.stderr)
        else:
            print("Acceptance failed. Check explicit artifact/fixture prerequisites; no reset or fallback was performed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
