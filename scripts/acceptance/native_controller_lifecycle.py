#!/usr/bin/env python3
"""Central-only native positive matrix through real producer, API audit and custody.

No source of LegacyAuditMember JSON or PASS flags is accepted from a caller.
F's actual native_settlement_adapter enrolls original identities before creation.
E owns this daemon; never overlap F's live_cases runner on the same generation.
"""
import argparse
import hashlib
import importlib.util
import json
import os
import re
import signal
from pathlib import Path
import socket
import ssl
import stat
import sys
import time
import urllib.parse
import urllib.request

from fixture_support import FixtureError, read_secret, require, write_private
import native_producer_fixture as producer

MODES = ("blob", "shared", "blob_and_shared")
MODULES = ("main.py", "controller.py", "journal.py", "native.py", "protocol.py", "http_input.py")
SOURCE_PINS = json.loads((Path(__file__).resolve().parent / "native-controller-source-pins.json").read_text())["modules"]


def validate(binding):
    require(binding.get("enabled_for_controlled_acceptance") is True and re.fullmatch(r"[a-z][a-z0-9-]{0,19}", binding.get("run_id", "")), "Explicit controlled binding and bounded run ID required")
    require([case["mode"] for case in binding["cases"]] == list(MODES), "All three fresh native modes are required")
    artifact = binding["producer_adapter"]
    root_file(artifact["binary"])
    require(os.path.isabs(artifact["binary"]) and Path(artifact["binary"]).name == "native_settlement_adapter" and os.access(artifact["binary"], os.X_OK) and file_hash(artifact["binary"]) == artifact["sha256"], "Missing actual F native_settlement_adapter; no fabricated rows fallback")
    release = binding["producer_release"]
    root_file(release["archive_file"])
    require(re.fullmatch(r"[0-9a-f]{64}", release["digest"]) and file_hash(release["archive_file"]) == release["digest"], "Supply the measured actual canonical native producer archive")
    evidence = Path(binding["evidence_file"])
    require(evidence.is_absolute() and not os.path.lexists(evidence), "Evidence must be a new owned private file, never replace an earlier receipt")
    paths = set()
    generations = set()
    original_ids = set()
    service_lanes = set()
    for case in binding["cases"]:
        require(case["project"] == "eaccept-" + binding["run_id"] + "-native-" + case["mode"], "Foreign native project")
        root_file(case["controller_config"])
        config = json.loads(Path(case["controller_config"]).read_text())
        instance = binding["run_id"] + "-" + case["mode"]
        require(case["controller_config"] == "/etc/attune/native-legacy/" + instance + "/config.json" and binding["controller_directory"] == "/opt/attune/native-controller", "Use the installed template's exact module/config locations")
        require(config["service_uid"] == 61124 and config["enabled_for_controlled_acceptance"] is True, "Unit and config must use the same dedicated non-root UID and explicit gate")
        require(config["generation"] not in generations, "Never reuse a generation across native cases")
        generations.add(config["generation"])
        principal = producer.writer_principal(case, config)
        for key in ("native_instance", "worker_instance", "execution_attempt"):
            require(principal[key] not in original_ids, "Never reuse original writer UUIDs across native lanes")
            original_ids.add(principal[key])
        template = producer.validate_context(dict(case, run_id=binding["run_id"]), config, root_file)
        require(case["api_url"] not in service_lanes and template["database_url"] not in service_lanes, "Native modes require separate private API/database lanes")
        service_lanes.update((case["api_url"], template["database_url"]))
        for value in (*config["roots"].values(), config["state_dir"], config["cgroup"]):
            path = Path(value)
            require(path.is_absolute() and str(path) == value and not any(path == prior or path in prior.parents or prior in path.parents for prior in paths), "Native cases need separate non-overlapping absolute roots, journal and cgroup")
            paths.add(path)
        for name in MODULES:
            path = Path(binding["controller_directory"]) / name
            root_file(path)
            require(file_hash(path) == binding["controller_sha256"][name] == SOURCE_PINS[name], "Installed native controller differs from reviewed six-module F source pins")
        for key in ("writer_ca", "writer_certificate", "writer_key", "producer_context_template_file", "maintenance_credential_file", "installer_credential_file"):
            root_file(case[key])


