#!/usr/bin/env python3
"""Real non-admin interoperability, called by the existing owned lifecycle runner.

Usage: python3 scripts/acceptance/interoperability_probe.py --execute < context.private.json
Requires E's integrated fixture_support. Does not build, launch services or adopt
developer profiles. The owning runner must await this probe before stack teardown.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import tarfile
import time
from urllib.parse import quote

from fixture_support import API, FixtureError, read_secret, require, run, write_private
from interoperability_transports import Transport, authored, environment, foreground, resource_path, unwrap
from interoperability_observer import Observer, consumed_input_ids
from interoperability_scenarios import Scenarios

LANES = ("api", "cli", "mcp", "python", "javascript", "java")
ROLES = ("deployer", "maintenance", "runtimeconsumer", "reader", "sponsor")
HERE = Path(__file__).resolve().parent


def sha(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def validate(ctx, binding):
    require(binding.get("run_id") == ctx["run_id"] and binding.get("project") == ctx["project"], "Interop binding is not owned by this lifecycle run")
    require(binding.get("api_url") == ctx["api_url"], "Interop API origin differs from owned runner")
    require(set(binding.get("roles", {})) == set(ROLES), "All five separate principals are required; none may be omitted")
    require(set(binding.get("transports", {})) == set(LANES) - {"api"}, "All CLI/MCP/SDK lanes are required; zero skips")
    prefix = "ginterop_" + ctx["run_id"].replace("-", "_")
    require(binding.get("pack_ref") == prefix, "Pack must have this run's exact unique ref")
    for field in ("action_ref", "input_action_ref", "runtime_action_ref", "queue_ref", "schema_ref", "permission_ref"):
        require(re.fullmatch(re.escape(prefix) + r"\.[a-z][a-z0-9_]*", binding.get(field, "")), "Fixture references must be owned by this exact pack: " + field)
    require(binding["action_ref"] != binding["input_action_ref"], "Input and runtime-only action scenarios must be distinct")
    require(re.fullmatch(r"[0-9a-f]{64}", binding.get("input_sha256", "")), "Expected canonical input digest is required, not a plaintext secret")
    require(re.fullmatch(r"[0-9a-f]{64}", binding.get("expected_release_digest", "")), "Supply the canonical archive helper's measured release digest")
    require(re.fullmatch(r"[A-Za-z0-9_.:@-]+", binding.get("source_key_ref", "")), "Supply the real source Key's canonical reference")
    require(binding.get("config_field") == "interop_counter" and type(binding.get("config_default")) is int and binding["config_default"] != 77, "Use the fixture's public integer field interop_counter with a distinct release default")
    require(binding.get("default_refs") and isinstance(binding["default_refs"], list), "Fixture needs nonempty admitted runtime defaults")
    require(binding["default_refs"]==[prefix+".runtime"],"Fixture must admit its exact local runtime set for real action reads and historical explicit-ref UI checks")
    require(all(ref == "standard" or ref.startswith(prefix + ".") for ref in binding["default_refs"]), "Default runtime sets must be pack-local")
    root = Path(ctx["compose_file"]).resolve().parent
    require(not (root / "g-interoperability").exists(), "Interop root exists; do not adopt a previous run")
    compose = json.loads(Path(ctx["compose_file"]).read_text())
    require(compose.get("services") and all(service.get("labels", {}).get("io.attune.acceptance.owner") == ctx["run_id"] for service in compose["services"].values()), "All service ownership labels must match the existing runner")
    require(all(re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", service.get("image", "")) for service in compose["services"].values()), "Actual immutable images are required; no mock or build fallback")
    artifacts = binding.get("artifacts", {})
    require(artifacts, "Pin actual central binary/SDK/browser/archive source artifacts")
    require(binding.get("inspector_binary") in artifacts and binding.get("inspector_context_template_file") in artifacts, "Pin the actual read-only repository inspector and private owned observer template")
    require(Path(binding["inspector_binary"]).name=="interoperability_inspector" and os.access(binding["inspector_binary"],os.X_OK),"Use the actual centrally compiled repository inspector, not an external PASS adapter")
    require(binding.get("inspector_release_digests") == [binding.get("expected_release_digest")], "Inspector must pin the actual canonical fixture release, not current refs")
    for name, digest in artifacts.items():
        path = Path(name)
        require(path.is_absolute() and path.is_file() and not path.is_symlink() and sha(path) == digest, "Missing or changed central artifact; supply actual builds, never silently mock")
    for name in ("provision_interoperability.py", "interoperability_probe.py", "interoperability_transports.py", "interoperability_python_sdk.py", "interoperability_js_sdk.mjs", "interoperability_browser.mjs", "interoperability_worker.py", "InteroperabilityJavaSdk.java", "interoperability_observer.py", "interoperability_scenarios.py", "interoperability_batch_worker.py", "interoperability_emitter.py"):
        require(str(HERE / name) in artifacts, "Pin the integrated probe source: " + name)
    for lane, config in binding["transports"].items():
        command = config.get("command")
        require(isinstance(command, list) and command and all(isinstance(arg, str) and arg for arg in command), "Supply actual foreground command for " + lane)
        require(command[0] in artifacts and os.access(command[0], os.X_OK), "Transport executable must be prebuilt and pinned")
        if lane in ("cli", "mcp"):
            require(len(command) == 1 and Path(command[0]).name == ("attune" if lane == "cli" else "attune-mcp"), "Use actual Attune binaries, not adapter commands")
        elif lane == "python":
            require(command[1:] == [str(HERE / "interoperability_python_sdk.py")], "Use the concrete repo Python SDK probe")
        elif lane == "javascript":
            require(len(command) == 3 and command[1] == str(HERE / "interoperability_js_sdk.mjs") and command[2] in artifacts, "Use the concrete repo JS SDK probe and pinned built entrypoint")
        else:
            require(len(command) == 4 and command[1] == "-cp" and command[3] == "InteroperabilityJavaSdk", "Supply the centrally compiled Java probe and actual SDK classpath")
            require(all(Path(p).is_absolute() and Path(p).exists() for p in command[2].split(os.pathsep)), "Java classpath prerequisites are missing")
            for path in map(Path, command[2].split(os.pathsep)):
                artifact = path / "InteroperabilityJavaSdk.class" if path.is_dir() else path
                require(str(artifact) in artifacts, "Pin each actual SDK jar and the compiled Java probe class")
    browser = binding.get("browser", {})
    command = browser.get("command", [])
    require(len(command) == 3 and command[1] == str(HERE / "interoperability_browser.mjs") and command[0] in artifacts and command[2] in artifacts, "Supply the concrete browser script with an existing pinned Playwright entrypoint; never install or mock it")
    require(browser.get("executable") in artifacts and os.access(browser["executable"], os.X_OK), "Actual owned Chromium image/executable is missing")
    API(browser.get("web_url", ""))
    require(str(Path(binding["archive_file"])) in artifacts, "Pin the canonical local fixture archive")
    with tarfile.open(binding["archive_file"], "r:*") as archive:
        entries = list(archive)
        for name in ("interoperability_worker.py","interoperability_batch_worker.py","interoperability_emitter.py"):
            matches = [entry for entry in entries if entry.isfile() and entry.name.endswith("/"+name)]
            require(len(matches) == 1, "Archive must contain exactly one real source: " + name)
            content = archive.extractfile(matches[0]).read(1024 * 1024)
            require(hashlib.sha256(content).hexdigest() == sha(HERE / name), "Admitted probe source differs from reviewed worker; no fixture result substitution")
    return root / "g-interoperability"


class Probe:
    def __init__(self, ctx, binding, root):
        self.ctx, self.b, self.root = ctx, binding, root
        self.api = API(ctx["api_url"])
        self.tokens, self.identities = {}, {}
        self.approvals, self.executions, self.evidence = [], [], []
        self.transports = {}
        self.upload_started = False
        self.observer = None
        self.saved_pins = {}
        self.queue_item_ids = {}

    def note(self, case, **fields):
        require(len(self.evidence)<4096,"Bounded interoperability evidence budget exceeded")
        self.evidence.append(dict(case=case, **fields))

    def call(self, role, operation, **args):
        if operation == "queue_grouping":
            path = "/api/v1/queues/" + args["ref"]
            current = unwrap(self.api.call(path, self.tokens[role]))
            config = dict(current.get("config") or {})
            config["dispatch"] = dict(config.get("dispatch") or {}, separate_requesters=args["grouping"] == "separate")
            return self.api.call(path, self.tokens[role], {"config": config}, "PUT")
        if operation == "permission_import":
            current = unwrap(self.api.call("/api/v1/permissions/sets/by-ref/" + args["ref"], self.tokens[role]))
            body = {key: value for key, value in args["definition"].items() if key != "ref"}
            return self.api.call(f"/api/v1/permissions/sets/{current['id']}", self.tokens[role], body, "PUT")
        method, path, body = resource_path(operation, args)
        result = self.api.call(path, self.tokens[role], body, method)
        return authored(unwrap(result)) if operation == "permission_export" else result

    def principals(self):
        for role in ROLES:
            record = self.b["roles"][role]
            credential = json.loads(read_secret(record["credential_file"]))
            require(credential.get("run_id") == self.ctx["run_id"] and credential.get("owned_project") == self.ctx["project"] and credential.get("kind") == "password", "Use separate owned local-password principals, not an admin Access token fallback")
            token = self.api.call("/auth/login", body={"login": credential["login"], "password": credential["password"]}, method="POST")["data"]["access_token"]
            me = unwrap(self.api.call("/auth/me", token))
            require(me["id"] == record["identity_id"] and me["id"] not in self.identities.values(), "Principals must be distinct real identities")
            require(me["login"] == record["login"] == credential["login"], "Principal login differs from the bound actual owner")
            require(canonical(me["effective_permissions"]) == canonical(record["expected_permissions"]) and me["assigned_permission_set_refs"] == record["assigned_permission_set_refs"], "Unexpected inherited roles/grants in " + role)
            require(not me.get("frozen", False), "Principal must start active")
            grants = me["effective_permissions"]
            if role in ("deployer", "runtimeconsumer", "reader"):
                require(all(g["resource"] not in ("keys", "permissions", "identities") for g in grants), "Ordinary principal acquired internal runtime or administrative authority")
                require(all(g.get("constraints", {}).get("refs") or g.get("constraints", {}).get("owner") == "self" for g in grants), "Ordinary grants must be scoped, not global")
            self.tokens[role], self.identities[role] = token, me["id"]
            self.note("principal", role=role, identity_id=me["id"], grants_sha256=hashlib.sha256(canonical(grants).encode()).hexdigest())
        key = unwrap(self.api.call("/api/v1/keys/" + quote(self.b["source_key_ref"], safe=""), self.tokens["sponsor"]))
        require(key["owner_type"] == "identity" and key["owner_identity"] == self.identities["sponsor"], "External input approval must bind the actual authorized identity owner")

    def upload(self):
        # No force, skipped tests or generation. Canonical archive is supplied by
        # the owning E/D fixture with actual native/image prerequisites.
        boundary = "ginterop" + secrets.token_hex(16)
        content = Path(self.b["archive_file"]).read_bytes()
        require(len(content) < 64 * 1024 * 1024, "Probe archive exceeds bounded fixture size")
        body = (f"--{boundary}\r\nContent-Disposition: form-data; name=\"expected_release_digest\"\r\n\r\n{self.b['expected_release_digest']}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"pack\"; filename=\"interop.tar.gz\"\r\nContent-Type: application/gzip\r\n\r\n".encode() + content + f"\r\n--{boundary}--\r\n".encode())
        import urllib.request
        import urllib.error
        request = urllib.request.Request(self.api.base + "/api/v1/packs/upload", body, {"Authorization": "Bearer " + self.tokens["deployer"], "Content-Type": "multipart/form-data; boundary=" + boundary}, method="POST")
        self.upload_started = True
        try:
            response = self.api.opener.open(request, timeout=300)
        except urllib.error.HTTPError as response:
            response.close()
            raise FixtureError("Limited deployer upload failed; no admin/force/skip-tests fallback") from None
        with response:
            require(response.status == 201, "Real pack upload did not return 201")
            uploaded = unwrap(json.loads(response.read(4 * 1024 * 1024)))
        pack = unwrap(self.call("deployer", "pack_get", pack=self.b["pack_ref"]))
        require(pack["ref"] == self.b["pack_ref"], "Upload did not publish the owned pack")
        self.observer = Observer(self, pack["id"])
        self.note("limited_deployer_upload", pack_id=pack["id"], archive_sha256=sha(self.b["archive_file"]))
        return uploaded

    def wait(self, identity, execution, refs, nonce, input_expected=False, input_approval=None):
        execution = unwrap(execution)
        require(type(execution.get("id")) is int and execution["id"] > 0, "Transport did not return a real execution ID")
        eid = execution["id"]
        self.executions.append(eid)
        deadline = time.monotonic() + 120
        while True:
            if input_expected:
                native = self.observer.execution(eid)
                row = {"permission_set_refs":native["selected_refs"],"status":native["status"],"worker":native["worker_id"]}
            else:
                row = unwrap(self.api.call(f"/api/v1/executions/{eid}", self.tokens[identity]))
            require(row["permission_set_refs"] == refs, "Stored selection differs from omitted/default/empty/explicit request")
            if row["status"] in ("completed", "failed", "cancelled", "timeout", "abandoned"):
                break
            require(time.monotonic() < deadline, "Real worker execution deadline expired")
            time.sleep(0.2)
        require(row["status"] == "completed" and row.get("worker") is not None, "Execution did not complete on an actual worker")
        receipt = {"protocol":"interop-v1", "nonce":nonce,"api_token_present":bool(refs),"runtime_read_status":200 if refs else None}
        if input_expected:
            receipt["input_sha256"] = self.b["input_sha256"]
        actual = self.observer.execution(eid, receipt)
        require(actual.get("worker_receipt_verified") is True and actual["selected_refs"] == refs, "Saved worker receipt or native refs differ from the real invocation")
        if input_expected:
            require(type(input_approval) is int and input_approval in consumed_input_ids(actual), "Immutable input receipt did not consume the exact fixed approval")
        self.saved_pins[eid] = actual
        require(actual["schema_binding_digest"] is not None, "Execution lost its admitted schema binding")
        self.note("real_worker_execution", execution_id=eid, selection=refs, schema_binding_sha256=actual["schema_binding_digest"], input_checked=input_expected)
        return eid

    def execute(self, lane, action, refs="omitted", input_expected=False, input_approval=None):
        nonce = secrets.token_hex(12)
        body = {"action_ref": action, "parameters": {"nonce": nonce, "runtime_action_ref": self.b["runtime_action_ref"]}}
        if refs != "omitted":
            body["permission_set_refs"] = refs
        if lane == "api":
            result = self.call("runtimeconsumer", "execute", body=body)
        else:
            result = self.transports[(lane, "runtimeconsumer")].call("execute", body=body)
        self.note("selection_request", lane=lane, mode="omitted" if refs == "omitted" else "empty" if not refs else "explicit")
        return self.wait("runtimeconsumer", result, self.b["default_refs"] if refs == "omitted" else refs, nonce, input_expected, input_approval)

    def approval(self, lane):
        transport = self.transports.get((lane, "sponsor"))
        invoke = (lambda operation, **args: self.call("sponsor", operation, **args)) if lane == "api" else transport.call
        targets = unwrap(invoke("targets", pack=self.b["pack_ref"]))["items"]
        targets = [target for target in targets if target["invocation_binding"] == {"kind":"action","action_ref":self.b["input_action_ref"]}]
        require(len(targets) == 1, "Fixture must have exactly one visible exact action input declaration")
        target = targets[0]
        grant = {"resource": "keys", "actions": ["read", "decrypt"], "scope": "external", "constraints": {"refs": [self.b["source_key_ref"]], "owner_types": ["identity"], "owner_refs": [self.b["roles"]["sponsor"]["login"]]}}
        body = {key: target[key] for key in ("policy_revision", "invocation_binding", "declaration_revision", "source_path", "expression")}
        body["grants"] = [grant]
        approval = unwrap(invoke("issue", pack=self.b["pack_ref"], body=body))
        aid = approval["id"]
        self.approvals.append(aid)
        require(approval["purpose"] == {"kind": "declared_input", "targets": [target]}, "Consent purpose or immutable target changed")
        direct = unwrap(self.call("sponsor", "approval_get", pack=self.b["pack_ref"], id=aid))
        require(direct["purpose"] == approval["purpose"] and direct["issuer_identity_id"] == self.identities["sponsor"], "Transport did not persist the actual sponsor's exact input-only approval")
        self.execute(lane, self.b["input_action_ref"], [], input_expected=True, input_approval=aid)
        revoked = unwrap(invoke("approval_revoke", pack=self.b["pack_ref"], id=aid))
        require(revoked["active"] is False and revoked["purpose"] == approval["purpose"], "Revocation reclassified input-only consent")
        require(unwrap(self.call("maintenance", "approval_get", pack=self.b["pack_ref"], id=aid))["active"] is False, "Revocation was not persisted")
        self.note("input_consent_roundtrip", lane=lane, approval_id=aid, policy_revision=target["policy_revision"], purpose="declared_input")
        return body

    def roundtrips(self, lane):
        invoke = (lambda operation, **args: self.call("maintenance", operation, **args)) if lane == "api" else self.transports[(lane, "maintenance")].call
        schema = unwrap(invoke("schema_get", ref=self.b["schema_ref"]))
        require(schema, "Named schema read returned no definition")
        original = unwrap(self.call("maintenance", "queue_get", ref=self.b["queue_ref"]))["config"]
        require(original.get("dispatch", {}).get("separate_requesters", False) is False, "Queue fixture must begin with default aggregate semantics")
        def unrelated(config):
            config = json.loads(canonical(config))
            config.setdefault("dispatch", {}).pop("separate_requesters", None)
            return config
        for grouping in ("aggregate", "separate", "aggregate"):
            invoke("queue_grouping", ref=self.b["queue_ref"], grouping=grouping)
            queue = unwrap(self.call("maintenance", "queue_get", ref=self.b["queue_ref"]))
            require(queue["config"]["dispatch"]["separate_requesters"] is (grouping == "separate"), "Queue grouping failed real persisted roundtrip")
            require(unrelated(queue["config"]) == unrelated(original), "Grouping mutation changed unrelated queue configuration")
        field = self.b["config_field"]
        invoke("config_set", pack=self.b["pack_ref"], config={field: 77})
        require(unwrap(self.call("maintenance", "pack_get", pack=self.b["pack_ref"]))["config"][field] == 77, "Whole-field configuration override did not persist")
        invoke("config_reset", pack=self.b["pack_ref"], fields=[field])
        require(unwrap(self.call("maintenance", "pack_get", pack=self.b["pack_ref"]))["config"][field] == self.b["config_default"], "Configuration reset did not restore the release default")
        exported = unwrap(invoke("permission_export", ref=self.b["permission_ref"]))
        require(exported.get("ref") == self.b["permission_ref"] and "grants" in exported and "id" not in exported, "Export is not a canonical authorable definition")
        invoke("permission_import", ref=self.b["permission_ref"], definition=exported)
        actual = self.call("maintenance", "permission_export", ref=self.b["permission_ref"])
        require(actual == exported, "Permission export/import changed authored selectors, scope or grants")
        self.note("management_roundtrips", lane=lane, queue_groupings=3, reset=True, authored_export_import=True, schema_read=True)

    def browser(self, phase, **args):
        settings = self.b["browser"]
        credentials = {}
        for role in ("runtimeconsumer", "sponsor"):
            credential = json.loads(read_secret(self.b["roles"][role]["credential_file"]))
            credentials[role] = {key: credential[key] for key in ("login", "password")}
        request = dict(phase=phase, web_url=settings["web_url"], api_url=self.ctx["api_url"],
                       browser_executable=settings["executable"], credentials=credentials,
                       forbidden_source_sha256=self.b["input_sha256"],explicit_refs=self.b["default_refs"],pack=self.b["pack_ref"], action=self.b["action_ref"], **args)
        result = json.loads(foreground(settings["command"], json.dumps(request), environment(self.root), timeout=180))
        require(result.get("real_browser") is True, "Missing real browser evidence")
        return result

    def execute_all(self):
        self.principals()
        self.upload()
        endpoint = "/api/v1/executions/execute"
        # True anonymous call and a separate authenticated reader denial.
        self.api.call(endpoint, body={"action_ref": self.b["action_ref"]}, method="POST", expected=(401,))
        self.api.call(endpoint, self.tokens["reader"], {"action_ref": self.b["action_ref"]}, "POST", expected=(403,))
        for lane, settings in self.b["transports"].items():
            for role in ("runtimeconsumer", "sponsor", "maintenance"):
                role_root = self.root / role
                role_root.mkdir(mode=0o700, exist_ok=True)
                self.transports[(lane, role)] = Transport(lane, settings, self.api.base, self.tokens[role], role_root)
        for lane in LANES:
            for refs in ("omitted", [], self.b["default_refs"]):
                self.execute(lane, self.b["action_ref"], refs)
            self.approval(lane)
            self.roundtrips(lane)
        scenarios = Scenarios(self)
        scenarios.runtime_approvals()
        scenarios.input_variants()
        scenarios.queue_batches()
        # A permission import can legitimately readmit the pack. Rediscover now,
        # rather than substituting the latest policy into a retained target.
        last_body = self.approval("api")
        browser = self.browser("consent", body=last_body)
        self.approvals.append(browser["approval_id"])
        require(unwrap(self.call("maintenance", "approval_get", pack=self.b["pack_ref"], id=browser["approval_id"]))["active"] is False, "Browser did not persist revocation")
        self.note("browser_consent", approval_id=browser["approval_id"])
        persistent = unwrap(self.call("sponsor", "issue", pack=self.b["pack_ref"], body=last_body))
        self.approvals.append(persistent["id"])
        for role in ("deployer", "sponsor"):
            self.api.call(f"/api/v1/identities/{self.identities[role]}/freeze", self.tokens["maintenance"], {}, "POST")
            self.api.call("/auth/me", self.tokens[role], expected=(401, 403))
        for lane in LANES:
            self.execute(lane, self.b["action_ref"])
            self.execute(lane, self.b["input_action_ref"], [], input_expected=True, input_approval=persistent["id"])
        options = unwrap(self.api.call("/api/v1/actions/" + self.b["action_ref"] + "/execution-options", self.tokens["runtimeconsumer"]))
        require(options["authority_source"] == "pack" and options["default_permission_set_refs"] == self.b["default_refs"] and type(options["policy_revision_id"]) is int, "Post-freeze execution options lost admitted pack authority")
        browser = self.browser("execute", parameters={"runtime_action_ref": self.b["runtime_action_ref"]}, policy_revision_id=options["policy_revision_id"], default_refs=self.b["default_refs"])
        require(len(browser["executions"]) == 3, "Browser must submit default, empty and explicit selections")
        for entry in browser["executions"]:
            self.wait("runtimeconsumer", {"id": entry["id"]}, self.b["default_refs"] if entry["mode"] in ("default","explicit") else [], entry["nonce"])
        self.note("browser_execution_after_human_freeze", count=3)
        self.call("maintenance", "approval_revoke", pack=self.b["pack_ref"], id=persistent["id"])
        path = "/api/v1/executions?action_ref=" + quote(self.b["input_action_ref"], safe="") + "&page_size=100"
        before = self.api.call(path, self.tokens["runtimeconsumer"])
        require(not before["pagination"]["has_next"], "Denial inventory exceeded bound")
        self.api.call(endpoint, self.tokens["runtimeconsumer"], {"action_ref": self.b["input_action_ref"], "permission_set_refs": [], "parameters": {"nonce": secrets.token_hex(12), "runtime_action_ref": self.b["runtime_action_ref"]}}, "POST", expected=(400, 403, 409, 422))
        after = self.api.call(path, self.tokens["runtimeconsumer"])
        require(not after["pagination"]["has_next"] and {e["id"] for e in before["items"]} == {e["id"] for e in after["items"]}, "Revoked consent still created an execution")
        self.note("operational_revocation_denies_new_input_execution", approval_id=persistent["id"])
        scenarios.browser_readmission(browser)
        require({r["lane"] for r in self.evidence if r["case"] == "input_consent_roundtrip"} == set(LANES), "Missing required live transport coverage")
        require({(r["lane"],r["binding_kind"]) for r in self.evidence if r["case"] == "all_input_binding"} == {(lane,kind) for lane in LANES for kind in ("action","workflow_task","rule","queue")}, "Missing a mandatory native invocation binding case")
        require({(r["lane"],r["grouping"]) for r in self.evidence if r["case"] == "real_multi_requester_batch"} == {(lane,g) for lane in LANES for g in ("aggregate","separate")}, "Missing real requester dispatch/confidentiality coverage")
        require({r["lane"] for r in self.evidence if r["case"] == "runtime_replace_expiry"} == set(LANES), "Missing native runtime lifecycle coverage")
        require({(r["lane"],r["reason"]) for r in self.evidence if r["case"] == "native_postrevocation_diagnostic"} == {(lane,reason) for lane in LANES for reason in ("input_revoked","runtime_replaced","runtime_expired")}, "Missing native postrevocation diagnostics")
        require(any(r["case"]=="browser_historical_rerun_readmission" for r in self.evidence), "Missing actual browser historical selection/readmission proof")
        from interoperability_observer import stable_pins
        for eid, saved in self.saved_pins.items():
            retained = self.observer.execution(eid)
            require(stable_pins(retained)==stable_pins(saved) and consumed_input_ids(retained)==consumed_input_ids(saved),"Readmission/revocation replaced an earlier saved admission or input approval receipt")
        self.note("all_saved_pins_retained",count=len(self.saved_pins))

    def cleanup(self):
        errors = 0
        if self.upload_started and "maintenance" in self.tokens:
            for queue in (self.b["queue_ref"],self.b["pack_ref"]+".input_inbox"):
                try:
                    self.api.call("/api/v1/queues/"+queue,self.tokens["maintenance"],{"enabled":False},"PUT")
                    page = self.api.call("/api/v1/queues/"+queue+"/items?page_size=100",self.tokens["maintenance"])
                    require(not page["pagination"]["has_next"],"Owned queue cleanup inventory exceeded bound")
                    self.queue_item_ids[queue] = {item["id"] for item in page["items"]}
                except Exception:
                    errors += 1
        if "maintenance" in self.tokens:
            try:
                # Recover uncertain browser/HTTP mutations using this run's
                # exclusive pack and action IDs, never global age-based scans.
                ledger = self.api.call("/api/v1/packs/" + self.b["pack_ref"] + "/external-approvals?page_size=100", self.tokens["maintenance"])
                require(not ledger["pagination"]["has_next"], "Approval cleanup inventory exceeded bound")
                self.approvals = sorted(set(self.approvals) | {a["id"] for a in ledger["items"]})
            except Exception:
                # Missing pack after a failed pre-upload setup has no approvals.
                if self.upload_started:
                    errors += 1
        if "runtimeconsumer" in self.tokens:
            for action in [self.b["pack_ref"] + "." + name for name in ("probe","input_probe","input_workflow","emit","batch_probe","external_probe")]:
                try:
                    page = self.api.call("/api/v1/executions?action_ref=" + quote(action, safe="") + "&page_size=100", self.tokens["maintenance"])
                    require(not page["pagination"]["has_next"], "Execution cleanup inventory exceeded bound")
                    self.executions = sorted(set(self.executions) | {e["id"] for e in page["items"] if e["action_ref"] == action})
                except Exception:
                    errors += 1
        # Only IDs returned during this run. Never delete fixture identities,
        # shared keys or packs while other owned writers may still use them.
        for aid in self.approvals:
            try:
                self.call("maintenance", "approval_revoke", pack=self.b["pack_ref"], id=aid)
            except Exception:
                errors += 1
        for eid in self.executions:
            try:
                row = self.observer.execution(eid) if self.observer is not None else unwrap(self.api.call(f"/api/v1/executions/{eid}", self.tokens["maintenance"]))
                if row["status"] not in ("completed", "failed", "cancelled", "timeout", "abandoned"):
                    self.api.call(f"/api/v1/executions/{eid}/cancel", self.tokens["maintenance"], {}, "POST")
                    deadline = time.monotonic() + 30
                    while self.observer.execution(eid)["status"] not in ("completed", "failed", "cancelled", "timeout", "abandoned"):
                        require(time.monotonic() < deadline, "Owned execution cancellation did not settle")
                        time.sleep(0.2)
            except Exception:
                errors += 1
        for queue, ids in self.queue_item_ids.items():
            try:
                if ids:
                    self.api.call("/api/v1/queues/"+queue+"/items/query/apply",self.tokens["maintenance"],{"selector":{"path":"$.id ? (@ == $ids[*])","vars":{"ids":sorted(ids)}},"operation":"cancel","preview_limit":100},"POST")
                page = self.api.call("/api/v1/queues/"+queue+"/items?page_size=100",self.tokens["maintenance"])
                require(not page["pagination"]["has_next"] and all(item["status"] in ("completed","failed","skipped","cancelled") for item in page["items"]),"Owned pending/leased queue items did not settle during cleanup")
            except Exception:
                errors += 1
        for lane in ("cli", "mcp"):
            for role in ("runtimeconsumer", "sponsor", "maintenance"):
                path = self.root / role / lane / ".config" / "attune" / "config.yaml"
                try:
                    path.unlink(missing_ok=True)
                except OSError:
                    errors += 1
        if self.observer is not None:
            try:
                self.observer.close()
            except OSError:
                errors += 1
        return {"approval_ids": self.approvals, "execution_ids": self.executions,"queue_item_ids":{queue:sorted(ids) for queue,ids in self.queue_item_ids.items()},
                "errors": errors, "infrastructure_cleanup": "owning lifecycle runner must stop/join writers and remove only label-verified owned resources"}


def probe(ctx):
    binding = json.loads(read_secret(ctx["interoperability_binding_file"]))
    root = validate(ctx, binding)
    root.mkdir(mode=0o700)
    instance = Probe(ctx, binding, root)
    completed = False
    try:
        instance.execute_all()
        completed = True
    finally:
        cleanup = instance.cleanup()
        write_private(root / "evidence.json", {"run_id": ctx["run_id"], "cases": instance.evidence,
                      "completed": completed, "cleanup": cleanup,
                      "artifact_sha256": binding["artifacts"], "backend": ctx.get("backend"),
                      "image_refs": {name: service["image"] for name, service in json.loads(Path(ctx["compose_file"]).read_text())["services"].items()},
                      "interoperability_criteria_passed": completed and cleanup["errors"]==0,
                      "full_sow_acceptance": False,
                      "acceptance_scope": "Only mandatory interoperability criteria; whole-SOW acceptance remains central-owned"})
        require(cleanup["errors"] == 0, "Interop cleanup failed; inspect bounded ID ledger before lifecycle teardown")
    return {"real_server_matrix_completed": completed, "interoperability_criteria_passed":completed, "lanes": list(LANES), "browser": True,
            "evidence_file": str(root / "evidence.json"), "full_sow_acceptance": False}


if __name__ == "__main__":
    run(probe)
