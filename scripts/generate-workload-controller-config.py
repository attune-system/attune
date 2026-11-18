#!/usr/bin/env python3
"""Write public fixed-policy controller config; no daemon/credential access.

Images/engine IDs are operator inputs, not attested by this generator. Actual
startup verifies the authenticated CP binding, image platform and cache volume.
"""
import argparse
import json
import os
from pathlib import Path
import re
from urllib.parse import urlsplit


def pinned(value):
    match = re.fullmatch(r"(?:[^\s@]+@)?sha256:([0-9a-f]{64})", value)
    if not match:
        raise argparse.ArgumentTypeError("image must be an actual immutable sha256 reference")
    return {"reference": value, "sha256": match[1]}


def engine_name(value):
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}", value) or value in {"host", "none"}:
        raise argparse.ArgumentTypeError("invalid fixed Engine volume/network name")
    return value


def https_origin(value):
    try:
        url = urlsplit(value)
        if (url.scheme != "https" or not url.hostname or url.username is not None
                or url.password is not None or url.path not in {"", "/"}
                or "?" in value or "#" in value or any(c.isspace() for c in value)
                or (url.port is not None and not 1 <= url.port <= 65535)):
            raise ValueError()
    except ValueError as error:
        raise argparse.ArgumentTypeError("CP URL must be an HTTPS origin without credentials/path/query") from error
    return value.rstrip("/") + "/"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--api-origin", type=https_origin, required=True)
    parser.add_argument("--engine-id", type=int, required=True)
    parser.add_argument("--cache-volume", type=engine_name, required=True)
    parser.add_argument("--workload-network", type=engine_name, required=True)
    parser.add_argument("--probe-image", type=pinned, required=True)
    parser.add_argument("--allow-image", type=pinned, action="append", default=[])
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.engine_id <= 0 or args.engine_id > 2**63 - 1:
        parser.error("engine ID must be a positive i64 allocated by CP")
    images = {image["reference"]: image for image in [args.probe_image, *args.allow_image]}
    config = {
        "api_origin": args.api_origin, "listen": "127.0.0.1:8092",
        "docker_socket": "/var/run/docker.sock", "engine_id": args.engine_id,
        "cache_volume": args.cache_volume, "workload_network": args.workload_network,
        "allowed_images": list(images.values()), "probe_image": args.probe_image,
        "memory_bytes": 512 * 1024 * 1024, "pids_limit": 128,
        "scratch_bytes": 128 * 1024 * 1024,
        "compressed_bytes": 128 * 1024 * 1024, "expanded_bytes": 512 * 1024 * 1024,
        "archive_files": 100000, "artifact_max_total_bytes": 256 * 1024 * 1024,
        "artifact_max_entries": 4096, "minimum_free_bytes": 1024 * 1024 * 1024,
        "max_active": 4, "max_retained": 16, "capture_bytes_per_stream": 4 * 1024 * 1024,
        "session_file": "/run/secrets/attune-controller-session", "development_plain_http": False,
    }
    data = (json.dumps(config, indent=2) + "\n").encode()
    # Never overwrite operator state or follow a preexisting output alias.
    descriptor = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())
    directory = os.open(args.output.resolve().parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


if __name__ == "__main__":
    main()
