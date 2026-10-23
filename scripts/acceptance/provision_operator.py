#!/usr/bin/env python3
"""Provision separate installer/invoker through ordinary public APIs.

Run only via the owned driver. An existing operator credential is mandatory.
"""
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import secrets
import re

from fixture_support import API, FixtureError, read_secret, require, run, write_private


INSTALLER = [{"resource": "packs", "actions": ["read", "install", "configure"], "constraints": {"refs": ["core"]}}]
INVOKER = [
    {"resource": "actions", "actions": ["read", "execute"], "constraints": {"refs": ["core.echo", "core.sleep", "core.noop"]}},
    {"resource": "executions", "actions": ["read", "cancel"], "constraints": {"owner": "self"}},
]
# Identity admin routes currently authorize without a target context. A narrower
# ids constraint cannot mint tokens through those routes. This temporary issuer
# receives only identities:update, never a role or permissions:manage.
ISSUER = [{"resource": "identities", "actions": ["update"]}]


def principals(ctx):
    roles = [("installer", INSTALLER), ("invoker", INVOKER)]
    storage = ctx.get("storage_fixture")
    if storage is None:
        return roles
    require(storage.get("allow_shell_invocation") is True and storage.get("allow_maintenance_management") is True, "Concrete storage fixture requires explicit isolated shell and maintenance-management consent")
    run_ref = "core.f_probe_" + ctx["run_id"].replace("-", "_")
    legacy_ref = storage.get("legacy_artifact_ref", "")
    legacy_owner = storage.get("legacy_owner_ref", "")
    require(bool(re.fullmatch(r"core\.[a-zA-Z0-9_]+", legacy_ref)) and bool(re.fullmatch(r"core\.[a-zA-Z0-9_]+", legacy_owner)), "Supply exact real run-owned legacy artifact and action owner refs")
    invoker = [
        {"resource": "actions", "actions": ["read", "execute"], "constraints": {"refs": ["core.run_agent_command"]}},
        {"resource": "executions", "actions": ["read", "cancel"], "constraints": {"owner": "self"}},
    ]
    maintenance = [
        # Protected plaintext file GET explicitly checks Read AND Decrypt.
        {"resource": "artifacts", "actions": ["read", "decrypt", "delete"], "constraints": {"refs": sorted({run_ref, legacy_ref}), "owner_types": ["action"], "owner_refs": sorted({"core.run_agent_command", legacy_owner})}},
        {"resource": "retention", "actions": ["read", "update"]},
        {"resource": "permissions", "actions": ["manage"]},
    ]
    return [("installer", INSTALLER), ("invoker", invoker), ("maintenance", maintenance)]


def normalize(grants):
    return sorted(json.dumps(dict({key: value for key, value in grant.items() if value is not None}, actions=sorted(grant["actions"])), sort_keys=True) for grant in grants)


def verify_me(api, token, identity, grants, reference):
    me = api.call("/auth/me", token)["data"]
    require(me["id"] == identity and me["assigned_permission_set_refs"] == [reference], "Unexpected identity or inherited role grants")
    require(normalize(me["effective_permissions"]) == normalize(grants), "Effective permissions differ from least-scoped fixture grants")
    return me


