"""E's normal managed installation and private context for F's real adapter."""
import hashlib
import json
import os
from pathlib import Path
import secrets
import ssl
import urllib.parse
import urllib.request
import uuid

from fixture_support import API, read_secret, require, write_private


def writer_principal(case, config):
    certificate = ssl.PEM_cert_to_DER_cert(Path(case["writer_certificate"]).read_text())
    fingerprint = hashlib.sha256(certificate).hexdigest()
    principal = config["principals"].get(fingerprint, {})
    require(principal.get("role") == "writer", "Actual writer certificate must be bound before init/setup")
    require(sum(value.get("role") == "writer" for value in config["principals"].values()) == 1, "Each fresh native lane must have exactly its own writer principal")
    for key in ("native_instance", "worker_instance", "execution_attempt"):
        require(str(uuid.UUID(principal[key])) == principal[key], "Configure fresh canonical original UUIDs before setup")
    require(str(uuid.UUID(config["generation"])) == config["generation"], "Configure an actual fresh generation UUID")
    return principal


def validate_context(case, config, private_file):
    require("member" not in case and "write" not in case, "Caller members/writes cannot replace authenticated original setup")
    template = json.loads(read_secret(case["producer_context_template_file"]))
    require(template.get("enabled_for_controlled_acceptance") is True, "Producer context's controlled gate is off")
    access = template["access"]
    require((access["run_id"], access["project"], access["generation"], access["mode"]) == (case["run_id"], case["project"], config["generation"], case["mode"]), "Producer context differs from the exact original generation/run/project/mode")
    require(access["producer_action_ref"] == "f_native_settlement.producer" and type(access["maintenance_identity"]) is int and access["maintenance_identity"] > 0, "Use the actually installed managed producer and real maintenance identity")
    url = urllib.parse.urlsplit(template["database_url"])
    require(url.scheme in ("postgres", "postgresql") and url.hostname == "127.0.0.1" and url.port and not url.query and not url.fragment and url.path == "/" + access["database_name"] and urllib.parse.unquote(url.username or "") == access["database_role"] and access["database_role"] not in ("", "postgres", "attune") and url.password and access["schema"], "Native setup requires exact owned loopback DB/schema and a non-superuser scoped role, never an app/owner fallback")
    controller = template["controller"]
    require(controller["endpoint"].rstrip("/") == case["controller_endpoint"].rstrip("/") and controller["issuer"] == config["issuer"] and controller["audience"] == config["audience"], "Producer transport must match the owning manager endpoint and signer identity")
    require(controller["storage"]["provider"] == "filesystem" and controller["storage"]["root"] == config["roots"]["blob"] and controller["artifacts_dir"] == config["roots"]["shared"], "Producer must use the exact newly owned local filesystem roots, never S3/history")
    for key in ("verification_key_file", "client_identity_file", "ca_certificate_file"):
        private_file(controller[key])
    for key in ("jwt_secret_file", "encryption_key_file"):
        private_file(template[key])
        read_secret(template[key])  # Enforce private regular uid-owned mode and size; never retain/log bytes.
    parent = Path(case["producer_context_template_file"]).parent
    for output in (case["producer_context_file"], template["maintenance_access_token_file"]):
        path = Path(output)
        require(path.is_absolute() and path.parent == parent and not os.path.lexists(path), "New private producer context/token must share the protected template directory and never adopt earlier files")
    require(case["producer_context_file"] != template["maintenance_access_token_file"], "Keep private context and access token in distinct files")
    return template


def login(case, role):
    credential = json.loads(read_secret(case[role + "_credential_file"]))
    require(credential["kind"] == "password" and credential["owned_project"] == case["project"] and credential["run_id"] == case["run_id"], "Use the separate actual run-owned password principal, no token fallback")
    api = API(case["api_url"])
    token = api.call("/auth/login", body={"login": credential["login"], "password": credential["password"]}, method="POST")["data"]["access_token"]
    me = api.call("/auth/me", token)["data"]
    require(me["id"] == credential["identity_id"] and me["login"] == credential["login"], "Principal differs from actual owned local identity")
    return api, token, me["id"]


