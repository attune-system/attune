#!/usr/bin/env python3
"""Provision the five real principals through public APIs in E's owned stack.

Does not bootstrap an administrator, build an archive or start any services.
The operator credential must already belong to this disposable deployment.
"""
import hashlib
import json
from pathlib import Path
import secrets

from fixture_support import API, read_secret, require, run, write_private


def grants(pack):
    actions = [pack + "." + name for name in ("probe", "input_probe", "input_workflow", "emit", "batch_probe", "external_probe")]
    enqueue = {"resource":"queue_items","actions":["create"],"constraints":{"refs":[pack + ".inbox",pack + ".input_inbox"]}}
    pack_read = {"resource": "packs", "actions": ["read"], "constraints": {"refs": [pack]}}
    action_read = {"resource": "actions", "actions": ["read"], "constraints": {"refs": actions}}
    return {
        "deployer": [{"resource": "packs", "actions": ["read", "install", "configure"], "constraints": {"refs": [pack]}}],
        "runtimeconsumer": [
            {"resource": "actions", "actions": ["read", "execute"], "constraints": {"refs": actions}},
            {"resource": "executions", "actions": ["read", "cancel"], "constraints": {"owner": "self"}},
            enqueue,
        ],
        "reader": [pack_read, action_read],
        "sponsor": [pack_read, action_read, enqueue,
                    {"resource":"rules","actions":["read"],"constraints":{"refs":[pack + ".input_rule"]}},
                    {"resource":"triggers","actions":["read"],"constraints":{"refs":[pack + ".input_tick"]}},
                    {"resource":"queues","actions":["read"],"constraints":{"refs":[pack + ".input_inbox"]}},
                    {"resource": "keys", "actions": ["create", "read", "decrypt"], "constraints": {"owner": "self"}}],
        "maintenance": [
            {"resource": "packs", "actions": ["read", "configure", "decrypt"], "constraints": {"refs": [pack]}},
            {"resource":"actions","actions":["read","update"],"constraints":{"refs":actions}},
            {"resource": "queues", "actions": ["read", "update"], "constraints": {"refs": [pack + ".inbox", pack + ".input_inbox"]}},
            {"resource":"queue_items","actions":["read","decrypt","delete"],"constraints":{"refs":[pack + ".inbox",pack + ".input_inbox"]}},
            {"resource":"executions","actions":["read","decrypt","cancel"],"constraints":{"pack_refs":[pack],"owner":"any"}},
            # These routes currently authorize without a target context. This
            # account is restricted to the disposable owned deployment, and is
            # never used as an installer/invoker/sponsor fallback.
            {"resource": "permissions", "actions": ["manage"]},
            {"resource": "identities", "actions": ["update"]},
        ],
    }