def provision(ctx, api=None):
    api = api or API(ctx["api_url"])
    root = Path(ctx["compose_file"]).resolve().parent
    output = Path(ctx["credential_output"])
    invoker_output = Path(ctx["invoker_credential_output"])
    roles = principals(ctx)
    outputs = {"installer": output, "invoker": invoker_output}
    if ctx.get("storage_fixture") is not None:
        outputs["maintenance"] = Path(ctx["maintenance_credential_output"])
    require(len(set(outputs.values())) == len(outputs) and all(path.parent.resolve() == root and not path.exists() for path in outputs.values()), "Credential outputs must be new separate files in this run's private directory")
    human = json.loads(read_secret(ctx["operator_credential_file"]))
    require(human.get("api_url") == ctx["api_url"] or human.get("owned_project") == ctx["project"], "Operator credential file is not explicitly bound to this owned deployment")
    if human.get("kind") == "password":
        operator = api.call("/auth/login", body={"login": human["login"], "password": human["password"]}, method="POST")["data"]["access_token"]
    else:
        require(human.get("kind") == "access_token", "Operator file kind must be password or access_token, never an integration credential")
        operator = human["access_token"]
    operator_me = api.call("/auth/me", operator)["data"]
    tag = "acceptance." + ctx["run_id"].replace("-", "_")
    evidence = {"operator_identity": operator_me["id"], "identities": {}, "issuer_frozen": False}
    issuer_token = None
    issuer_id = None
    try:
        for role, grants in [("issuer", ISSUER), *roles]:
            password = secrets.token_urlsafe(32) if role in ("issuer", "maintenance") else None
            identity = api.call("/api/v1/identities", operator, {"login": tag + "." + role, "display_name": tag + " " + role, "password": password, "attributes": {"acceptance_run": ctx["run_id"]}}, "POST")["data"]
            require(identity["id"] != operator_me["id"] and identity["frozen"] is False, "Fixture must create a distinct active identity")
            require(identity["id"] not in [record["id"] for record in evidence["identities"].values()], "Installer/invoker/maintenance/issuer identities must be distinct")
            if role == "issuer":
                issuer_id = identity["id"]
            reference = tag + "_" + role
            evidence["identities"][role] = {"id": identity["id"], "permission_set_ref": reference}
            api.call("/api/v1/permissions/sets", operator, {"ref": reference, "label": reference, "grants": grants}, "POST")
            api.call("/api/v1/permissions/assignments", operator, {"identity_id": identity["id"], "permission_set_ref": reference}, "POST")
            if role == "issuer":
                issuer_token = api.call("/auth/login", body={"login": identity["login"], "password": password}, method="POST")["data"]["access_token"]
                verify_me(api, issuer_token, issuer_id, grants, reference)
            elif role == "maintenance":
                # The audit operator guard intentionally accepts unscoped Access
                # only. A new local password account uses that supported login
                # path, never an integration exchange or the supplied operator.
                credential = {"kind": "password", "role": "maintenance", "run_id": ctx["run_id"], "owned_project": ctx["project"], "identity_id": identity["id"], "login": identity["login"], "password": password}
                access = api.call("/auth/login", body={"login": identity["login"], "password": password}, method="POST")["data"]["access_token"]
                me = verify_me(api, access, identity["id"], grants, reference)
                require(me.get("auth_provider") == "local" and me.get("is_local") is True and me.get("can_change_password") is True, "Maintenance account must support actual local password authentication")
                evidence["identities"][role].update(auth_method="password", secret=credential, access=access)
            else:
                minted = api.call(f"/api/v1/identities/{identity['id']}/integration-tokens", issuer_token, {"label": reference, "expires_at": (datetime.now(timezone.utc) + timedelta(hours=2)).isoformat()}, "POST")["data"]
                metadata = minted["integration_token"]
                require(metadata["created_by"] == issuer_id and metadata["identity_id"] == identity["id"] and metadata["active"] is True, "Integration credential provenance mismatch")
                access = api.call("/auth/token-login", body={"token": minted["token"]}, method="POST")["data"]["access_token"]
                verify_me(api, access, identity["id"], grants, reference)
                evidence["identities"][role].update(token_id=metadata["id"], secret=minted["token"], access=access)
        api.call(f"/api/v1/identities/{issuer_id}/freeze", operator, {}, "POST")
        require(api.call(f"/api/v1/identities/{issuer_id}", operator)["data"]["frozen"] is True, "Issuer freeze was not persisted")
        api.call("/auth/me", issuer_token, expected=(401, 403))
        evidence["issuer_frozen"] = True
        for role, grants in roles:
            record = evidence["identities"][role]
            # Test old access and fresh authentication after freezing only issuer.
            verify_me(api, record["access"], record["id"], grants, record["permission_set_ref"])
            if role == "maintenance":
                access = api.call("/auth/login", body={"login": record["secret"]["login"], "password": record["secret"]["password"]}, method="POST")["data"]["access_token"]
            else:
                access = api.call("/auth/token-login", body={"token": record["secret"]}, method="POST")["data"]["access_token"]
            verify_me(api, access, record["id"], grants, record["permission_set_ref"])
            record["access"] = access
        if "maintenance" in outputs:
            # This is a real API authorization check, not a caller success flag.
            # Never bypass the existing unscoped local operator requirement.
            maintenance = evidence["identities"]["maintenance"]
            try:
                api.call("/api/v1/retention-config/file-audits/capabilities", maintenance["access"])
            except FixtureError:
                raise FixtureError("Maintenance password capability check failed; verify the separate local account and current IAM. No operator/integration-token fallback permitted") from None
        for role, path in outputs.items():
            write_private(path, evidence["identities"][role]["secret"])
        for record in evidence["identities"].values():
            record.pop("secret", None)
            record.pop("access", None)
        write_private(root / "operator-provision-evidence.json", evidence)
        return dict(evidence, ordinary_operator_provisioning=True, invoker_credential_output=str(invoker_output), maintenance_credential_output=str(outputs["maintenance"]) if "maintenance" in outputs else None)
    finally:
        try:
            if issuer_id is not None and not evidence["issuer_frozen"]:
                # Only this freshly created identity, never the supplied operator.
                api.call(f"/api/v1/identities/{issuer_id}/freeze", operator, {}, "POST")
                evidence["issuer_frozen"] = True
        finally:
            # Retain exact known IDs even when the capability gate rejects the
            # maintenance principal. No claim that lost/uncertain HTTP effects
            # were rolled back, and no secret-bearing failure dump.
            records = {role: {key: record[key] for key in ("id", "permission_set_ref", "token_id") if key in record} for role, record in evidence["identities"].items()}
            write_private(root / "operator-provision-state.private.json", {"identities": records, "issuer_frozen": evidence["issuer_frozen"]})


if __name__ == "__main__":
    run(provision)
