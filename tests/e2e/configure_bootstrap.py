"""Issue and revoke an owned E2E publication credential without printing it."""

import argparse
import json
import os
from pathlib import Path
import stat
import sys

import requests


def private_write(path, value):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        output.write(value)


def private_read(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor) as source:
        attributes = os.fstat(source.fileno())
        if not stat.S_ISREG(attributes.st_mode) or attributes.st_uid != os.getuid():
            raise RuntimeError("Bootstrap file is not owned by this runner")
        if stat.S_IMODE(attributes.st_mode) != 0o600:
            raise RuntimeError("Bootstrap file must have mode 0600")
        value = source.read(16385)
        if len(value) > 16384:
            raise RuntimeError("Bootstrap file exceeds its size limit")
        return value


def data(response, operation):
    if response.status_code not in (200, 201):
        # Response bodies can contain credential material. Never include them.
        raise RuntimeError(f"Bootstrap {operation} failed, HTTP {response.status_code}")
    return response.json()["data"]


def positive_id(value):
    if type(value) is not int or value <= 0:
        raise RuntimeError("Bootstrap API returned an invalid identifier")
    return value


def authenticated_session(environment, session_factory=requests.Session):
    session = session_factory()
    try:
        base = environment["ATTUNE_API_URL"].rstrip("/")
        login = data(session.post(base + "/auth/login", json={
            "login": environment["TEST_USER_LOGIN"],
            "password": environment["TEST_USER_PASSWORD"],
        }, timeout=20), "login")
        session.headers["Authorization"] = "Bearer " + login["access_token"]
        identity = data(session.get(base + "/auth/me", timeout=20), "identity")
        return session, base, positive_id(identity["id"])
    except BaseException:
        session.close()
        raise


def configure(operation, directory, environment, session_factory=requests.Session):
    directory = Path(directory)
    attributes = directory.lstat()
    if not stat.S_ISDIR(attributes.st_mode) or attributes.st_uid != os.getuid():
        raise RuntimeError("Bootstrap directory must be owned by this runner")
    if stat.S_IMODE(attributes.st_mode) != 0o700:
        raise RuntimeError("Bootstrap directory must have mode 0700")
    record_path = directory / "credential.json"
    token_path = directory / "token"
    if operation == "revoke" and not record_path.exists():
        return
    session, base, identity = authenticated_session(environment, session_factory)
    try:
        if record_path.exists():
            record = json.loads(private_read(record_path))
            if record["api_origin"] != base or record["identity"] != identity:
                raise RuntimeError("Bootstrap credential belongs to another API or identity")
            token_id = positive_id(record["token_id"])
            if operation == "issue":
                if not private_read(token_path).strip():
                    raise RuntimeError("Bootstrap credential file is empty")
                return
        elif operation == "issue":
            created = data(session.post(
                base + f"/api/v1/identities/{identity}/integration-tokens",
                json={"label": "e2e-bootstrap-" + environment["ATTUNE_E2E_RUN_ID"],
                      "description": "Owned ephemeral E2E core publication"},
                timeout=20,
            ), "credential creation")
            token_id = positive_id(created["integration_token"]["id"])
            try:
                token = created["token"]
                if not isinstance(token, str) or not token or any(char.isspace() for char in token):
                    raise RuntimeError("Bootstrap API returned an invalid credential")
                private_write(record_path, json.dumps({
                    "api_origin": base, "identity": identity, "token_id": token_id,
                }))
                private_write(token_path, token + "\n")
            except BaseException:
                # A local write failure must not leave an untracked credential.
                response = session.delete(
                    base + f"/api/v1/identities/{identity}/integration-tokens/{token_id}", timeout=20,
                )
                if response.status_code not in (200, 204, 404):
                    raise RuntimeError("Bootstrap credential rollback failed") from None
                raise
            return
        else:
            raise RuntimeError("Unknown bootstrap operation")
        response = session.delete(
            base + f"/api/v1/identities/{identity}/integration-tokens/{token_id}", timeout=20,
        )
        if response.status_code not in (200, 204, 404):
            raise RuntimeError(f"Bootstrap revocation failed, HTTP {response.status_code}")
        if token_path.exists():
            token = private_read(token_path).strip()
            denied = session.post(base + "/auth/token-login", json={
                "token": token,
            }, timeout=20)
            if denied.status_code != 401:
                raise RuntimeError("Bootstrap credential is still accepted after revocation")
        # Keep only a non-secret audit descriptor. Revocation is retry-safe.
        token_path.unlink(missing_ok=True)
    finally:
        session.close()


def main():
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog="Examples: configure_bootstrap.py issue --directory /run/bootstrap; "
               "configure_bootstrap.py revoke --directory /run/bootstrap",
    )
    parser.add_argument("operation", choices=("issue", "revoke"))
    parser.add_argument("--directory", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        configure(arguments.operation, arguments.directory, os.environ)
    except Exception:
        # Do not print exception chains, requests or response bodies.
        print("E2E bootstrap credential operation failed; check API readiness, setup credentials and private file ownership", file=sys.stderr)
        return 1
    print(f"E2E bootstrap credential {arguments.operation} completed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