def install(case, release, maintenance_identity):
    api, token, installer_identity = login(case, "installer")
    require(installer_identity != maintenance_identity, "Installer and native maintenance must remain distinct")
    archive = Path(release["archive_file"]).read_bytes()
    require(len(archive) <= 1024 * 1024 and hashlib.sha256(archive).hexdigest() == release["digest"], "Use the real measured canonical producer archive")
    boundary = "native" + secrets.token_hex(16)
    body = (f"--{boundary}\r\nContent-Disposition: form-data; name=\"expected_release_digest\"\r\n\r\n{release['digest']}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"pack\"; filename=\"producer.tar.gz\"\r\nContent-Type: application/gzip\r\n\r\n".encode() + archive + f"\r\n--{boundary}--\r\n".encode())
    request = urllib.request.Request(api.base + "/api/v1/packs/upload", body, {"Authorization": "Bearer " + token, "Content-Type": "multipart/form-data; boundary=" + boundary}, method="POST")
    # One attempt only. Unknown publication outcome is retained, never retried,
    # force-uploaded, registered from a server path or seeded into repositories.
    with api.opener.open(request, timeout=300) as response:
        require(response.status == 201, "Actual managed producer upload did not return 201")
        response.read(4 * 1024 * 1024)
    action = api.call("/api/v1/actions/f_native_settlement.producer", token)["data"]
    require(action["ref"] == "f_native_settlement.producer" and type(action["id"]) is int, "Managed producer action was not actually installed/admitted")
    return action["id"]


def context(case, template, token, identity):
    require(template["access"]["maintenance_identity"] == identity, "Adapter context must bind the actual authenticated maintenance identity")
    write_private(template["maintenance_access_token_file"], token)
    write_private(case["producer_context_file"], template)


def original(prepared, config, principal, mode):
    require(prepared.get("protocol") == "controlled_native_v1" and prepared["scope"] == config["scope"], "Adapter must return the actual controlled-native protocol and exact scope")
    for key in ("artifact_id", "artifact_version", "execution_id", "admission_id", "worker_id", "source_receipt"):
        require(type(prepared[key]) is int and prepared[key] > 0, "Missing actual producer/admission/source repository identity")
    member = prepared["member"]
    require(set(member) == {"operation", "version", "root_read", "object_key", "provider_version", "staging_path", "execution_attempt", "worker_instance", "native_instance"}, "Adapter returned an incomplete original member")
    require(str(uuid.UUID(member["operation"])) == member["operation"] and type(member["version"]) is int and member["version"] > 0 and type(member["root_read"]) is int and member["root_read"] > 0 and member["provider_version"] is None, "Expected a real Pending original producer, never Ready/historical publication")
    require(all(member[key] == principal[key] for key in ("native_instance", "worker_instance", "execution_attempt")), "Producer original UUIDs differ from pre-enrolled writer certificate principal")
    require((member["object_key"] is not None) == (mode != "shared") and (member["staging_path"] is not None) == (mode != "blob"), "Keep every original physical obligation in one member")
    return member


def observation(value, original_member, scope, released=False):
    require(value["member"] == original_member and value["scope"] == scope, "Repository observation changed the original member/scope")
    if released:
        require(all(type(value[key]) is int and value[key] == 0 for key in ("pins", "unresolved_operations", "evidence_owner_count")), "Real original custody/pins/operations were not released")
        audit = value["audit"]
        require(audit and audit["state"] == "completed" and audit["plan_matches_original"] is True and audit["plan"]["members"] == [original_member] and audit["plan"]["scope"] == scope, "Completed repository audit does not match the actual original producer")
        require(all(audit["receipt_counts"].get(kind, 0) > 0 for kind in ("writers", "catalogue", "absence", "settled", "controller")) and audit["controller_receipts"], "Missing actual persisted signed controller/audit evidence")
    else:
        require(value["pins"] > 0 and value["unresolved_operations"] > 0 and value["evidence_owner_count"] == 1, "Real original custody must remain pinned before native settlement")
