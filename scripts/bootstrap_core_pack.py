#!/usr/bin/env python3
"""Publish or wait for the bundled core pack through the Attune API."""

import argparse
import gzip
import io
import json
import os
import tarfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path

BOOTSTRAP_CLIENT_VERSION = 1
BOOTSTRAP_REVISION = "authenticated-pack-publication-1"
BOOTSTRAP_COMPONENTS = frozenset((
    "immutable_release", "atomic_deployment",
))


def require_contract(ready):
    # This checks authenticated publication, not completion of the managed-pack
    # runtime SOW. Upload still enforces the registrar's actual authority.
    contract = ready.get("pack_publication", {})
    if (not isinstance(contract, dict)
            or type(contract.get("protocol_version")) is not int
            or contract["protocol_version"] != 1
            or contract.get("authorization") != "registrar"
            or type(contract.get("client_version")) is not int
            or contract["client_version"] != BOOTSTRAP_CLIENT_VERSION
            or contract.get("revision") != BOOTSTRAP_REVISION
            or not isinstance(contract.get("components"), list)
            or not all(isinstance(item, str) for item in contract["components"])
            or set(contract["components"]) != BOOTSTRAP_COMPONENTS):
        raise RuntimeError("API publication contract is unavailable; deploy compatible API/bootstrap versions before publishing core")


