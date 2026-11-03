#!/usr/bin/env python3
"""CENTRAL ONLY. Real owned kernel/provider/app cases. Never run by source tests."""
import argparse
import base64
import copy
import hashlib
import http.client
import importlib
import io
import json
import os
from pathlib import Path
import re
import resource
import ssl
import stat
import subprocess
import sys
import tempfile
import time
import unittest
import uuid

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "acceptance"))

from journal import Journal
from main import configuration, protected_file
from native import LIBC, directory, fs_type, root_identity
from protocol import MAX_BODY, Quarantine, canonical, decode, digest, member, require
from storage_probe import API, private

CASES = ("paused_combined", "same_boot_restart", "lost_delete_reply", "untracked_pid",
         "mount_replacement", "external_hardlink", "cert_role_attempts")
NEGATIVES = frozenset(CASES[3:])


class Unsupported(Exception):
    pass


def supported(condition, code):
    if not condition:
        raise Unsupported(code)


def wait_for(predicate, code, seconds=15):
    deadline = time.monotonic() + seconds
    while True:
        value = predicate()
        if value:
            return value
        require(time.monotonic() < deadline, code)
        time.sleep(min(0.025, max(0, deadline - time.monotonic())))


def start_time(pid):
    try:
        # comm may contain spaces or parentheses; fields after its final ')'
        # start with state. starttime is stat field 22.
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
    except FileNotFoundError:
        return None


def regular(path, owner=0, executable=False):
    path = Path(path)
    info = path.lstat()
    require(path.is_absolute() and stat.S_ISREG(info.st_mode) and info.st_uid == owner
            and info.st_mode & 0o022 == 0, "UnownedLiveArtifact")
    if executable:
        require(info.st_mode & 0o111 and not info.st_mode & (stat.S_ISUID | stat.S_ISGID),
                "UntrustedLiveExecutable")
    return path


def within(path, root):
    path = Path(path)
    require(path.is_absolute() and root in path.parents and path.resolve() == path,
            "PathOutsideOwnedLane")
    return path


def owner_binding(binding):
    require(binding["schema_version"] == 1
            and re.fullmatch(r"[a-z][a-z0-9-]{0,19}", binding["run_id"])
            and re.fullmatch(r"[a-z0-9_-]{1,128}", binding["project"])
            and binding["project"].startswith("eaccept-" + binding["run_id"] + "-"),
            "InvalidOwnedRunBinding")


def run_journal_cases(owned_root):
    """Actually execute all nine SQLite tests, only after the live CLI gate."""
    for name in ("runtime_tests.py", "enrollment_runtime_tests.py", "test_controller_source.py"):
        regular(HERE / name)
    old = tempfile.tempdir
    tempfile.tempdir = str(owned_root)
    try:
        module = importlib.import_module("runtime_tests")
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(module.DurableJournalTests)
        require(suite.countTestCases() == 9, "JournalCoverageChanged")
        # Failures remain private; never dump fixture state, paths or secrets.
        result = unittest.TextTestRunner(stream=io.StringIO()).run(suite)
        require(result.wasSuccessful() and result.testsRun == 9 and not result.skipped
                and not result.expectedFailures, "JournalRuntimeFailure")
        enrollment = importlib.import_module("enrollment_runtime_tests")
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(enrollment.OriginalEnrollmentTests)
        require(suite.countTestCases() == 4, "EnrollmentCoverageChanged")
        enrolled = unittest.TextTestRunner(stream=io.StringIO()).run(suite)
        require(enrolled.wasSuccessful() and enrolled.testsRun == 4 and not enrolled.skipped
                and not enrolled.expectedFailures, "EnrollmentRuntimeFailure")
        return {"tests": result.testsRun, "enrollment_tests": enrolled.testsRun,
                "status": "PASS", "native_acceptance": False}
    finally:
        tempfile.tempdir = old


