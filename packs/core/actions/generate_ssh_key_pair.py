#!/usr/bin/env python3
"""Generate an SSH key pair and store it in Attune's encrypted key store.

Parameters are read as JSON from stdin. Stdout contains only the declared JSON
result. Private key material is sent only to the Attune key-store API over the
worker-provided execution token and is never written to stdout or stderr.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any
from urllib import error, parse, request


def fail(message: str, exit_code: int = 1) -> None:
    print(f"ERROR: {message}", file=sys.stderr)
    raise SystemExit(exit_code)


def read_params() -> dict[str, Any]:
    raw = sys.stdin.read()
    if not raw.strip():
        return {}
    try:
        params = json.loads(raw)
    except json.JSONDecodeError as exc:
        fail(f"invalid JSON parameters: {exc.msg}")
    if not isinstance(params, dict):
        fail("parameters must be a JSON object")
    return params


def require_string(params: dict[str, Any], name: str) -> str:
    value = params.get(name)
    if not isinstance(value, str) or not value.strip():
        fail(f"{name} is required and must be a non-empty string")
    return value.strip()


def optional_string(params: dict[str, Any], name: str, default: str) -> str:
    value = params.get(name, default)
    if value is None:
        return default
    if not isinstance(value, str):
        fail(f"{name} must be a string")
    return value


def optional_int(params: dict[str, Any], name: str, default: int) -> int:
    value = params.get(name, default)
    if isinstance(value, bool) or not isinstance(value, int):
        fail(f"{name} must be an integer")
    return value


def run_ssh_keygen(args: list[str]) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            ["ssh-keygen", *args],
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except FileNotFoundError:
        fail("ssh-keygen is required on the worker but was not found in PATH")
    except subprocess.CalledProcessError as exc:
        detail = (exc.stderr or exc.stdout or "").strip()
        if detail:
            fail(f"ssh-keygen failed: {detail}")
        fail("ssh-keygen failed")


def try_ssh_keygen(args: list[str]) -> subprocess.CompletedProcess[str] | None:
    try:
        return subprocess.run(
            ["ssh-keygen", *args],
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except (FileNotFoundError, subprocess.CalledProcessError):
        return None


def generate_key_pair(key_type: str, bits: int, comment: str) -> tuple[str, str, str]:
    if key_type not in {"ed25519", "rsa"}:
        fail("key_type must be one of: ed25519, rsa")

    if key_type == "rsa" and (bits < 2048 or bits > 8192):
        fail("bits must be between 2048 and 8192 for RSA keys")

    with tempfile.TemporaryDirectory(prefix="attune-ssh-key-") as temp_dir:
        key_path = Path(temp_dir) / "id_attune"
        args = ["-q", "-t", key_type, "-N", "", "-C", comment, "-f", str(key_path)]
        if key_type == "rsa":
            args[3:3] = ["-b", str(bits)]
        run_ssh_keygen(args)

        private_key = key_path.read_text(encoding="utf-8")
        public_key = key_path.with_suffix(".pub").read_text(encoding="utf-8").strip()

        fingerprint_result = try_ssh_keygen(
            ["-l", "-E", "sha256", "-f", str(key_path.with_suffix(".pub"))]
        )
        if fingerprint_result is None:
            fingerprint_result = run_ssh_keygen(["-l", "-f", str(key_path.with_suffix(".pub"))])
        fingerprint_output = fingerprint_result.stdout
        fingerprint_parts = fingerprint_output.strip().split()
        fingerprint = fingerprint_parts[1] if len(fingerprint_parts) >= 2 else ""

    return private_key, public_key, fingerprint


def api_url() -> str:
    base_url = os.environ.get("ATTUNE_API_URL", "").strip()
    if not base_url:
        fail("ATTUNE_API_URL is not set")
    parsed = parse.urlsplit(base_url)
    if (
        parsed.scheme not in {"http", "https"}
        or not parsed.hostname
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
    ):
        fail("ATTUNE_API_URL must be an HTTP(S) URL without credentials, query, or fragment")
    return base_url.rstrip("/")


def api_token() -> str:
    token = os.environ.get("ATTUNE_API_TOKEN", "").strip()
    if not token:
        fail("ATTUNE_API_TOKEN is not set; this action requires execution permission set core.key_creator or equivalent keys:create access")
    return token


def safe_api_error(exc: error.HTTPError) -> str:
    message = f"Attune key-store API request failed with HTTP {exc.code}"
    body = exc.read()
    if not body:
        return message
    try:
        parsed = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return message

    for key in ("message", "error"):
        value = parsed.get(key) if isinstance(parsed, dict) else None
        if isinstance(value, str) and value:
            return f"{message}: {value}"
    return message


class NoRedirectHandler(request.HTTPRedirectHandler):
    """Prevent execution credentials from being forwarded to another origin."""

    def redirect_request(
        self,
        req: request.Request,
        fp: Any,
        code: int,
        msg: str,
        headers: Any,
        newurl: str,
    ) -> request.Request | None:
        return None


def call_key_api(method: str, key_ref: str | None, payload: dict[str, Any]) -> dict[str, Any]:
    path = "/api/v1/keys"
    if key_ref is not None:
        path = f"{path}/{parse.quote(key_ref, safe='')}"

    data = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    req = request.Request(
        f"{api_url()}{path}",
        data=data,
        method=method,
        headers={
            "Authorization": f"Bearer {api_token()}",
            "Content-Type": "application/json",
            "Accept": "application/json",
        },
    )
    opener = request.build_opener(request.ProxyHandler({}), NoRedirectHandler())
    try:
        with opener.open(req, timeout=30) as response:
            response_body = response.read()
    except error.HTTPError as exc:
        raise RuntimeError(safe_api_error(exc)) from exc
    except error.URLError as exc:
        raise RuntimeError(f"Attune key-store API request failed: {exc.reason}") from exc

    if not response_body:
        return {}
    try:
        parsed = json.loads(response_body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise RuntimeError("Attune key-store API returned an invalid JSON response") from exc
    return parsed if isinstance(parsed, dict) else {}


def response_data(response: dict[str, Any]) -> dict[str, Any]:
    data = response.get("data")
    return data if isinstance(data, dict) else {}


def main() -> None:
    params = read_params()

    if shutil.which("ssh-keygen") is None:
        fail("ssh-keygen is required on the worker but was not found in PATH")

    local_ref = require_string(params, "local_ref")
    key_type = optional_string(params, "key_type", "ed25519").lower()
    bits = optional_int(params, "bits", 4096)
    comment = optional_string(params, "comment", local_ref)
    name = optional_string(params, "name", f"SSH Key Pair ({local_ref})")
    owner_type = optional_string(params, "owner_type", "pack").lower()
    owner_pack_ref = optional_string(params, "owner_pack_ref", "core")
    owner_action_ref = optional_string(params, "owner_action_ref", "core.generate_ssh_key_pair")

    if owner_type not in {"pack", "action"}:
        fail("owner_type must be one of: pack, action")

    # Validate API access before generating any secret key material.
    api_url()
    api_token()

    private_key, public_key, fingerprint = generate_key_pair(key_type, bits, comment)

    owner_ref = owner_pack_ref if owner_type == "pack" else owner_action_ref
    create_payload: dict[str, Any] = {
        "local_ref": local_ref,
        "owner_type": owner_type,
        "name": name,
        "value": {
            "private_key": private_key,
            "public_key": public_key,
            "fingerprint": fingerprint,
            "key_type": key_type,
            "bits": bits if key_type == "rsa" else None,
            "comment": comment,
        },
        "encrypted": True,
    }
    if owner_type == "pack":
        create_payload["owner_pack_ref"] = owner_pack_ref
    else:
        create_payload["owner_action_ref"] = owner_action_ref

    try:
        api_response = call_key_api("POST", None, create_payload)
    except RuntimeError as exc:
        fail(str(exc))

    stored = response_data(api_response)
    result = {
        "key_ref": stored.get("ref") if isinstance(stored.get("ref"), str) else local_ref,
        "public_key": public_key,
        "fingerprint": fingerprint,
        "key_type": key_type,
        "comment": comment,
        "owner_type": owner_type,
        "owner_ref": owner_ref,
        "encrypted": True,
        "created": True,
    }
    if isinstance(stored.get("id"), int):
        result["key_id"] = stored["id"]
    if key_type == "rsa":
        result["bits"] = bits
    print(json.dumps(result, separators=(",", ":")))


if __name__ == "__main__":
    main()
