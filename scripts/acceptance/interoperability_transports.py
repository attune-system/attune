"""Real foreground transports for the owned interoperability probe.

No subprocess output is persisted. Credentials enter through a private profile
or stdin, never argv. Missing transports are errors, not fallback HTTP clients.
"""
import json
import os
import signal
import subprocess

from fixture_support import require, write_private


def foreground(argv, payload, env, timeout=90, allow_failure=False):
    process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, env=env,
                               start_new_session=True)
    try:
        output, _ = process.communicate(payload, timeout=timeout)
        require(allow_failure or process.returncode == 0, "Transport process failed; output suppressed")
        require(len(output) <= 4 * 1024 * 1024, "Transport response exceeds limit")
        return (process.returncode, output) if allow_failure else output
    finally:
        # Join the complete owned process group, including a timed-out MCP/SDK.
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


def environment(home):
    return {"PATH": "/usr/bin:/bin", "HOME": str(home),
            "XDG_CONFIG_HOME": str(home / ".config"), "LANG": "C.UTF-8",
            "TMPDIR": str(home),
            "NO_PROXY": "127.0.0.1,localhost", "PYTHONDONTWRITEBYTECODE": "1"}


class Transport:
    def __init__(self, lane, settings, base, token, root):
        self.lane, self.settings, self.base, self.token = lane, settings, base, token
        home = root / lane
        home.mkdir(mode=0o700)
        self.env = environment(home)
        if lane in ("cli", "mcp"):
            config = home / ".config" / "attune"
            config.mkdir(parents=True, mode=0o700)
            write_private(config / "config.yaml", {
                "current_profile": "interop", "profiles": {
                    "interop": {"api_url": base, "auth_token": token}},
                "default_output_format": "json"})

    def call(self, operation, expect_denial=False, **args):
        if self.lane == "cli":
            return self.cli(operation, args, expect_denial)
        if self.lane == "mcp":
            return self.mcp(operation, args, expect_denial)
        request = dict(operation=operation, api_url=self.base, access_token=self.token, expect_denial=expect_denial, **args)
        result = json.loads(foreground(self.settings["command"], json.dumps(request), self.env))
        return denial(result.get("native_denial")) if expect_denial else result

    def cli(self, operation, args, expect_denial=False):
        pack = args.get("pack")
        stdin = ""
        commands = {
            "targets": ["pack", "input-approval", "targets", pack],
            "approval_get": ["pack", "input-approval", "show", pack, str(args.get("id"))],
            "approval_revoke": ["pack", "input-approval", "revoke", pack, str(args.get("id")), "--yes"],
            "pack_get": ["pack", "show", pack],
            "queue_get": ["queue", "show", args.get("ref")],
            "schema_get": ["schema-type", "show", args.get("ref")],
            "permission_export": ["permission", "set", "export", args.get("ref")],
        }
        if operation in ("runtime_issue", "runtime_replace"):
            command = ["pack", "approval", "issue" if operation == "runtime_issue" else "replace", pack]
            if operation == "runtime_replace":
                command += [str(args["id"]), "--yes"]
            command += ["--file", "-"]
            stdin = json.dumps(args["body"])
        elif operation == "enqueue":
            command = ["queue", "enqueue", args["ref"], "--request-file", "-"]
            stdin = json.dumps(args["body"])
        elif operation == "issue":
            command = ["pack", "input-approval", "issue", pack, "--file", "-"]
            stdin = json.dumps(args["body"])
        elif operation == "execute":
            body = args["body"]
            command = ["action", "execute", body["action_ref"], "--params-json", json.dumps(body.get("parameters", {}))]
            if "permission_set_refs" in body:
                refs = body["permission_set_refs"]
                if refs:
                    for ref in refs:
                        command += ["--permission-set", ref]
                else:
                    command += ["--no-api-token"]
        elif operation == "config_set":
            command = ["pack", "config", "set", pack, "--file", "-"]
            stdin = json.dumps(args["config"])
        elif operation == "config_reset":
            command = ["pack", "config", "reset", pack, *args["fields"], "--yes"]
        elif operation == "queue_grouping":
            command = ["queue", "update", args["ref"], "--requester-grouping", args["grouping"]]
        elif operation == "permission_import":
            command = ["permission", "set", "update", args["ref"], "--file", "-"]
            stdin = json.dumps(args["definition"])
        else:
            require(operation in commands, "Unsupported CLI operation")
            command = commands[operation]
        output = foreground([*self.settings["command"], "--profile", "interop", "--output", "json", *command], stdin, self.env, allow_failure=expect_denial)
        if expect_denial:
            code, output = output
            require(code != 0, "Native CLI unexpectedly accepted revoked/expired authority")
            return denial(json.loads(output))
        return json.loads(output)

    def mcp(self, operation, args, expect_denial=False):
        mapping = {
            "enqueue": ("queues_enqueue", {**(args.get("body") or {}), "ref": args.get("ref")}),
            "runtime_issue": ("pack_approvals_issue", {**(args.get("body") or {}), "ref": args.get("pack")}),
            "runtime_replace": ("pack_approvals_replace", {**(args.get("body") or {}), "ref": args.get("pack"), "id": args.get("id"), "confirm_revocation": True}),
            "targets": ("pack_input_approval_targets_list", {"ref": args.get("pack")}),
            "issue": ("pack_input_approvals_issue", {**(args.get("body") or {}), "ref": args.get("pack")}),
            "approval_get": ("pack_input_approvals_get", {"ref": args.get("pack"), "id": args.get("id")}),
            "approval_revoke": ("pack_input_approvals_revoke", {"ref": args.get("pack"), "id": args.get("id"), "confirm_revocation": True}),
            "execute": ("actions_execute", args.get("body")),
            "pack_get": ("packs_get", {"ref": args.get("pack")}),
            "config_set": ("packs_update_config", {"ref": args.get("pack"), "config": args.get("config")}),
            "config_reset": ("packs_reset_config", {"ref": args.get("pack"), "fields": args.get("fields"), "confirm_reset": True}),
            "queue_get": ("queues_get", {"ref": args.get("ref")}),
            "queue_grouping": ("queues_update_requester_grouping", {"ref": args.get("ref"), "grouping": args.get("grouping")}),
            "schema_get": ("schema_types_get", {"ref": args.get("ref")}),
            "permission_export": ("permission_sets_export", {"ref": args.get("ref")}),
            "permission_import": ("permission_sets_import", {"ref": args.get("ref"), "definition": args.get("definition"), "confirm_import": True}),
        }
        require(operation in mapping, "Unsupported MCP operation")
        name, arguments = mapping[operation]
        messages = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "owned-interop", "version": "1"}}},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": name, "arguments": arguments}},
        ]
        output = foreground([*self.settings["command"], "--profile", "interop"], "".join(json.dumps(m) + "\n" for m in messages), self.env)
        responses = [json.loads(line) for line in output.splitlines() if line.strip()]
        matches = [r for r in responses if r.get("id") == 2]
        require(len(matches) == 1 and "error" not in matches[0], "MCP protocol failed")
        result = matches[0]["result"]
        if expect_denial:
            require(result.get("isError") is True, "Native MCP unexpectedly accepted revoked/expired authority")
            return denial(result.get("structuredContent"))
        require(result.get("isError") is False, "MCP tool failed; no REST fallback")
        return result["structuredContent"]