def file_hash(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def root_file(path):
    path = Path(path)
    require(path.is_absolute(), "Use an absolute installed native input")
    value = path.lstat()
    require(stat.S_ISREG(value.st_mode) and value.st_uid == 0 and not value.st_mode & 0o022, "Native controller inputs must be root-owned non-writable regular files")
    for parent in path.parents:
        value = parent.lstat()
        require(stat.S_ISDIR(value.st_mode) and value.st_uid == 0 and not value.st_mode & 0o022, "Native input ancestors must be root-owned non-writable real directories")


def facts(config):
    identity = lambda path: [os.stat(path, follow_symlinks=False).st_dev, os.stat(path, follow_symlinks=False).st_ino]
    return {"boot": Path("/proc/sys/kernel/random/boot_id").read_text().strip(), "pid_namespace": os.readlink("/proc/self/ns/pid"), "roots": {key: identity(value) for key, value in config["roots"].items()}, "journal": identity(Path(config["state_dir"]) / "journal.sqlite3"), "cgroup": identity(config["cgroup"])}


class Lifecycle:
    def __init__(self, binding, driver):
        self.b, self.driver = binding, driver
        self.records = []

    def command(self, argv, **kwargs):
        return self.driver.command(argv, **kwargs)

    def systemd(self, verb, unit):
        require(unit.startswith("attune-native-legacy@") and unit.endswith(".service") and "/" not in unit and ".." not in unit, "Only this installed owned native instance unit may be controlled")
        return self.command(["/usr/bin/systemctl", verb, unit], timeout=180)

    def inspect(self, case, operation, **fields):
        artifact = self.b["producer_adapter"]
        # The real F adapter authenticates prepare-origin before creating the
        # original producer transaction. E never supplies a member/SQL/proof.
        output = self.command([artifact["binary"], "--execute"], data=json.dumps({"protocol": 1, "run_id": self.b["run_id"], "project": case["project"], "mode": case["mode"], "generation": case["generation"], "operation": operation, **fields}), env={"ATTUNE_NATIVE_ACCEPTANCE_CONTEXT": case["producer_context_file"]}, timeout=120)
        require(len(output.encode()) <= 65536, "Native setup adapter exceeded its bounded response contract")
        result = json.loads(output)
        require(result.get("protocol") == 1 and "data" in result, "F setup/observer adapter must return its actual typed repository result; no fallback")
        return result["data"]

    def receipts(self, api, path, token):
        records = []
        cursor = ""
        for _ in range(32):
            page = api.call(path + "/receipts" + cursor, token)["data"]
            require(isinstance(page, list) and len(page) <= 128, "Invalid bounded audit receipt page")
            if not page:
                return records
            require(not records or (page[0]["kind"], page[0]["identity"]) > (records[-1]["kind"], records[-1]["identity"]), "Audit receipt cursor failed to advance")
            records.extend(page)
            cursor = "?" + urllib.parse.urlencode({"after_kind": page[-1]["kind"], "after_identity": page[-1]["identity"]})
        raise FixtureError("Audit receipts exceeded bounded private evidence inventory")

    def tls(self, case):
        endpoint = urllib.parse.urlsplit(case["controller_endpoint"])
        require(endpoint.scheme == "https" and endpoint.hostname == "127.0.0.1" and not endpoint.username and endpoint.path in ("", "/") and not endpoint.query and not endpoint.fragment, "Use the owned local controller HTTPS root")
        tls = ssl.create_default_context(cafile=case["writer_ca"])
        tls.minimum_version = ssl.TLSVersion.TLSv1_3
        tls.load_cert_chain(case["writer_certificate"], case["writer_key"])
        deadline = time.monotonic() + 30
        while True:
            try:
                with socket.create_connection((endpoint.hostname, endpoint.port or 443), timeout=3) as raw, tls.wrap_socket(raw, server_hostname=endpoint.hostname) as secure:
                    require(hashlib.sha256(secure.getpeercert(binary_form=True)).hexdigest() == case["server_der_sha256"], "Controller TLS leaf differs from the actual pinned deployment certificate")
                break
            except OSError:
                require(time.monotonic() < deadline, "Native mTLS transport did not become available; not settlement evidence")
                time.sleep(0.1)
        return tls

    def writer(self, case, member):
        tls = self.tls(case)
        import base64
        payload = b"owned native acceptance\n" * 8192
        body = {"member": member, "mode": case["mode"], "payload_base64": base64.b64encode(payload).decode(), "pause_after_first_chunk": True}
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *args):
                return None
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=tls), NoRedirect())
        request = urllib.request.Request(case["controller_endpoint"].rstrip("/") + "/native-writers/" + member["operation"], json.dumps(body).encode(), {"Content-Type": "application/json"}, method="POST")
        with opener.open(request, timeout=30) as response:
            require(response.status == 202, "Writer admission must actually return durable 202; never infer settlement")
            response.read(16384)

    def barrier(self, config, mode):
        deadline = time.monotonic() + 30
        required = {"blob", "shared"} if mode == "blob_and_shared" else {mode}
        while True:
            for pid in Path(config["cgroup"], "cgroup.procs").read_text().split():
                proc = Path("/proc") / pid
                try:
                    executable = (proc / "exe").stat()
                    pinned = Path(config["writer_binary"]).stat()
                    if proc.stat().st_uid != config["service_uid"] or (executable.st_dev, executable.st_ino) != (pinned.st_dev, pinned.st_ino):
                        continue
                    roots = set()
                    for fd in (proc / "fd").iterdir():
                        try:
                            target = os.readlink(fd)
                            roots.update(name for name, root in config["roots"].items() if target.startswith(root + "/") and not target.endswith(" (deleted)"))
                        except FileNotFoundError:
                            continue
                    if required <= roots:
                        return {"pid": int(pid), "open_roots": sorted(roots)}
                except FileNotFoundError:
                    continue
            require(time.monotonic() < deadline, "Actual pinned native writer never held every original destination open; 202 is not a barrier")
            time.sleep(0.1)

    def case(self, case):
        case = dict(case, run_id=self.b["run_id"])
        config_path = Path(case["controller_config"])
        root_file(config_path)
        config = json.loads(config_path.read_text())
        case["generation"] = config["generation"]
        principal = producer.writer_principal(case, config)
        template = producer.validate_context(case, config, root_file)
        require(config["enabled_for_controlled_acceptance"] is True and config["host_pid_namespace"] == os.readlink("/proc/self/ns/pid"), "Explicit controlled gate and actual host PID namespace are required")
        require(case["systemd_unit"] == "attune-native-legacy@" + self.b["run_id"] + "-" + case["mode"] + ".service", "Never control an existing unrelated native unit")
        unit = case["systemd_unit"]
        require(self.command(["/usr/bin/systemctl", "show", "--property=ActiveState", "--value", unit]).strip() == "inactive", "Native acceptance unit must start inactive, never adopt a running daemon")
        properties = dict(line.split("=", 1) for line in self.command(["/usr/bin/systemctl", "show", unit, "--property=User,Group,NoNewPrivileges,CapabilityBoundingSet,Environment,ExecStart,ControlGroup"]).splitlines())
        require(properties["User"] == "61124" and properties["Group"] == "61124" and properties["NoNewPrivileges"] == "yes" and not properties["CapabilityBoundingSet"], "Installed unit lost its non-root/no-caps/no-new-privileges contract")
        require("ATTUNE_NATIVE_ACCEPTANCE_FLAGS=--enable-controlled-acceptance" in properties["Environment"] and str(config_path) in properties["ExecStart"] and "python3 -I -B" in properties["ExecStart"], "Unit must explicitly opt in and use this exact installed isolated Python/config")
        daemon_cgroup = Path("/sys/fs/cgroup" + properties["ControlGroup"])
        require(Path(config["cgroup"]) != daemon_cgroup and daemon_cgroup not in Path(config["cgroup"]).parents, "Writer leaf must survive daemon restart outside its unit subtree")
        for path in config["roots"].values():
            require(not os.path.lexists(path), "Controller roots must be new inodes; do not adopt an empty/historical root")
        require(not (Path(config["state_dir"]) / "journal.sqlite3").exists(), "Never re-init or adopt a native generation journal")
        for name in MODULES:
            root_file(Path(self.b["controller_directory"]) / name)
        root_file(config["writer_binary"])
        require(file_hash(config["writer_binary"]) == config["writer_sha256"], "Actual native writer artifact hash mismatch")
        api, token, identity = producer.login(case, "maintenance")
        require(identity == template["access"]["maintenance_identity"], "Native setup must use the actual maintenance login identity")
        record = {"mode": case["mode"], "generation": config["generation"], "unit": unit, "completed": False, "daemon_joined": False, "writers_joined": False, "private_context_removed": False, "producer_installation": "ATTEMPTED_OUTCOME_UNKNOWN"}
        self.records.append(record)
        producer_action = producer.install(case, self.b["producer_release"], identity)
        record.update(producer_installation="INSTALLED", producer_action_id=producer_action)
        # Central packaging grants only this dedicated UID access to fresh
        # parents/journal and the separate stable delegated writer cgroup.
        self.command(["/usr/bin/setpriv", "--reuid", str(config["service_uid"]), "--regid", str(config["service_uid"]), "--clear-groups", "--no-new-privs", "--bounding-set=-all", "/usr/bin/python3", "-I", "-B", str(Path(self.b["controller_directory"]) / "main.py"), "init", "--config", str(config_path), "--enable-controlled-acceptance"], timeout=60)
        unit = case["systemd_unit"]
        started = False
        try:
            producer.context(case, template, token, identity)
            record["producer_action_id"] = producer_action
            started = True  # A lost start response still requires owned stop/wait.
            self.systemd("start", unit)
            self.tls(case)  # Bounded mTLS readiness only, not an enrollment/settlement proof.
            before = facts(config)
            prepared = self.inspect(case, "prepare-original-native-producer")
            member = producer.original(prepared, config, principal, case["mode"])
            original = json.loads(json.dumps(member))
            record.update(version_id=member["version"], operation=member["operation"], roots_and_boot=before)
            self.writer(case, member)
            record["native_barrier"] = self.barrier(config, case["mode"])
            raw_opener = api.opener
            class AuditTimeout:
                def open(self, request, timeout):
                    return raw_opener.open(request, timeout=150)
            api.opener = AuditTimeout()
            # Ordinary deletion retains native custody while the request is live.
            api.call(f"/api/v1/artifacts/{prepared['artifact_id']}/versions/{prepared['artifact_version']}", token, method="DELETE")
            pinned = self.inspect(case, "observe-original-native-custody", version_id=member["version"])
            producer.observation(pinned, original, config["scope"])
            audit = api.call("/api/v1/retention-config/file-audits", token, {"versions": [member["version"]]}, "POST")["data"]
            require(audit["plan"]["members"] == [original], "The actual frozen audit must equal the full original journal member")
            require(audit["plan"]["scope"] == config["scope"], "Actual API backend scope differs from this exact local provider topology")
            path = "/api/v1/retention-config/file-audits/" + audit["id"]
            result = api.call(path + "/step", token, {}, "POST")["data"]
            require(result["state"] in ("cataloguing", "cleaning"), "Real stop/fence/catalogue must succeed, not manual quarantine or caller flags")
            writers_before = [r for r in self.receipts(api, path, token) if r["kind"] == "writers"]
            require(writers_before, "Actual repository has no persisted native writer fence receipt")
            events = Path(config["cgroup"], "cgroup.events").read_text()
            require("frozen 1" in events and "populated 0" in events, "Actual kernel freezer/drain not observed")
            invocation = self.command(["/usr/bin/systemctl", "show", "--property=InvocationID", "--value", unit]).strip()
            self.systemd("restart", unit)  # Never init/recreate roots/cgroup/journal.
            self.tls(case)  # Wait for the new daemon's authenticated listener before API resume.
            require(facts(config) == before, "Daemon restart replaced boot, mount/root/journal/cgroup identity; quarantine required")
            require(self.command(["/usr/bin/systemctl", "show", "--property=InvocationID", "--value", unit]).strip() != invocation, "Daemon was not actually restarted")
            events = Path(config["cgroup"], "cgroup.events").read_text()
            require("frozen 1" in events and "populated 0" in events, "Restart lost the real frozen kernel barrier")
            deadline = time.monotonic() + 1800
            for _ in range(128):
                require(time.monotonic() < deadline, "Native API audit exceeded bounded case deadline")
                state = api.call(path + "/step", token, {}, "POST")["data"]["state"]
                require(state != "manual_required", "Native resume failed; leave quarantine, never attest cloud/history")
                if state == "completed":
                    break
            else:
                raise FixtureError("Bounded actual audit never completed")
            released = self.inspect(case, "observe-original-native-custody", version_id=member["version"])
            require(released["pins"] == 0 and released["unresolved_operations"] == 0 and released["evidence_owner_count"] == 0, "Actual repository physical custody/pins were not released")
            producer.observation(released, original, config["scope"], released=True)
            require(released["audit"]["id"] == audit["id"], "Repository inspection returned a different audit")
            records = self.receipts(api, path, token)
            require({"writers", "catalogue", "absence", "settled"} <= {r["kind"] for r in records}, "Missing actual fence/catalogue/exact-delete/settlement evidence")
            require([r for r in records if r["kind"] == "writers"] == writers_before, "Persisted fence changed across same-boot restart")
            record.update(completed=True, audit_id=audit["id"], repository_before=pinned, repository_after=released, receipt_kinds=sorted({r["kind"] for r in records}))
        finally:
            if started:
                self.systemd("stop", unit)
                require(self.command(["/usr/bin/systemctl", "show", "--property=MainPID", "--value", unit]).strip() == "0", "Native daemon has not joined; retain generation")
                record["daemon_joined"] = True
                if record["completed"]:
                    require("populated 0" in Path(config["cgroup"], "cgroup.events").read_text(), "Native writers have not joined; retain credentials and custody")
                    record["writers_joined"] = True
                    Path(case["producer_context_file"]).unlink()
                    Path(template["maintenance_access_token_file"]).unlink()
                    record["private_context_removed"] = True
                # Do not remove data roots/journal or the kernel freezer barrier.
                # Failure retains all original native custody for operator recovery.


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binding", type=Path, required=True)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--enable-controlled-acceptance", action="store_true")
    args = parser.parse_args()
    if not args.execute:
        print(json.dumps({"mode": "DRY_RUN", "binding": str(args.binding), "cases": MODES, "required": "actual F original producer/repository adapter; dedicated host topology and pinned artifacts", "mutations": False, "full_sow_acceptance": False}))
        return
    require(args.execute and args.enable_controlled_acceptance, "Native lifecycle requires both explicit controlled execution flags")
    binding = json.loads(read_secret(args.binding))
    require(os.geteuid() == 0, "Only the central root-owned fixture may control installed native units")
    validate(binding)
    spec = importlib.util.spec_from_file_location("native_owned_commands", Path(__file__).resolve().parent.parent / "run-bootstrap-lifecycle-acceptance.py")
    driver = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(driver)
    owner = Lifecycle(binding, driver)
    def interrupted(_number, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try:
        for case in binding["cases"]:
            owner.case(case)
    finally:
        write_private(binding["evidence_file"], {"cases": owner.records, "positive_matrix_completed": len(owner.records) == len(MODES) and all(row["completed"] and row["daemon_joined"] and row["writers_joined"] and row["private_context_removed"] for row in owner.records), "full_sow_acceptance": False, "roots_journal_and_kernel_fences": "retained; no broad deletion or automatic adoption"})


if __name__ == "__main__":
    try:
        main()
    except Exception:
        print("Native acceptance failed; retain original generation/journal/cgroup and quarantine. Inspect owned private evidence; no payload/credential dump.", file=sys.stderr)
        sys.exit(1)