class Lane:
    """E provisions each fresh lane. F owns these exact executable case steps."""
    def __init__(self, binding, name, execute_owned_live=False, allow_mount_fault=False):
        require(execute_owned_live is True, "LiveExecutionFlagRequired")
        supported(os.geteuid() == 0, "DedicatedHostRunnerRequired")
        owner_binding(binding)
        require(name in CASES and binding["schema_version"] == 1, "InvalidLiveCase")
        self.name, self.binding = name, binding
        self.b = binding["lanes"][name]
        self.root = Path(binding["owned_root"])
        mode = self.root.lstat()
        require(self.root.is_absolute() and self.root.resolve() == self.root
                and mode.st_uid == 0 and stat.S_ISDIR(mode.st_mode)
                and mode.st_mode & 0o022 == 0, "UnownedLiveRoot")
        marker = decode(private(self.root / ".attune-native-owner.json").encode())
        require(marker == {"run_id": binding["run_id"], "project": binding["project"]},
                "LiveOwnerMarkerMismatch")
        self.config_path = regular(within(self.b["config"], self.root))
        self.config = configuration(self.config_path)
        require(self.config["enabled_for_controlled_acceptance"], "ControlledAcceptanceGateDisabled")
        require(self.config["bind"] == "127.0.0.1" and type(self.config["port"]) is int
                and 1024 <= self.config["port"] <= 65535, "OwnedLoopbackRequired")
        self.boot = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
        self.pid_namespace = os.readlink("/proc/self/ns/pid")
        supported(self.pid_namespace == os.readlink("/proc/1/ns/pid")
                  == self.config["host_pid_namespace"], "HostPidNamespaceUnavailable")
        self.uid, self.gid = self.config["service_uid"], self.b["service_gid"]
        require(type(self.gid) is int and self.gid > 0, "DedicatedServiceGroupRequired")
        for key in ("tls_key", "signing_key"):
            info = Path(self.config[key]).stat()
            require(info.st_gid == self.gid and info.st_mode & 0o007 == 0,
                    "NativePrivateKeyGroupMismatch")
        self.cgroup_path = Path(self.config["cgroup"])
        require(self.cgroup_path.is_absolute()
                and Path("/sys/fs/cgroup") in self.cgroup_path.parents
                and self.cgroup_path.name == "attune-native-" + self.config["generation"]
                and self.cgroup_path.resolve() == self.cgroup_path, "UnownedWriterCgroup")
        self.cgroup = directory(str(self.cgroup_path))
        supported(fs_type(self.cgroup) == 0x63677270, "CgroupV2Unavailable")
        for key in ("cgroup.freeze", "cgroup.kill", "cgroup.procs"):
            info = (self.cgroup_path / key).stat()
            supported((info.st_uid == self.uid and info.st_mode & stat.S_IWUSR)
                      or (info.st_gid == self.gid and info.st_mode & stat.S_IWGRP),
                      "CgroupDelegationUnavailable")
        supported(not self.cg("cgroup.procs").strip(), "WriterLeafNotFresh")
        self.roots = {}
        for key, value in self.config["roots"].items():
            path = within(value, self.root)
            info = path.stat()
            require(info.st_uid == self.uid and stat.S_IMODE(info.st_mode) == 0o700,
                    "RootUidOrModeMismatch")
            fd = directory(str(path))
            self.roots[key] = fd
            supported(fs_type(fd) in (0xEF53, 0x58465342), "LocalNativeProviderUnavailable")
        self.state = within(self.config["state_dir"], self.root)
        state_info = self.state.stat()
        require(state_info.st_uid == self.uid and stat.S_IMODE(state_info.st_mode) == 0o700,
                "PrivateJournalOwnershipMismatch")
        self.initial = Journal.observe(self.state / "journal.sqlite3")
        require(not self.initial["fenced"] and not self.initial["requests"], "GenerationNotFresh")
        actual = self.initial["binding"]
        require(actual["generation"] == self.config["generation"] and actual["scope"] == self.config["scope"]
                and actual["boot"] == self.boot and actual["host_pid_namespace"] == self.pid_namespace
                and actual["service_uid"] == self.uid and actual["service_gid"] == self.gid
                and actual["cgroup"] == root_identity(self.cgroup)
                and actual["roots"] == {key: root_identity(fd) for key, fd in self.roots.items()},
                "NativeOriginBindingMismatch")
        writer = regular(self.config["writer_binary"], executable=True)
        with writer.open("rb") as stream:
            require(hashlib.file_digest(stream, "sha256").hexdigest()
                    == self.config["writer_sha256"] == actual["writer_sha256"], "WriterBinaryChanged")
        require(all(not os.listdir(fd) for fd in self.roots.values()), "UntrackedHistoricalChildren")
        require("write" not in self.b and "member" not in self.b, "CallerMemberNotAccepted")
        for name in ("main.py", "controller.py", "native.py", "journal.py", "protocol.py", "http_input.py"):
            regular(HERE / name)
        self.api = API(self.b["api"])
        self.token, expected_operator = self.api.password_login(self.b["maintenance_credential_file"],
                                                               binding["run_id"], binding["project"])
        operator = self.api.call("/auth/me", self.token)["data"]
        require(all(operator[key] == expected_operator[key] for key in ("id", "login")),
                "ActualMaintenanceIdentityMismatch")
        cap = self.app("/file-audits/capabilities")
        require(cap["scope"] == self.config["scope"]
                and cap["management_transport"] == "configured_unverified", "ActualApiScopeMismatch")
        artifact = self.b["producer_adapter"]
        self.producer_adapter = regular(artifact["binary"], executable=True)
        with self.producer_adapter.open("rb") as stream:
            require(hashlib.file_digest(stream, "sha256").hexdigest() == artifact["sha256"],
                    "ProducerAdapterBinaryChanged")
        private(artifact["context_file"])
        self.daemon = None
        self.old_writers = {}
        self.audit = None
        self.plan = None
        self.allow_mount_fault = allow_mount_fault
        self.observed = {"boot": self.boot, "pid_namespace": self.pid_namespace,
                         "service_uid": self.uid, "generation": self.config["generation"]}

    def cg(self, key):
        require(key in ("cgroup.events", "cgroup.procs", "cgroup.freeze"), "InvalidKernelObservation")
        fd = os.open(key, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=self.cgroup)
        try:
            value = os.read(fd, 65537)
            require(len(value) <= 65536, "KernelObservationBound")
            return value.decode("ascii")
        finally:
            os.close(fd)

    def app(self, suffix, body=None, method="GET", expected=(200,)):
        return self.api.call("/api/v1/retention-config" + suffix, self.token, body, method, expected)["data"]

    def adapter(self, operation, **fields):
        request = {"protocol": 1, "run_id": self.binding["run_id"], "project": self.binding["project"],
                   "generation": self.config["generation"], "mode": "blob_and_shared", "operation": operation, **fields}
        data = canonical(request)
        require(len(data) <= 16384, "InspectionRequestBound")
        def child_limits():
            resource.setrlimit(resource.RLIMIT_FSIZE, (2 * MAX_BODY, 2 * MAX_BODY))
        with tempfile.TemporaryFile(dir=self.root) as output:
            result = subprocess.run([str(self.producer_adapter), "--execute"], input=data, stdout=output,
                                    stderr=subprocess.DEVNULL, timeout=115, check=False,
                                    env={"ATTUNE_NATIVE_ACCEPTANCE_CONTEXT": self.b["producer_adapter"]["context_file"]},
                                    preexec_fn=child_limits)
            require(result.returncode == 0, "TypedRepositoryInspectionFailed")
            output.seek(0)
            raw = output.read(2 * MAX_BODY + 1)
        require(len(raw) <= 2 * MAX_BODY, "InspectionResponseBound")
        value = json.loads(raw)
        require(value["protocol"] == 1, "InspectorProtocolMismatch")
        return value["data"]

    def prepare(self):
        prepared = self.adapter("prepare-original-native-producer")
        self.original = member(prepared["member"])
        require(self.original["object_key"] and self.original["staging_path"], "MissingOriginalObligation")
        self.member_version = self.original["version"]
        self.legacy_record = {"artifact": prepared["artifact_id"], "version": prepared["artifact_version"]}
        payload = b"owned native acceptance\n" * 8192
        self.expected_size = len(payload)
        self.write = {"mode": "blob_and_shared", "pause_after_first_chunk": True,
                      "member": self.original, "payload_base64": base64.b64encode(payload).decode()}
        self.retained()

    def evidence(self):
        value = self.adapter("observe-original-native-custody", version_id=self.member_version)
        require(value["member"] == self.original, "ObservedOriginalMemberChanged")
        return {"owner": value["evidence_owner_count"] > 0, "pins": value["pins"],
                "operations": value["unresolved_operations"], "audit": value["audit"]}

    def retained(self):
        value = self.evidence()
        require(value["owner"] and value["pins"] > 0 and value["operations"] > 0,
                "PrematureSourcePinOrOperationRelease")
        return value

    def tls(self, role):
        record = self.b[role + "_tls"]
        for path in (record["certificate"], record["key"], self.b["server_ca"]):
            regular(path)
        protected_file(record["key"], private_material=True)
        context = ssl.create_default_context(cafile=self.b["server_ca"])
        context.minimum_version = ssl.TLSVersion.TLSv1_3
        context.load_cert_chain(record["certificate"], record["key"])
        return context

    def wire(self, role, path, body=None, method="POST", expected=(200,), drop_reply=False):
        require(path.startswith("/") and not path.startswith("//") and "?" not in path,
                "InvalidNativeTestPath")
        # tls() requires verified TLS 1.3 and a client certificate; the peer is pinned below.
        connection = http.client.HTTPSConnection("127.0.0.1", self.config["port"],  # nosemgrep: python.lang.security.audit.httpsconnection-detected.httpsconnection-detected
                                                  context=self.tls(role), timeout=8)
        try:
            connection.connect()
            actual = hashlib.sha256(connection.sock.getpeercert(binary_form=True)).digest()
            leaf = re.search(rb"-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----",
                             protected_file(self.config["tls_certificate"]), re.S)
            require(leaf is not None, "NativeServerCertificateMissing")
            expected_cert = ssl.PEM_cert_to_DER_cert(leaf[0].decode())
            require(actual == hashlib.sha256(expected_cert).digest(), "NativeServerCertificateMismatch")
            connection.request(method, path, None if body is None else canonical(body),
                               {"Content-Type": "application/json"})
            if drop_reply:
                # Wait for real response headers so the operation definitely
                # executed, then discard its proof body without reading it.
                # The app repository has received NO absence receipt.
                response = connection.getresponse()
                require(response.status == 200, "LostReplyOperationFailed")
                response.close()
                return None
            response = connection.getresponse()
            raw = response.read(MAX_BODY + 1)
            require(len(raw) <= MAX_BODY and response.status in expected, "NativeHttpExpectationFailed")
            return response.status, raw
        finally:
            connection.close()

    def start(self):
        require(self.daemon is None, "DaemonAlreadyOwned")
        self.daemon = subprocess.Popen(["/usr/bin/python3", "-I", str(HERE / "main.py"), "serve",
                                        "--config", str(self.config_path), "--enable-controlled-acceptance"],
                                       stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                       stderr=subprocess.DEVNULL, env={}, user=self.uid, group=self.gid,
                                       extra_groups=[self.gid], umask=0o077)
        def ready():
            require(self.daemon.poll() is None, "NativeDaemonStartupFailed")
            try:
                return self.wire("manager", "/", method="GET", expected=(501,))
            except (OSError, ssl.SSLError, http.client.HTTPException):
                return False
        wait_for(ready, "NativeDaemonReadinessDeadline")
        status = Path(f"/proc/{self.daemon.pid}/status").read_text().splitlines()
        ids = next(line.split()[1:] for line in status if line.startswith("Uid:"))
        require(ids == [str(self.uid)] * 4 and str(self.daemon.pid) not in self.cg("cgroup.procs").split()
                and os.readlink(f"/proc/{self.daemon.pid}/ns/pid") == self.pid_namespace,
                "DaemonUidNamespaceOrLeafMismatch")
        self.observed.setdefault("daemon_pids", []).append(self.daemon.pid)

    def stop(self):
        if self.daemon is not None:
            if self.daemon.poll() is None:
                self.daemon.kill()
            self.daemon.wait(timeout=10)
            self.daemon = None
            wait_for(lambda: not self.cg("cgroup.procs").strip(), "OwnedWriterExitDeadline")

    def paused(self):
        operation = self.original["operation"]
        status, _ = self.wire("writer", "/native-writers/" + operation, self.write, expected=(202,))
        require(status == 202, "NativeWriteNotAccepted")
        def pending():
            pids = self.cg("cgroup.procs").split()
            if not pids or "populated 1" not in self.cg("cgroup.events").splitlines():
                return False
            shared = Path(self.config["roots"]["shared"]) / self.original["staging_path"]
            stages = Path(self.config["roots"]["blob"]) / ".attune-upload"
            if not shared.is_file() or shared.stat().st_size != self.expected_size or not stages.is_dir():
                return False
            candidates = list(stages.iterdir())
            if not candidates:
                return False
            shared_identity = (shared.stat().st_dev, shared.stat().st_ino)
            for value in pids:
                pid = int(value)
                try:
                    if os.readlink(f"/proc/{pid}/exe") != str(Path(self.config["writer_binary"]).resolve()):
                        return False
                    handles = []
                    for entry in Path(f"/proc/{pid}/fd").iterdir():
                        try:
                            info = entry.stat()
                            handles.append((info.st_dev, info.st_ino))
                        except FileNotFoundError:
                            pass
                    if shared_identity in handles and any((entry.stat().st_dev, entry.stat().st_ino)
                                                          in handles for entry in candidates):
                        self.old_writers[pid] = start_time(pid)
                        return True
                except FileNotFoundError:
                    pass
            return False
        wait_for(pending, "RealMultipartAndSharedDescriptorNotObserved")
        require(Journal.observe(self.state / "journal.sqlite3")["requests"] == {operation: "accepted"},
                "OriginalNativeAdmissionMissing")
        self.observed["actual_paused_multipart_and_shared_descriptor"] = True

    def close_metadata(self):
        original = self.legacy_record
        self.api.call(f"/api/v1/artifacts/{original['artifact']}/versions/{original['version']}",
                      self.token, method="DELETE")
        self.retained()
        created = self.app("/file-audits", {"versions": [self.member_version]}, "POST", (201,))
        self.audit = str(created["id"])
        status = self.app("/file-audits/" + self.audit)
        self.plan = status["plan"]
        require(self.plan["scope"] == self.config["scope"] and self.plan["members"] == [self.original],
                "ActualRepositoryPlanDiffersFromOriginal")

    def manager(self, operation, arguments, expected_error=None, drop_reply=False):
        request = {"nonce": str(uuid.uuid4()), "plan": self.plan,
                   "operation": operation, "arguments": arguments}
        result = self.wire("manager", f"/legacy-file-audits/{self.audit}/{operation}", request,
                           expected=(409,) if expected_error else (200,), drop_reply=drop_reply)
        if drop_reply:
            return None
        _, raw = result
        if expected_error:
            require(decode(raw)["diagnostic"] == expected_error, "WrongNativeFailureBranch")
            return None
        parts = raw.split(b".")
        require(len(parts) == 3, "NativeAttestationMissing")
        signature = base64.urlsafe_b64decode(parts[2] + b"=" * (-len(parts[2]) % 4))
        regular(self.b["verification_key"])
        with tempfile.TemporaryFile(dir=self.root) as file:
            file.write(signature)
            file.flush()
            result = subprocess.run(["/usr/bin/openssl", "dgst", "-sha256", "-verify", self.b["verification_key"],
                                     "-signature", f"/proc/self/fd/{file.fileno()}", "-sigopt", "rsa_padding_mode:pkcs1"],
                                    input=b".".join(parts[:2]), pass_fds=(file.fileno(),),
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env={}, timeout=3)
        require(result.returncode == 0, "NativeAttestationSignatureFailed")
        header = decode(base64.urlsafe_b64decode(parts[0] + b"=" * (-len(parts[0]) % 4)))
        claims = decode(base64.urlsafe_b64decode(parts[1] + b"=" * (-len(parts[1]) % 4)))
        now = int(time.time())
        require(header == {"alg": "RS256", "typ": "JWT"}
                and claims["nonce"] == request["nonce"] and claims["request_digest"] == digest(request)
                and claims["audit"] == self.audit and claims["operation"] == operation
                and claims["scope"] == self.config["scope"] and claims["iss"] == self.config["issuer"]
                and claims["aud"] == self.config["audience"] and now - 60 <= claims["iat"] <= now
                and now < claims["exp"] <= claims["iat"] + 60, "NativeAttestationBindingFailed")
        return claims["data"]

    def kernel_settled(self):
        events = self.cg("cgroup.events").splitlines()
        require("frozen 1" in events and "populated 0" in events and self.cg("cgroup.freeze").strip() == "1"
                and not self.cg("cgroup.procs").strip(), "ActualKernelFenceNotObserved")
        require(all(start_time(pid) != when for pid, when in self.old_writers.items()),
                "NativeWriterNotReaped")
        require(Journal.observe(self.state / "journal.sqlite3")["requests"]
                == {self.original["operation"]: "settled"}, "ProviderJournalNotSettled")
        for fd in self.roots.values():
            require(LIBC.syncfs(fd) == 0, "ActualProviderSyncfsFailed")
        self.observed["kernel"] = {"frozen": 1, "populated": 0, "reaped": True, "syncfs_roots": 2}

    def first_step(self):
        self.app("/file-audits/" + self.audit + "/step", {}, "POST")
        state = self.app("/file-audits/" + self.audit)
        require(state["state"] == "cleaning" and state["catalogued"] >= 2, "RealCombinedCatalogueUnavailable")
        self.kernel_settled()
        self.retained()

    def finish(self):
        def done():
            self.app("/file-audits/" + self.audit + "/step", {}, "POST")
            state = self.app("/file-audits/" + self.audit)
            require(state["state"] != "manual_required", "UnexpectedNativeQuarantine")
            return state if state["state"] == "completed" else False
        state = wait_for(done, "AppAuditCompletionDeadline", seconds=90)
        self.kernel_settled()
        value = self.evidence()
        require(not value["owner"] and value["pins"] == value["operations"] == 0,
                "AppRepositoryDidNotReleaseExactSourcePins")
        visited = 0
        def no_files(fd, depth=0):
            nonlocal visited
            require(depth <= 16, "LiveInventoryDepthBound")
            with os.scandir(fd) as entries:
                for entry in entries:
                    visited += 1
                    require(visited <= 2048, "LiveInventoryBound")
                    if not entry.is_dir(follow_symlinks=False):
                        return False
                    child = os.open(entry.name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                    try:
                        if not no_files(child, depth + 1):
                            return False
                    finally:
                        os.close(child)
            return True
        require(all(no_files(fd) for fd in self.roots.values()), "OwnedPhysicalChildrenRemain")
        self.observed.update(audit=self.audit, source_pins_released=True,
                             catalogued=state["catalogued"], absent=state["confirmed_absent"])

    def quarantine(self, expected_native_error=None):
        if expected_native_error:
            self.manager("stop-and-settle", {}, expected_error=expected_native_error)
            self.app("/file-audits/" + self.audit + "/step", {}, "POST")
        else:
            self.app("/file-audits/" + self.audit + "/quarantine", {}, "POST")
        require(self.app("/file-audits/" + self.audit)["state"] == "manual_required", "ExpectedQuarantineMissing")
        self.retained()
        self.observed.update(audit=self.audit, source_pins_retained=True)

    def close(self):
        self.stop()
        for fd in self.roots.values():
            os.close(fd)
        self.roots.clear()
        if self.cgroup is not None:
            os.close(self.cgroup)
            self.cgroup = None


def case_paused_combined(lane):
    lane.paused()
    lane.close_metadata()
    lane.first_step()
    lane.finish()


def case_same_boot_restart(lane):
    lane.paused()
    lane.stop()  # Actual daemon SIGKILL + wait. Never treated as settlement.
    lane.retained()
    require(Path("/proc/sys/kernel/random/boot_id").read_text().strip() == lane.boot,
            "HostBootChangedDuringRecovery")
    lane.start()
    lane.wire("writer", "/native-writers/" + lane.original["operation"], lane.write, expected=(202,))
    require(not lane.cg("cgroup.procs").strip(), "AcceptedOperationReexecutedAfterCrash")
    lane.close_metadata()
    lane.first_step()
    lane.finish()
    lane.observed["same_boot_daemon_recovery"] = True


def case_lost_delete_reply(lane):
    lane.paused()
    lane.close_metadata()
    lane.first_step()
    fence = lane.manager("stop-and-settle", {})
    page = lane.manager("catalogue", {"fence": fence, "cursor": None, "limit": 128, "max_bytes": 524288})
    require(page["complete"] and len(page["items"]) >= 2, "LiveCatalogueIncomplete")
    item = page["items"][0]
    proof = item["ownership_proof"]
    target = Path(lane.config["roots"][proof["root"]]) / proof["physical_key"]
    lane.manager("delete-exact", {"fence": fence, "item": item}, drop_reply=True)
    wait_for(lambda: not target.exists(), "DroppedReplyDeleteDidNotExecute")
    lane.retained()  # Physical absence alone has not retired the app operation.
    lane.stop()
    lane.start()
    absence = lane.manager("delete-exact", {"fence": fence, "item": item})
    require(absence["item_digest"] == digest(item), "ExactIntentRecoveryMismatch")
    lane.finish()
    lane.observed["lost_reply_exact_intent_recovered"] = True


def case_untracked_pid(lane):
    lane.paused()
    lane.close_metadata()
    process = subprocess.Popen(["/usr/bin/python3", "-I", "-c", "import signal;signal.pause()"],
                               user=lane.uid, group=lane.gid, extra_groups=[lane.gid], env={},
                               stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        require(process.poll() is None, "UntrackedProcessFixtureUnavailable")
        lane.quarantine("UncontrolledWriterProcess")
    finally:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=5)


def case_external_hardlink(lane):
    lane.paused()
    lane.close_metadata()
    target = Path(lane.config["roots"]["shared"]) / lane.original["staging_path"]
    alias = within(Path(lane.config["roots"]["shared"]).parent / "outside-owned-roots.link", lane.root)
    require(not alias.exists(), "HardlinkFaultIdentityReused")
    os.link(target, alias, follow_symlinks=False)
    require(alias.stat().st_ino == target.stat().st_ino and target.stat().st_nlink == 2,
            "ActualOutsideHardlinkNotObserved")
    lane.quarantine("UnownedPhysicalAlias")
    # Retain this exact alias and all evidence for E's owned teardown.


def case_mount_replacement(lane):
    supported(lane.allow_mount_fault, "OwnedMountFaultFlagRequired")
    supported(os.readlink("/proc/self/ns/mnt") != os.readlink("/proc/1/ns/mnt"),
              "DedicatedPrivateMountNamespaceRequired")
    mount_id = root_identity(lane.roots["blob"])["mount"]
    info = next((line.split() for line in Path("/proc/self/mountinfo").read_text().splitlines()
                 if line.split()[0] == mount_id), None)
    supported(info is not None and not any(field.startswith(("shared:", "master:"))
                                           for field in info[6:info.index("-")]),
              "PrivateMountPropagationRequired")
    lane.paused()
    lane.close_metadata()
    target = Path(lane.config["roots"]["blob"])
    replacement = lane.state / "replacement-mount"
    require(not replacement.exists(), "MountFaultIdentityReused")
    replacement.mkdir(mode=0o700)
    os.chown(replacement, lane.uid, lane.gid)
    mounted = False
    try:
        result = subprocess.run(["/bin/mount", "--bind", str(replacement), str(target)], env={},
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)
        supported(result.returncode == 0, "OwnedBindMountUnavailable")
        mounted = True
        current = directory(str(target))
        try:
            require(root_identity(current) != root_identity(lane.roots["blob"]), "MountReplacementNotObserved")
        finally:
            os.close(current)
        lane.quarantine("NativeRootReplaced")
    finally:
        if mounted:
            result = subprocess.run(["/bin/umount", str(target)], env={}, stdout=subprocess.DEVNULL,
                                    stderr=subprocess.DEVNULL, timeout=5)
            require(result.returncode == 0, "OwnedMountFaultCleanupFailed")


def case_cert_role_attempts(lane):
    lane.paused()
    lane.close_metadata()
    path = "/native-writers/" + lane.original["operation"]
    for role, code in (("manager", "WriterPrincipalRequired"), ("unknown", "UnregisteredDeploymentPrincipal")):
        _, raw = lane.wire(role, path, lane.write, expected=(409,))
        require(decode(raw)["diagnostic"] == code, "CertificateRoleNegativeWrongBranch")
    bad = copy.deepcopy(lane.write)
    bad["member"]["execution_attempt"] = str(uuid.uuid4())
    _, raw = lane.wire("writer", path, bad, expected=(409,))
    require(decode(raw)["diagnostic"] == "WriterIdentityMismatch", "AttemptNegativeWrongBranch")
    _, raw = lane.wire("writer", path, dict(lane.write, root="/unowned"), expected=(409,))
    require(decode(raw)["diagnostic"] == "InvalidWrite", "ArbitraryRootNegativeWrongBranch")
    request = {"nonce": str(uuid.uuid4()), "plan": lane.plan, "operation": "stop-and-settle", "arguments": {}}
    _, raw = lane.wire("writer", f"/legacy-file-audits/{lane.audit}/stop-and-settle", request, expected=(409,))
    require(decode(raw)["diagnostic"] == "ManagerPrincipalRequired", "WriterManagementNegativeWrongBranch")
    require(Journal.observe(lane.state / "journal.sqlite3")["requests"] == {lane.original["operation"]: "accepted"},
            "NegativeRequestChangedNativeAdmission")
    lane.quarantine()


CASE_HANDLERS = {
    "paused_combined": case_paused_combined,
    "same_boot_restart": case_same_boot_restart,
    "lost_delete_reply": case_lost_delete_reply,
    "untracked_pid": case_untracked_pid,
    "mount_replacement": case_mount_replacement,
    "external_hardlink": case_external_hardlink,
    "cert_role_attempts": case_cert_role_attempts,
}


def run_case(binding, name, execute_owned_live=False, allow_mount_fault=False):
    require(name in CASE_HANDLERS, "InvalidLiveCase")
    lane = Lane.__new__(Lane)
    lane.daemon, lane.cgroup, lane.roots = None, None, {}
    try:
        Lane.__init__(lane, binding, name, execute_owned_live, allow_mount_fault)
        lane.start()
        lane.prepare()
        CASE_HANDLERS[name](lane)
        lane.stop()
        if name in NEGATIVES:
            lane.retained()  # Killing a daemon must not retire native obligations.
        return {"case": name, "status": "PASS", "observations": lane.observed}
    finally:
        lane.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__, epilog=
        "Example: python3 -I live_cases.py --binding /owned/native-live.private.json "
        "--execute-owned-live --allow-owned-mount-fault")
    parser.add_argument("--binding", required=True)
    parser.add_argument("--execute-owned-live", action="store_true")
    parser.add_argument("--allow-owned-mount-fault", action="store_true")
    parser.add_argument("--case", choices=CASES, help="E may run one fresh lifecycle at a time; not full acceptance")
    args = parser.parse_args()
    require(args.execute_owned_live, "LiveExecutionFlagRequired")
    binding = decode(private(args.binding).encode())
    owner_binding(binding)
    root = Path(binding["owned_root"])
    require(root.is_absolute() and root.resolve() == root and root.stat().st_uid == os.geteuid() == 0,
            "UnownedLiveRoot")
    marker = decode(private(root / ".attune-native-owner.json").encode())
    require(marker == {"run_id": binding["run_id"], "project": binding["project"]}, "LiveOwnerMarkerMismatch")
    journal = run_journal_cases(root)
    names = (args.case,) if args.case else CASES
    results = []
    for name in names:
        try:
            results.append(run_case(binding, name, True, args.allow_owned_mount_fault))
        except (Unsupported, Quarantine) as error:
            status = "UNSUPPORTED" if isinstance(error, Unsupported) else "FAIL"
            results.append({"case": name, "status": status, "diagnostic": str(error)})
            print(canonical({"protocol": 1, "status": status, "journal": journal, "cases": results,
                             "acceptance_complete": False, "cloud_acceptance": False}).decode())
            return 2 if status == "UNSUPPORTED" else 1
        except Exception:
            results.append({"case": name, "status": "FAIL", "diagnostic": "LiveCaseFailedDetailsWithheld"})
            print(canonical({"protocol": 1, "status": "FAIL", "journal": journal, "cases": results,
                             "acceptance_complete": False, "cloud_acceptance": False}).decode())
            return 1
    print(canonical({"protocol": 1, "status": "PASS", "run_id": binding["run_id"],
                     "project": binding["project"], "journal": journal, "cases": results,
                     "acceptance_complete": names == CASES, "cloud_acceptance": False}).decode())
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Unsupported as error:
        print(canonical({"status": "UNSUPPORTED", "acceptance_complete": False,
                         "diagnostic": str(error)}).decode())
        sys.exit(2)
    except Quarantine as error:
        print(canonical({"status": "FAIL", "acceptance_complete": False,
                         "diagnostic": str(error)}).decode())
        sys.exit(1)
    except Exception:
        print('{"status":"FAIL","acceptance_complete":false,"diagnostic":"LiveCaseFailedDetailsWithheld"}')
        sys.exit(1)