def unwrap(value):
    # API envelopes and MCP data wrappers are explicit, never discard purpose.
    if isinstance(value, dict) and "data" in value:
        return value["data"]
    return value


def resource_path(operation, args):
    from urllib.parse import quote
    pack = quote(args.get("pack", ""), safe="")
    ref = quote(args.get("ref", ""), safe="")
    prefix = f"/api/v1/packs/{pack}"
    paths = {
        "runtime_issue": ("POST", prefix + "/external-approvals", args.get("body")),
        "runtime_replace": ("POST", prefix + f"/external-approvals/{args.get('id')}/replace", args.get("body")),
        "enqueue": ("POST", "/api/v1/queues/" + ref + "/items", args.get("body")),
        "targets": ("GET", prefix + "/input-approval-targets", None),
        "issue": ("POST", prefix + "/external-input-approvals", args.get("body")),
        "approval_get": ("GET", prefix + f"/external-approvals/{args.get('id')}", None),
        "approval_revoke": ("POST", prefix + f"/external-approvals/{args.get('id')}/revoke", {}),
        "execute": ("POST", "/api/v1/executions/execute", args.get("body")),
        "pack_get": ("GET", prefix, None),
        "config_set": ("PUT", prefix, {"config": args.get("config")}),
        "config_reset": ("PUT", prefix, {"config_reset_fields": args.get("fields")}),
        "queue_get": ("GET", "/api/v1/queues/" + ref, None),
        "schema_get": ("GET", "/api/v1/schema-types/" + ref, None),
        "permission_export": ("GET", "/api/v1/permissions/sets/by-ref/" + ref, None),
    }
    require(operation in paths, "Unsupported public API operation")
    return paths[operation]


def authored(record):
    require(isinstance(record.get("authored_definition"), dict), "Canonical authored_definition missing; never export effective grants")
    definition = dict(record["authored_definition"])
    definition.update(label=record.get("label"), description=record.get("description"))
    return definition


def denial(value):
    import re
    require(isinstance(value, dict) and type(value.get("status")) is int and value["status"] in (400, 403, 409, 422), "Missing native HTTP denial status; parser/network/command errors are not revocation proofs")
    require(isinstance(value.get("code"), str) and re.fullmatch(r"[A-Za-z][A-Za-z0-9_]{0,63}", value["code"]), "Missing safe native machine diagnostic; no fabricated API fallback")
    result = {"status": value["status"], "code": value["code"]}
    request_id = value.get("request_id")
    if isinstance(request_id, str) and re.fullmatch(r"[A-Za-z0-9_.:/-]{1,256}", request_id):
        result["request_id"] = request_id
    return result
