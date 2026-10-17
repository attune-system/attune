#!/usr/bin/env python3
"""E three-phase storage fixture. Live use requires --execute and private binding.

Reads E's context on stdin; writes one JSON result on stdout. Never starts a
service, installs a pack, edits SQL state, or accepts caller-supplied check flags.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

CHECKS = ("backend_observed", "real_worker_dispatch", "pinned_execution_retained",
          "provider_cleanup_retry", "exact_provider_version", "staging_cleanup",
          "quarantine_denies_authority", "writers_joined")
TERMINAL = {"completed", "failed", "cancelled", "timeout", "abandoned"}
LIMIT = 2 * 1024 * 1024


class ProbeError(RuntimeError):
    pass


def require(value, message):
    if not value:
        raise ProbeError(message)


def private(path):
    path = Path(path)
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid()
            and not info.st_mode & 0o077 and info.st_size <= LIMIT,
            "private input must be a small owned regular mode-0600 file")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd) as stream:
        require(os.fstat(stream.fileno()) == info, "private input changed while opening")
        return stream.read(LIMIT)


def save(path, value):
    path = Path(path)
    require(not path.is_symlink(), "state cannot be a symlink")
    fd, temporary = tempfile.mkstemp(prefix=".f-probe-", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(value, stream, sort_keys=True)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def run(argv, data=None, env=None, timeout=45):
    result = subprocess.run(argv, input=data, text=True, capture_output=True,
                            env=env, timeout=timeout, check=False)
    # Never print command, stderr or credential-bearing AWS/HTTP subprocess output.
    require(result.returncode == 0, "native operation failed: " + Path(argv[0]).name)
    require(len(result.stdout) <= LIMIT, "native observation exceeds bound")
    return result.stdout


class API:
    def __init__(self, base):
        parsed = urllib.parse.urlsplit(base)
        require(parsed.scheme in ("http", "https") and parsed.hostname == "127.0.0.1" and parsed.port
                and not parsed.username and not parsed.password and not parsed.query
                and not parsed.fragment and parsed.path in ("", "/", "/api/v1", "/api/v1/"), "invalid API origin")
        self.base = urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, "", "", ""))

    def call(self, path, token=None, body=None, method="GET", expected=(200,), bytes_response=False, error_contains=None):
        auth_paths = ("/auth/login", "/auth/token-login", "/auth/me")
        require((path in auth_paths or path.startswith("/api/v1/") and path != "/api/v1/auth" and not path.startswith("/api/v1/auth/"))
                and not path.startswith("//") and not any(part in (".", "..") for part in path.split("/")), "invalid or noncanonical API path")
        headers = {"Content-Type": "application/json"}
        if token:
            headers["Authorization"] = "Bearer " + token
        data = None if body is None else json.dumps(body).encode()
        # Do not forward credentials across redirects.
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *args, **kwargs):
                return None
        try:
            response = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect).open(
                urllib.request.Request(self.base + path, data, headers, method=method), timeout=45)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            status = response.code
            raw = response.read(LIMIT + 1)
        require(len(raw) <= LIMIT, "HTTP response exceeds bound")
        require(status in expected, f"API {method} {path} returned HTTP {status}; response withheld")
        if status not in (200, 201, 202):
            if error_contains:
                require(error_contains.encode() in raw, "negative request failed for an unrelated reason, not the required custody denial")
            return {"http_status": status}
        if bytes_response:
            return {"http_status": status, "size": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}
        return json.loads(raw) if raw else {}

    def token_login(self, file):
        # E's installer/invoker outputs are raw integration-token files. This
        # method never accepts password JSON or a supplied human access token.
        token = private(file).strip()
        require(token.startswith("attune_it_") and len(token) <= 4096 and not any(c.isspace() for c in token),
                "installer/invoker credential must be a raw integration token")
        return self.call("/auth/token-login", body={"token": token}, method="POST")["data"]["access_token"]

    def password_login(self, file, run_id, project):
        record = json.loads(private(file))
        fields = {"kind", "role", "run_id", "owned_project", "identity_id", "login", "password"}
        require(isinstance(record, dict) and set(record) == fields, "maintenance credential must have exactly the typed password metadata fields")
        require(record["kind"] == "password" and record["role"] == "maintenance"
                and record["run_id"] == run_id and record["owned_project"] == project
                and record["login"] == "acceptance." + run_id.replace("-", "_") + ".maintenance"
                and type(record["identity_id"]) is int and record["identity_id"] > 0
                and isinstance(record["password"], str) and 1 <= len(record["password"]) <= 4096,
                "maintenance password identity is not bound to this owned run")
        token = self.call("/auth/login", body={"login": record["login"], "password": record["password"]}, method="POST")["data"]["access_token"]
        return token, {"id": record["identity_id"], "login": record["login"]}


class Native:
    def __init__(self, context, binding, runner=run):
        self.c, self.b, self.run = context, binding, runner
        self.docker = ["docker", "--context", context["docker_context"]]

    def container(self, service):
        require(re.fullmatch(r"[a-zA-Z0-9_-]+", service or ""), "invalid service name")
        ids = self.run(self.docker + ["ps", "-q", "--filter", "label=" + self.c["owner_label"] + "=" + self.c["run_id"],
            "--filter", "label=com.docker.compose.project=" + self.c["project"],
            "--filter", "label=com.docker.compose.service=" + service]).split()
        require(len(ids) == 1 and re.fullmatch(r"[0-9a-f]{12,64}", ids[0]), "expected one owned running service: " + service)
        obj = json.loads(self.run(self.docker + ["inspect", ids[0]]))[0]
        labels = obj["Config"]["Labels"]
        require(labels.get(self.c["owner_label"]) == self.c["run_id"]
                and labels.get("com.docker.compose.project") == self.c["project"]
                and labels.get("com.docker.compose.service") == service,
                "native service ownership mismatch")
        return obj

    def inspect(self, kind, execution_id, identity=None, version_id=None):
        require(kind in ("pins", "version", "backend", "protected", "ledger", "released", "legacy"), "unsupported typed inspection operation")
        binary = Path(self.b["inspector_binary"])
        info = binary.lstat()
        require(binary.is_absolute() and stat.S_ISREG(info.st_mode) and info.st_uid in (0, os.getuid()) and not info.st_mode & 0o022
                and os.access(binary, os.X_OK), "inspector must be an owned non-writable precompiled regular executable")
        with binary.open("rb") as stream:
            require(hashlib.file_digest(stream, "sha256").hexdigest() == self.b["inspector_sha256"], "inspector artifact digest differs from explicit binding")
        # The adapter independently validates this mode-0600 context, scope and
        # ordinary invoker's actual execution. Never put DB credentials on argv.
        context_file = self.b["inspector_context_file"]
        private(context_file)
        operation = {"kind": kind}
        if identity is not None:
            operation["identity"] = identity
        if version_id is not None:
            operation["version_id"] = version_id
        request = {"scope": {"run_id": self.c["run_id"], "project": self.c["project"], "execution_id": execution_id},
                   "operation": operation}
        data = json.dumps(request)
        require(len(data.encode()) <= 16384, "typed inspection request exceeds bound")
        result = json.loads(self.run([str(binary), "--execute"], data=data,
            env={"ATTUNE_STORAGE_PROBE_INSPECTOR_CONTEXT": context_file}, timeout=25))
        require(result.get("protocol") == 1 and "data" in result, "invalid typed inspector protocol")
        value = result["data"]
        if kind == "version" and value is not None and value.get("body_state") == "ready":
            require(result.get("identity"), "Ready inspection lacks repository-observed exact identity receipt")
            value["_inspection_identity"] = result["identity"]
        return value

    def mounted(self, service, root):
        obj = self.container(service)
        matches = [m for m in obj["Mounts"] if m["Destination"] == root and m["Type"] == "volume"]
        require(len(matches) == 1, "physical root must be an exact owned named-volume mount")
        volume = json.loads(self.run(self.docker + ["volume", "inspect", matches[0]["Name"]]))[0]
        labels = volume.get("Labels") or {}
        require(labels.get(self.c["owner_label"]) == self.c["run_id"]
                and labels.get("com.docker.compose.project") == self.c["project"], "volume ownership mismatch")
        return obj

    def physical(self, root, relative, operation="stat", expected=None):
        require(isinstance(relative, str) and relative and not relative.startswith("/")
                and all(p not in ("", ".", "..") for p in relative.split("/")), "unsafe relative physical identity")
        obj = self.mounted(self.b["inspection_service"], root)
        script = r'''
import hashlib,json,os,pathlib,stat,sys
root,rel,operation,run_id,expected=json.load(sys.stdin)
parts=pathlib.PurePosixPath(rel).parts
parent=os.open(root,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
for part in parts[:-1]:
 try: child=os.open(part,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=parent)
 except FileNotFoundError:
  if operation=="stat": print("null"); sys.exit(0)
  raise
 os.close(parent); parent=child
name=parts[-1]; hold=".f-probe-"+run_id+"-held"
def identity(name):
 try: fd=os.open(name,os.O_RDONLY|os.O_NOFOLLOW,dir_fd=parent)
 except FileNotFoundError: return None
 with os.fdopen(fd,"rb") as f:
  s=os.fstat(f.fileno())
  if not stat.S_ISREG(s.st_mode) or s.st_size>134217728: raise RuntimeError("not bounded regular file")
  digest=hashlib.file_digest(f,"sha256").hexdigest()
 return {"device":s.st_dev,"inode":s.st_ino,"size":s.st_size,"mtime_micros":s.st_mtime_ns//1000,"sha256":digest}
if operation=="swap":
 if identity(name)!=expected: raise RuntimeError("identity changed")
 # The exclusive hardlink reserves a non-overwriting same-volume hold identity.
 os.link(name,hold,src_dir_fd=parent,dst_dir_fd=parent,follow_symlinks=False)
 if identity(hold)!=expected or identity(name)!=expected: raise RuntimeError("hold identity changed")
 os.unlink(name,dir_fd=parent)
 fd=os.open(name,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600,dir_fd=parent)
 with os.fdopen(fd,"wb") as f:
  f.write(b"F owned replacement generation\n"); f.flush(); os.fsync(f.fileno())
elif operation=="restore":
 if identity(hold)!=expected["original"] or identity(name)!=expected["replacement"]: raise RuntimeError("fault identities changed; manual recovery required")
 os.unlink(name,dir_fd=parent)
 os.link(hold,name,src_dir_fd=parent,dst_dir_fd=parent,follow_symlinks=False)
 os.unlink(hold,dir_fd=parent)
elif operation!="stat": raise RuntimeError("unsupported operation")
print(json.dumps(identity(name)))
os.close(parent)
'''
        return json.loads(self.run(self.docker + ["exec", "-i", obj["Id"], self.b["python"], "-c", script],
            data=json.dumps([root, relative, operation, self.c["run_id"], expected])))

    def writer_processes(self):
        # A unique argv marker is present in the actual worker producer process.
        marker = "f-storage-probe-" + self.c["run_id"]
        found = []
        script = '''import glob,json,sys
marker=json.load(sys.stdin).encode(); found=[]
for p in glob.glob("/proc/[0-9]*/cmdline"):
 try:
  data=open(p,"rb").read()
  if marker in data and b"/proc/[0-9]*/cmdline" not in data: found.append(int(p.split("/")[2]))
 except FileNotFoundError: pass
print(json.dumps(found))
'''
        for service in self.b["writer_services"]:
            obj = self.container(service)
            pids = json.loads(self.run(self.docker + ["exec", "-i", obj["Id"], self.b["python"], "-c", script], data=json.dumps(marker)))
            found.extend([[service, pid] for pid in pids])
        return found


class S3:
    def __init__(self, c, b, runner=run):
        self.c, self.b, self.run = c, b, runner

    def call(self, operation, *args):
        credentials = json.loads(private(self.b["provider_credentials_file"]))
        require(credentials["project"] == self.c["project"] and credentials["bucket"] == self.c["owned_bucket"], "provider credential ownership mismatch")
        env = {k: v for k, v in os.environ.items() if not k.startswith("AWS_") and k.lower() not in ("http_proxy", "https_proxy", "all_proxy")}
        env.update(AWS_ACCESS_KEY_ID=credentials["access_key_id"], AWS_SECRET_ACCESS_KEY=credentials["secret_access_key"],
                   AWS_DEFAULT_REGION=credentials["region"], AWS_EC2_METADATA_DISABLED="true", AWS_CONFIG_FILE="/dev/null",
                   AWS_SHARED_CREDENTIALS_FILE="/dev/null", AWS_PAGER="")
        require(credentials["endpoint"] == self.b["provider_endpoint"], "provider endpoint differs from private binding")
        text = self.run([self.b["aws"], "--endpoint-url", credentials["endpoint"], "--no-paginate", "--output", "json",
                         "s3api", operation, "--bucket", credentials["bucket"], *args], env=env)
        return json.loads(text or "{}")

    def versions(self, key):
        key = self.b.get("object_prefix", "").strip("/") + "/" + key if self.b.get("object_prefix") else key
        values = self.call("list-object-versions", "--prefix", key, "--max-keys", "128")
        require(not values.get("IsTruncated"), "owned exact-key version inventory truncated")
        uploads = self.call("list-multipart-uploads", "--prefix", key, "--max-uploads", "128")
        require(not uploads.get("IsTruncated"), "owned exact-key multipart inventory truncated")
        return {"versions": [v["VersionId"] for v in values.get("Versions", []) if v["Key"] == key],
                "delete_markers": [v["VersionId"] for v in values.get("DeleteMarkers", []) if v["Key"] == key],
                "uploads": [v["UploadId"] for v in uploads.get("Uploads", []) if v["Key"] == key]}


class Probe:
    def __init__(self, c, b, api=None, native=None, provider=None):
        self.c, self.b = c, b
        require(c["backend"] in ("SharedFile", "ObjectSegments"), "unsupported backend")
        require(re.fullmatch(r"[a-z][a-z0-9-]{0,19}", c["run_id"]), "invalid driver run id")
        require(c["owner_label"] == "io.attune.acceptance.owner" and c["project"].startswith("eaccept-" + c["run_id"] + "-"), "unknown driver owner label or project")
        require(b["project"] == c["project"] and b["run_id"] == c["run_id"] and b["backend"] == c["backend"], "binding ownership mismatch")
        require(b["allow_native_faults"] is True, "exact owned native faults must be explicitly authorized")
        self.api = api or API(c["api_url"])
        self.native = native or Native(c, b)
        self.provider = provider or S3(c, b)
        self.state_path = Path(c["probe_state"])
        require(self.state_path.parent.resolve() == Path(c["credential_output"]).parent.resolve(), "state is outside driver's private run directory")
        credential_paths = [Path(p) for p in (c["invoker_credential_output"], b["maintenance_credential_file"], c["credential_output"])]
        require(all(p.parent.resolve() == self.state_path.parent.resolve() for p in credential_paths)
                and len({p.resolve() for p in credential_paths}) == 3, "principal credential files must be separate outputs in this run's private directory")
        self.invoker = self.api.token_login(c["invoker_credential_output"])
        self.operator, operator_identity = self.api.password_login(b["maintenance_credential_file"], c["run_id"], c["project"])
        installer = self.api.token_login(c["credential_output"])
        principals = [self.api.call("/auth/me", t)["data"] for t in [self.invoker, self.operator, installer]]
        tag = "acceptance." + c["run_id"].replace("-", "_")
        require(all(me["login"] == tag + "." + role for me, role in zip(principals, ("invoker", "maintenance", "installer")))
                and all(principals[1][k] == operator_identity[k] for k in ("id", "login")), "authenticated principals do not match the provisioned fixture roles")
        identities = [me["id"] for me in principals]
        require(all(type(identifier) is int and identifier > 0 for identifier in identities)
                and len(set(identities)) == 3, "invoker, installer and maintenance operator must be distinct real identities")
        # The owning API enforces Access with scope=None and fresh management
        # IAM. Never reinterpret an integration exchange's scope or fall back.
        self.api.call("/api/v1/retention-config/file-audits/capabilities", self.operator)
        self.invoker_id = identities[0]

    def wait(self, description, observe, timeout=None):
        deadline = time.monotonic() + (timeout or self.b["poll_seconds"])
        while True:
            value = observe()
            if value:
                return value
            require(time.monotonic() < deadline, "bounded observation timed out: " + description)
            time.sleep(0.5)

    def execution(self, identifier):
        return self.api.call(f"/api/v1/executions/{identifier}", self.invoker)["data"]

    def pins(self, identifier):
        require(type(identifier) is int and identifier > 0, "invalid execution identity")
        result = self.native.inspect("pins", identifier)
        require(isinstance(result, dict), "exact execution release/policy rows are missing or revoked")
        return result

    def owned_version(self, execution):
        return self.native.inspect("version", execution)

    def begin(self):
        require(not self.state_path.exists(), "probe state already exists; do not redispatch")
        source = Path(__file__).with_name("storage_probe_producer.py").read_bytes()
        encoded = base64.b64encode(source).decode()
        marker = "f-storage-probe-" + self.c["run_id"]
        # argv marker remains visible through exec; no tokens appear in command.
        require(re.fullmatch(r"[A-Za-z0-9_./-]+", self.b["python"]), "Python executable must be an unquoted path without whitespace")
        # core.run_agent_command's dotenv shim strips the outer quotes but does
        # not interpret embedded single-quote escapes. Use no apostrophes here.
        command = (self.b["python"] + ' -c "import base64;exec(base64.b64decode(\\"' + encoded
                   + '\\"))" ' + self.c["run_id"] + " " + str(self.b["hold_seconds"]) + " " + marker)
        execution = self.api.call("/api/v1/executions/execute", self.invoker, {
            "action_ref": "core.run_agent_command", "config": {"command": command, "require_mcp_binary": False},
            "permission_set_refs": ["standard"], "timeout_seconds": self.b["hold_seconds"],
        }, "POST", (200, 201, 202))["data"]
        identifier = execution["id"]
        state = {"schema_version": 1, "project": self.c["project"], "run_id": self.c["run_id"],
                 "backend": self.c["backend"], "execution_id": identifier, "phase": "dispatched", "observations": {}}
        save(self.state_path, state)  # Preserve ownership even if later observation fails.
        running = self.wait("real worker start", lambda: (e if (e := self.execution(identifier))["status"] == "running" and e.get("worker") else None))
        require(running["executor"] == self.invoker_id, "dispatch did not use ordinary invoker")
        pins = self.pins(identifier)
        fresh = self.c["expected_pins"]["fresh"]
        require(all(pins[k] == fresh[k] for k in ("release_id", "digest", "policy_revision")), "execution did not pin exact fresh release and policy")
        version = self.wait("actual file PUT Ready", lambda: (v if (v := self.owned_version(identifier)) and v["body_state"] == "ready" else None))
        require(version["provider_version"] and version["object_key"] and version["staging_path"], "known file request lacks exact provider/staging identities")
        protected = self.native.inspect("protected", identifier, identity=version["_inspection_identity"])
        require(protected["body"] and not protected["unsettled"], "known successful PUT lacks authenticated body or still has unsettled operations")
        require(self.native.writer_processes(), "actual producer process marker was not observed")
        backend = self.native.inspect("backend", identifier)
        require(backend == {"SharedFile": "shared_file", "ObjectSegments": "object_segments"}[self.c["backend"]], "actual worker log backend differs from requested backend")
        staging = self.native.physical(self.b["artifacts_root"], version["staging_path"])
        require(staging, "producer staging file missing before cleanup")
        state.update(pins=pins, version=version, phase="begin", staging=staging, worker=running["worker"])
        state["observations"].update(real_worker_dispatch={"execution": identifier, "worker": running["worker"], "executor": running["executor"]}, backend_observed=backend)
        if self.c["backend"] == "ObjectSegments":
            require(self.provider.call("get-bucket-versioning").get("Status") == "Enabled", "actual provider bucket versioning is not Enabled")
            obj = self.native.container(self.b["provider_service"])
            endpoint = urllib.parse.urlsplit(self.b["provider_endpoint"])
            ports = obj["NetworkSettings"]["Ports"].get(str(self.b["provider_port"]) + "/tcp") or []
            require(endpoint.scheme in ("http", "https") and endpoint.hostname in ("127.0.0.1", "localhost")
                    and any(int(p["HostPort"]) == endpoint.port for p in ports), "provider endpoint is not the owned service's explicit loopback published port")
            require(version["provider_version"].startswith("v:") and version["provider_version"][2:] not in ("", "null"), "provider did not return immutable S3 version ID")
            inventory = self.provider.versions(version["object_key"])
            require(version["provider_version"][2:] in inventory["versions"] and not inventory["uploads"], "exact provider version not present or multipart work outstanding")
            state["observations"]["provider_before"] = inventory
        else:
            state["physical"] = self.native.physical(self.b["blob_root"], version["object_key"])
            require(state["physical"], "actual owned filesystem blob missing")
            # The shipped object_store LocalFileSystem uses the inode/mtime/size
            # ETag, not an assumed name-based generation or the current HEAD.
            physical = state["physical"]
            exact = f'e:"{physical["inode"]:x}-{physical["mtime_micros"]:x}-{physical["size"]:x}"'
            require(version["provider_version"] == exact, "recorded filesystem provider identity differs from actual inode/mtime/size ETag")
        save(self.state_path, state)
        return {"backend": self.c["backend"], "execution_id": identifier}

    def load(self):
        state = json.loads(private(self.state_path))
        require(all(state[k] == self.c[k] for k in ("project", "run_id", "backend", "execution_id")), "phase state/context identity mismatch")
        return state

    def upgraded(self):
        state = self.load()
        require(state["phase"] == "begin", "upgraded requires completed begin phase")
        require(self.execution(state["execution_id"])["status"] == "running", "execution ended before upgrade observation")
        require(self.pins(state["execution_id"]) == state["pins"], "upgrade changed retained execution authority or release")
        require(self.c["expected_pins"]["upgrade"]["policy_revision"] != state["pins"]["policy_revision"], "upgrade did not actually change active policy")
        require(self.owned_version(state["execution_id"]) == state["version"], "upgrade changed exact published file")
        state["phase"] = "upgraded"
        state["observations"]["pinned_execution_retained"] = state["pins"]
        save(self.state_path, state)
        return {"backend": self.c["backend"], "execution_id": state["execution_id"], "pinned_execution_retained": True}

    def ledger(self, version):
        return self.native.inspect("ledger", version["execution"], identity=version["_inspection_identity"])

    def quarantine(self):
        identifier = self.b.get("legacy_version_id")
        require(type(identifier) is int and identifier > 0,
                "missing probe operation: real old-writer run-owned legacy_unknown file fixture ID; current public APIs cannot create legacy_unknown operations")
        owned = self.native.inspect("legacy", self.c["execution_id"], version_id=identifier)
        require(owned, "legacy fixture is not exact run-owned unknown-writer file")
        audit = self.api.call("/api/v1/retention-config/file-audits", self.operator, {"versions": [identifier]}, "POST", (201,))["data"]
        audit_id = str(audit["id"])
        require(re.fullmatch(r"[0-9a-f-]{36}", audit_id), "invalid audit identity")
        decision = self.api.call("/api/v1/retention-config/file-audits/" + audit_id + "/quarantine", self.operator, {}, "POST")["data"]
        require(decision["state"] == "manual_required", "unknown native request did not remain manual_required")
        denial = self.api.call(f"/api/v1/artifacts/{owned['artifact']}/versions/{owned['version']}/download", self.operator,
            expected=(403,), error_contains="File evidence readmission required")
        status = self.api.call(f"/api/v1/retention-config/file-evidence/{identifier}", self.operator)["data"]
        require(status["unresolved_operations"] > 0 and not status["object_done"], "unknown-writer operation was incorrectly settled")
        return {"version": identifier, "audit": audit_id, "http_status": denial["http_status"], "status": status}

    def finish(self):
        state = self.load()
        require(state["phase"] == "upgraded", "finish requires successful upgraded phase; failed cleanup needs manual inspection, not replay")
        require(self.pins(state["execution_id"]) == state["pins"], "rollback changed retained immutable execution pins")
        require(self.c["expected_pins"]["rollback"]["release_id"] == state["pins"]["release_id"], "rollback did not restore original release")
        require(self.owned_version(state["execution_id"]) == state["version"], "selected physical version changed before cleanup")
        require(self.native.physical(self.b["artifacts_root"], state["version"]["staging_path"]) == state["staging"], "selected staging identity changed before cleanup")
        state["observations"]["positive_download"] = self.api.call(
            f"/api/v1/artifacts/{state['version']['artifact']}/versions/{state['version']['version']}/download",
            self.operator, bytes_response=True)
        expected_bytes = ("F owned lifecycle " + self.c["run_id"] + "\n").encode() * 4096
        require(state["observations"]["positive_download"]["size"] == len(expected_bytes)
                and state["observations"]["positive_download"]["sha256"] == hashlib.sha256(expected_bytes).hexdigest(),
                "live protected download did not return the exact worker payload")
        # No cancellation-only settlement claim. Observe native marker disappearance too.
        self.api.call(f"/api/v1/executions/{state['execution_id']}/cancel", self.invoker, {}, "POST", (200, 202))
        self.wait("terminal worker execution", lambda: self.execution(state["execution_id"])["status"] in TERMINAL)
        self.wait("known producer native process joined", lambda: not self.native.writer_processes())
        state["observations"]["writers_joined"] = {"execution_terminal": True, "native_markers": []}
        state["phase"] = "cleanup_started"
        save(self.state_path, state)
        version = state["version"]
        before = self.ledger(version)
        require(before and not before["failed"], "exact clean baseline ledger row unavailable")
        fault_container = None
        swapped = False
        try:
            if self.c["backend"] == "ObjectSegments":
                fault_container = self.native.container(self.b["provider_service"])["Id"]
                state["active_fault"] = {"kind": "pause", "container": fault_container}
                save(self.state_path, state)
                self.native.run(self.native.docker + ["pause", fault_container])
            else:
                state["active_fault"] = {"kind": "replacement", "root": self.b["blob_root"],
                    "key": version["object_key"], "original": state["physical"]}
                save(self.state_path, state)
                state["replacement"] = self.native.physical(self.b["blob_root"], version["object_key"], "swap", state["physical"])
                swapped = True
                state["active_fault"]["replacement"] = state["replacement"]
                save(self.state_path, state)
            self.api.call(f"/api/v1/artifacts/{version['artifact']}/versions/{version['version']}", self.operator, method="DELETE")
            failed = self.wait("actual provider deletion failure recorded", lambda: (l if (l := self.ledger(version)) and l["attempts"] > before["attempts"] and l["failed"] else None))
            status = self.api.call(f"/api/v1/retention-config/file-evidence/{version['id']}", self.operator)["data"]
            require(not status["object_done"], "provider failure prematurely released object custody")
            state["observations"]["provider_failure"] = failed
            save(self.state_path, state)
        finally:
            if fault_container:
                self.native.run(self.native.docker + ["unpause", fault_container])
            if swapped:
                self.native.physical(self.b["blob_root"], version["object_key"], "restore",
                    {"original": state["physical"], "replacement": state["replacement"]})
            state.pop("active_fault", None)
            save(self.state_path, state)
        self.wait("exact provider retry and physical custody release", lambda: self.ledger(version) is None)
        # Completed cleanup removes its owner/pins, rather than returning a
        # permanent public success tombstone. Observe those exact rows natively.
        status = self.wait("both physical obligations released", lambda: (s if not any((s := self.native.inspect("released", state["execution_id"], identity=version["_inspection_identity"])).values()) else None))
        require(self.native.physical(self.b["artifacts_root"], version["staging_path"]) is None, "actual staging file remains after cleanup acknowledgement")
        if self.c["backend"] == "ObjectSegments":
            inventory = self.provider.versions(version["object_key"])
            require(version["provider_version"][2:] not in inventory["versions"] and not inventory["uploads"], "exact provider version or multipart work survived retry")
            state["observations"]["provider_after"] = inventory
        else:
            require(self.native.physical(self.b["blob_root"], version["object_key"]) is None, "original filesystem object survived cleanup")
        state["observations"].update(provider_cleanup_retry={"failed": state["observations"]["provider_failure"], "settled": status},
            exact_provider_version={"object_key": version["object_key"], "provider_version": version["provider_version"]},
            staging_cleanup={"before": state["staging"], "after": None})
        save(self.state_path, state)
        state["observations"]["quarantine_denies_authority"] = self.quarantine()
        state["phase"] = "finish"
        save(self.state_path, state)
        require(all(k in state["observations"] for k in CHECKS), "missing measured check evidence")
        return {"backend": self.c["backend"], "execution_id": state["execution_id"],
                "checks": {k: True for k in CHECKS}, "bucket_versioning": "Enabled" if self.c["backend"] == "ObjectSegments" else None,
                "evidence_file": str(self.state_path)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binding", type=Path, required=True, help="Private run-owned topology/observer binding, never a credential on argv")
    parser.add_argument("--execute", action="store_true", help="Permit bounded live API/native inspection and exact owned faults")
    args = parser.parse_args()
    require(args.execute, "live probe requires explicit --execute")
    raw = sys.stdin.read(LIMIT + 1)
    require(len(raw) <= LIMIT, "driver context exceeds bound")
    context = json.loads(raw)
    binding = json.loads(private(args.binding))
    require(30 <= binding["poll_seconds"] <= 900 and 120 <= binding["hold_seconds"] <= 3600, "invalid lifecycle time bounds")
    require(context["phase"] in ("begin", "upgraded", "finish"), "unknown lifecycle phase")
    probe = Probe(context, binding)
    print(json.dumps(getattr(probe, context["phase"])(), sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (ProbeError, OSError, ValueError, KeyError, TypeError, AttributeError, IndexError, subprocess.SubprocessError, urllib.error.URLError) as error:
        # Raw provider/native/HTTP outputs may contain credentials. Only our
        # deliberately bounded diagnostics are safe to emit.
        print(str(error) if isinstance(error, ProbeError) else "storage probe failed; inspect private evidence and explicit prerequisites", file=sys.stderr)
        sys.exit(1)
