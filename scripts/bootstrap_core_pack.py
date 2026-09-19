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
import subprocess
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


def create_bootstrap_token(pack_dir):
    database_url = (
        f"postgresql://{urllib.parse.quote(os.environ['DB_USER'], safe='')}:"
        f"{urllib.parse.quote(os.environ['DB_PASSWORD'], safe='')}@"
        f"{os.environ['DB_HOST']}:{os.environ['DB_PORT']}/{os.environ['DB_NAME']}"
    )
    environment = os.environ.copy()
    environment["PGOPTIONS"] = f"-c search_path={os.environ['DB_SCHEMA']},public"
    subprocess.run(
        [
            "python3",
            os.environ.get("LOADER_SCRIPT", "/scripts/load_core_pack.py"),
            "--database-url",
            database_url,
            "--pack-dir",
            str(pack_dir.parent),
            "--pack-name",
            pack_dir.name,
            "--schema",
            os.environ["DB_SCHEMA"],
        ],
        env=environment,
        check=True,
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
                "AND created < NOW() - INTERVAL '15 minutes'"
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
            cursor.execute("DELETE FROM identity WHERE id = %s", (identity_id,))


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
        database_url, identity_id, integration_token = create_bootstrap_token(pack_dir)
        try:
            upload(base_url, token_login(base_url, integration_token), pack_dir)
        finally:
            delete_bootstrap_identity(database_url, identity_id)
    wait_for_core(base_url, deadline)


if __name__ == "__main__":
    main()
