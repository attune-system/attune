#!/usr/bin/env python3
"""Run the protocol-relay suite on exclusively owned PostgreSQL 16/18 servers."""
import os
from pathlib import Path
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]


def command(*args, **kw):
    kw.setdefault("timeout", 30)
    return subprocess.check_output(args, text=True, **kw).strip()


def main():
    for version in (16, 18):
        token = uuid.uuid4().hex[:12]
        name = f"attune-session-{version}-{token}"
        try:
            command("docker", "run", "--detach", "--name", name,
                    "--label", f"attune.session-owner={token}",
                    "--env", "POSTGRES_PASSWORD=owned-session-test",
                    "--publish", "127.0.0.1::5432", f"postgres:{version}-alpine", timeout=180)
            deadline = time.monotonic() + 60
            while subprocess.run(["docker", "exec", name, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10).returncode:
                if time.monotonic() >= deadline:
                    raise RuntimeError("owned PostgreSQL readiness deadline")
                time.sleep(0.1)
            port = command("docker", "port", name, "5432").split(":")[-1]
            env = dict(os.environ, SESSION_TEST_URL=f"postgres://postgres:owned-session-test@127.0.0.1:{port}/postgres?sslmode=disable",
                       ATTUNE_TEST_RUN_ID=f"s{token}")
            print(f"PostgreSQL {version}: ATTUNE_TEST_RUN_ID=s{token}; owned container={name}", flush=True)
            subprocess.run(["cargo", "test", "--locked", "--manifest-path",
                            str(ROOT / "tests/sqlx-session/Cargo.toml"), "--",
                            "--nocapture", "--test-threads=4"], env=env, check=True, timeout=2400)
        finally:
            # Verify ownership even on partial setup failure. A failed inspect
            # is not proof of absence when the daemon itself is unavailable.
            present = command("docker", "ps", "--all", "--filter", f"name=^/{name}$", "--format", "{{.ID}}", timeout=30)
            if present:
                owner = command("docker", "inspect", "--format", '{{index .Config.Labels "attune.session-owner"}}', name, timeout=30)
                if owner != token:
                    raise RuntimeError(f"refusing cleanup of unowned container: {name}")
                command("docker", "rm", "--force", "--volumes", name, timeout=180)
                if command("docker", "ps", "--all", "--filter", f"name=^/{name}$", "--format", "{{.ID}}", timeout=30):
                    raise RuntimeError(f"owned container remains: {name}")


if __name__ == "__main__":
    main()