class NoApiRedirects(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # Publication credentials must never follow a server-selected destination.
        raise urllib.error.HTTPError(req.full_url, code, "API redirects are not allowed", headers, fp)


def request(url, *, data=None, headers=None, method=None):
    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username is not None or parsed.password is not None:
        raise ValueError("API URL must use HTTP or HTTPS without embedded credentials")
    opener = urllib.request.build_opener(NoApiRedirects())
    return opener.open(
        urllib.request.Request(url, data=data, headers=headers or {}, method=method),
        timeout=10,
    )


def wait_for_api(base_url, deadline):
    while time.monotonic() < deadline:
        try:
            with request(f"{base_url}/health/ready") as response:
                if response.status == 200:
                    require_contract(json.load(response))
                    return
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(2)
    raise TimeoutError("Attune API platform did not become ready before the deadline")


def token_login(base_url, token):
    body = json.dumps({"token": token}).encode()
    with request(
        f"{base_url}/auth/token-login",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    ) as response:
        return json.load(response)["data"]["access_token"]


def bundled_pack_version(pack_dir):
    import yaml

    with (pack_dir / "pack.yaml").open(encoding="utf-8") as source:
        metadata = yaml.safe_load(source)
    if metadata.get("ref") != "core" or not metadata.get("version"):
        raise RuntimeError("bundled core pack metadata is invalid")
    return str(metadata["version"])


def get_active_release(base_url, token):
    try:
        with request(
            f"{base_url}/api/v1/packs/core/releases",
            headers={"Authorization": f"Bearer {token}"},
        ) as response:
            releases = json.load(response)["data"]
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise
    return next((release for release in releases if release["is_active"]), None)


def deterministic_archive(pack_dir):
    output = io.BytesIO()
    with gzip.GzipFile(fileobj=output, mode="wb", mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w") as archive:
            for path in sorted(pack_dir.rglob("*")):
                relative = Path(pack_dir.name) / path.relative_to(pack_dir)
                info = archive.gettarinfo(str(path), str(relative))
                info.uid = 0
                info.gid = 0
                info.uname = ""
                info.gname = ""
                info.mtime = 0
                if info.isfile():
                    with path.open("rb") as source:
                        archive.addfile(info, source)
                else:
                    archive.addfile(info)
    return output.getvalue()


def require_active_admission(base_url, active):
    with request(f"{base_url}/health/bootstrap") as response:
        core = json.load(response)["core"]
    if (core["release_id"] != active["id"] or core["version"] != active["version"]
            or core["digest"] != active["digest"]):
        raise RuntimeError("core admission does not match the active release")


def upload(base_url, token, pack_dir, expected_digest=None):
    boundary = f"attune-{uuid.uuid4().hex}"
    archive = deterministic_archive(pack_dir)
    parts = []

    def field(name, value):
        parts.extend(
            [
                f"--{boundary}\r\n".encode(),
                f'Content-Disposition: form-data; name="{name}"\r\n\r\n'.encode(),
                value.encode(),
                b"\r\n",
            ]
        )

    field("force", "true")
    field("skip_tests", "true")
    if expected_digest:
        field("expected_release_digest", expected_digest)
    parts.extend(
        [
            f"--{boundary}\r\n".encode(),
            b'Content-Disposition: form-data; name="pack"; filename="core.tar.gz"\r\n',
            b"Content-Type: application/gzip\r\n\r\n",
            archive,
            b"\r\n",
            f"--{boundary}--\r\n".encode(),
        ]
    )
    with request(
        f"{base_url}/api/v1/packs/upload",
        data=b"".join(parts),
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": f"multipart/form-data; boundary={boundary}",
        },
        method="POST",
    ) as response:
        if response.status not in (200, 201):
            raise RuntimeError(f"core pack upload returned HTTP {response.status}")


def reconcile(base_url, token, pack_dir, expected_version, expected_digest):
    actual_version = bundled_pack_version(pack_dir)
    if actual_version != expected_version:
        raise RuntimeError(
            f"expected core pack version {expected_version}, bundled version is {actual_version}"
        )

    active = get_active_release(base_url, token)
    if active and active["version"] == expected_version:
        if expected_digest and active["digest"] != expected_digest:
            raise RuntimeError(
                f"active core pack {expected_version} has digest {active['digest']}, "
                f"expected {expected_digest}"
            )
        require_active_admission(base_url, active)
        print(f"core pack {expected_version} is already admitted; skipping upload")
        return False

    upload(base_url, token, pack_dir, expected_digest)
    active = get_active_release(base_url, token)
    if not active or active["version"] != expected_version:
        actual = active["version"] if active else "none"
        raise RuntimeError(
            f"core pack upload activated version {actual}, expected {expected_version}"
        )
    if expected_digest and active["digest"] != expected_digest:
        raise RuntimeError(
            f"core pack upload produced digest {active['digest']}, expected {expected_digest}"
        )
    require_active_admission(base_url, active)
    return True


def wait_for_core(base_url, deadline):
    while time.monotonic() < deadline:
        try:
            with request(f"{base_url}/health/content") as response:
                if response.status == 200:
                    return
        except (OSError, KeyError, urllib.error.URLError):
            pass
        time.sleep(2)
    raise TimeoutError("active core pack release did not appear before the deadline")


def publish_core(base_url, pack_dir):
    """Common admission call path for API, volume and former direct-loader jobs."""
    timeout = min(300, max(1, int(os.environ.get("ATTUNE_BOOTSTRAP_TIMEOUT_SECONDS", "300"))))
    wait_for_api(base_url, time.monotonic() + timeout)
    expected_version = os.environ.get("ATTUNE_CORE_PACK_VERSION", bundled_pack_version(pack_dir))
    expected_digest = os.environ.get("ATTUNE_CORE_PACK_DIGEST") or None
    integration_token = os.environ.get("ATTUNE_BOOTSTRAP_TOKEN")
    if not integration_token:
        raise RuntimeError("ATTUNE_BOOTSTRAP_TOKEN is required: supply an existing deployment credential; bootstrap never creates identities or grants")
    return reconcile(base_url, token_login(base_url, integration_token), pack_dir,
                     expected_version, expected_digest)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("publish", "wait"))
    args = parser.parse_args()
    base_url = os.environ["ATTUNE_API_URL"].rstrip("/")
    timeout_seconds = min(
        300, max(1, int(os.environ.get("ATTUNE_BOOTSTRAP_TIMEOUT_SECONDS", "300")))
    )
    deadline = time.monotonic() + timeout_seconds
    if args.command == "publish":
        pack_dir = Path(os.environ.get("SOURCE_PACKS_DIR", "/source/packs")) / "core"
        publish_core(base_url, pack_dir)
    else:
        wait_for_api(base_url, deadline)
        wait_for_core(base_url, deadline)


if __name__ == "__main__":
    main()