def provision(ctx):
    api = API(ctx["api_url"])
    root = Path(ctx["compose_file"]).resolve().parent
    pack = "ginterop_" + ctx["run_id"].replace("-", "_")
    operator = json.loads(read_secret(ctx["operator_credential_file"]))
    require(operator.get("owned_project") == ctx["project"], "Operator credential must name this exact owned deployment")
    require(operator.get("kind") == "password", "Use existing ordinary operator local login; no first-admin or integration fallback")
    access = api.call("/auth/login", body={"login": operator["login"], "password": operator["password"]}, method="POST")["data"]["access_token"]
    owner = api.call("/auth/me", access)["data"]["id"]
    roles, ids, sponsor = {}, [], None
    completed = False
    try:
        for role, permissions in grants(pack).items():
            login = pack + "." + role
            password = secrets.token_urlsafe(32)
            identity = api.call("/api/v1/identities", access, {"login": login, "display_name": login, "password": password, "attributes": {"acceptance_run": ctx["run_id"]}}, "POST")["data"]
            require(identity["id"] != owner and identity["id"] not in ids, "Provisioning reused an identity")
            ids.append(identity["id"])
            reference = "acceptance." + pack + "_" + role
            api.call("/api/v1/permissions/sets", access, {"ref": reference, "label": reference, "grants": permissions}, "POST")
            api.call("/api/v1/permissions/assignments", access, {"identity_id": identity["id"], "permission_set_ref": reference}, "POST")
            token = api.call("/auth/login", body={"login": login, "password": password}, method="POST")["data"]["access_token"]
            me = api.call("/auth/me", token)["data"]
            require(me["assigned_permission_set_refs"] == [reference], "Provisioning inherited an unexpected assignment")
            # Record the server's actual typed representation, including nulls,
            # only after checking normalized authored semantic grants.
            def normalized(values):
                return sorted(json.dumps({k: sorted(v) if k == "actions" else v for k, v in grant.items() if v is not None}, sort_keys=True) for grant in values)
            require(normalized(me["effective_permissions"]) == normalized(permissions), "Provisioned role effective grants differ from the intended minimum")
            path = root / ("g-interop-" + role + ".private.json")
            write_private(path, {"kind": "password", "run_id": ctx["run_id"], "owned_project": ctx["project"], "identity_id": identity["id"], "login": login, "password": password})
            roles[role] = {"credential_file": str(path), "identity_id": identity["id"], "login": login, "expected_permissions": me["effective_permissions"], "assigned_permission_set_refs": [reference]}
            if role == "sponsor":
                sponsor = token
        value = secrets.token_urlsafe(32)
        key = api.call("/api/v1/keys", sponsor, {"local_ref": "g_interop_source", "owner_type": "identity", "owner_identity_login": roles["sponsor"]["login"], "name": "Owned interoperability source", "value": value, "encrypted": True}, "POST")["data"]
        require(key["owner_type"] == "identity" and key["owner_identity"] == roles["sponsor"]["identity_id"], "Source key does not have the actual active sponsor owner")
        # Output provenance can encrypt even a SHA256 receipt after an SDK Key
        # read. Give only the separate controlled witness explicit current read
        # coverage of this one actual source. Never elevate installer/invoker or
        # hand the DB observer the platform encryption key.
        fixed_read = {"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[key["ref"]],"owner_types":["identity"],"owner_refs":[roles["sponsor"]["login"]]}}
        permissions = grants(pack)["maintenance"] + [fixed_read]
        set_record = api.call("/api/v1/permissions/sets/by-ref/"+roles["maintenance"]["assigned_permission_set_refs"][0],access)
        set_record = set_record.get("data",set_record)
        api.call(f"/api/v1/permissions/sets/{set_record['id']}",access,{"grants":permissions},"PUT")
        witness_credential = json.loads(read_secret(roles["maintenance"]["credential_file"]))
        witness = api.call("/auth/login",body={"login":witness_credential["login"],"password":witness_credential["password"]},method="POST")["data"]["access_token"]
        me = api.call("/auth/me",witness)["data"]
        require(normalized(me["effective_permissions"])==normalized(permissions) and me["assigned_permission_set_refs"]==roles["maintenance"]["assigned_permission_set_refs"],"Controlled receipt witness did not retain exactly its explicit scoped grants")
        roles["maintenance"]["expected_permissions"] = me["effective_permissions"]
        output = {"run_id": ctx["run_id"], "project": ctx["project"], "api_url": ctx["api_url"], "pack_ref": pack, "roles": roles, "source_key_ref": key["ref"], "source_key_id": key["id"], "input_sha256": hashlib.sha256(json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()}
        path = root / "g-interop-principals.private.json"
        write_private(path, output)
        completed = True
        return {"principals_binding_file": str(path), "role_identity_ids": ids, "source_key_id": key["id"], "pack_ref": pack}
    finally:
        if not completed:
            for identity in ids:
                api.call(f"/api/v1/identities/{identity}/freeze", access, {}, "POST")
        write_private(root / "g-interop-provision-ledger.json", {"run_id": ctx["run_id"], "identity_ids": ids, "completed": completed, "cleanup_owner": "existing lifecycle runner; no shared account deletion"})


if __name__ == "__main__":
    run(provision)
