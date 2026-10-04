#!/usr/bin/env python3
"""Publish or wait for the bundled core pack through the Attune API."""

import argparse
import base64
import gzip
import hashlib
import io
import json
import os
import secrets
import tarfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path


def request(url, *, data=None, headers=None, method=None):
    return urllib.request.urlopen(
        urllib.request.Request(url, data=data, headers=headers or {}, method=method),
        timeout=10,
    )


def wait_for_api(base_url, deadline):
    while time.monotonic() < deadline:
        try:
            with request(f"{base_url}/health/ready") as response:
                if response.status == 200:
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


def upload(base_url, token, pack_dir):
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
        print(f"core pack {expected_version} is already active; skipping upload")
        return False

    upload(base_url, token, pack_dir)
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
    return True


def create_bootstrap_token():
    database_url = (
        f"postgresql://{urllib.parse.quote(os.environ['DB_USER'], safe='')}:"
        f"{urllib.parse.quote(os.environ['DB_PASSWORD'], safe='')}@"
        f"{os.environ['DB_HOST']}:{os.environ['DB_PORT']}/{os.environ['DB_NAME']}"
    )
    import psycopg2
    from psycopg2 import sql

    secret = "attune_it_" + base64.urlsafe_b64encode(secrets.token_bytes(32)).decode().rstrip("=")
    token_hash = base64.urlsafe_b64encode(hashlib.sha256(secret.encode()).digest()).decode().rstrip("=")
    login = f"core-bootstrap-{uuid.uuid4().hex}"
    with psycopg2.connect(database_url) as connection:
        with connection.cursor() as cursor:
            cursor.execute(
                sql.SQL("SET search_path TO {}, public").format(
                    sql.Identifier(os.environ["DB_SCHEMA"])
                )
            )
            cursor.execute(
                "DELETE FROM identity WHERE attributes->>'attune_bootstrap' = 'core-pack' "
                "AND created < NOW() - INTERVAL '15 minutes' "
                "AND NOT EXISTS (SELECT 1 FROM rule WHERE owner_identity = identity.id) "
                "AND NOT EXISTS (SELECT 1 FROM pack WHERE installed_by = identity.id)"
            )
            cursor.execute(
                "INSERT INTO identity (login, display_name, attributes) "
                "VALUES (%s, %s, %s::jsonb) RETURNING id",
                (login, "Core pack bootstrap", '{"attune_bootstrap":"core-pack"}'),
            )
            identity_id = cursor.fetchone()[0]
            cursor.execute("SELECT id FROM permission_set WHERE ref = %s", ("core.admin",))
            permission_set = cursor.fetchone()
            if not permission_set:
                raise RuntimeError("core bootstrap permission set is missing")
            cursor.execute(
                "INSERT INTO permission_assignment (identity, permset) VALUES (%s, %s) "
                "ON CONFLICT (identity, permset) DO NOTHING",
                (identity_id, permission_set[0]),
            )
            cursor.execute(
                "INSERT INTO integration_token "
                "(identity, label, description, token_hash, token_prefix, token_suffix, expires_at) "
                "VALUES (%s, %s, %s, %s, %s, %s, NOW() + INTERVAL '10 minutes')",
                (
                    identity_id,
                    "Core pack bootstrap",
                    "Temporary Helm bootstrap credential",
                    token_hash,
                    secret[:18],
                    secret[-6:],
                ),
            )
    return database_url, identity_id, secret


def delete_bootstrap_identity(database_url, identity_id):
    import psycopg2
    from psycopg2 import sql

    with psycopg2.connect(database_url) as connection:
        with connection.cursor() as cursor:
            cursor.execute(
                sql.SQL("SET search_path TO {}, public").format(
                    sql.Identifier(os.environ["DB_SCHEMA"])
                )
            )
            cursor.execute("DELETE FROM integration_token WHERE identity = %s", (identity_id,))
            cursor.execute(
                "DELETE FROM identity WHERE id = %s "
                "AND NOT EXISTS (SELECT 1 FROM rule WHERE owner_identity = identity.id) "
                "AND NOT EXISTS (SELECT 1 FROM pack WHERE installed_by = identity.id)",
                (identity_id,),
            )


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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("publish", "wait"))
    args = parser.parse_args()
    base_url = os.environ["ATTUNE_API_URL"].rstrip("/")
    timeout_seconds = min(
        300, max(1, int(os.environ.get("ATTUNE_BOOTSTRAP_TIMEOUT_SECONDS", "300")))
    )
    deadline = time.monotonic() + timeout_seconds
    wait_for_api(base_url, deadline)
    if args.command == "publish":
        pack_dir = Path(os.environ.get("SOURCE_PACKS_DIR", "/source/packs")) / "core"
        expected_version = os.environ.get(
            "ATTUNE_CORE_PACK_VERSION", bundled_pack_version(pack_dir)
        )
        expected_digest = os.environ.get("ATTUNE_CORE_PACK_DIGEST") or None
        database_url, identity_id, integration_token = create_bootstrap_token()
        try:
            reconcile(
                base_url,
                token_login(base_url, integration_token),
                pack_dir,
                expected_version,
                expected_digest,
            )
        finally:
            delete_bootstrap_identity(database_url, identity_id)
    wait_for_core(base_url, deadline)


if __name__ == "__main__":
    main()
