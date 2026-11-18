#!/usr/bin/env python3
"""Actual public helper refuses direct-host execution; no real credentials used."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="attune-helper-guard-") as directory:
        root = Path(directory)
        sentinel = root / "must-not-run"
        environment = {
            "PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": directory,
            "ATTUNE_CONFIG": str(root / "must-not-load-config"),
            "ATTUNE__SECURITY__ENCRYPTION_KEY": "owned-dummy-manager-credential",
        }
        plan = {"version": 1, "environment": {"PATH": "/usr/bin:/bin"},
                "steps": [{"argv": ["/usr/bin/touch", str(sentinel)], "if_pack_file": None}]}
        for arguments, body, expected_success in [
            (["--help"], b"", True),
            (["unsupported"], b"", False),
            (["setup"], json.dumps(plan).encode(), False),
        ]:
            result = subprocess.run([str(binary), *arguments], input=body, env=environment,
                                    cwd=root, capture_output=True, timeout=10, check=False)
            assert (result.returncode == 0) == expected_success, "unexpected helper startup result"
            assert b"owned-dummy-manager-credential" not in result.stdout + result.stderr
            assert not sentinel.exists(), "public helper executed a plan outside admitted OCI"
        print("public helper help and direct-host rejection passed (not an OCI positive proof)")


if __name__ == "__main__":
    main()
