#!/usr/bin/env python3
"""Intentionally log a test Key to exercise managed log disclosure protection."""

import json
import os
import sys
import time
from urllib.parse import quote
from urllib.request import Request, urlopen


def probe():
    bootstrap = json.load(sys.stdin)
    config = next(
        instance["config"]
        for instance in bootstrap["triggers"]
        if instance["trigger_ref"].endswith(".sdk_log_probe")
    )
    base = os.environ["ATTUNE_API_URL"].rstrip("/")
    if not base.endswith("/api/v1"):
        base += "/api/v1"
    request = Request(
        f"{base}/keys/{quote(config['key_ref'], safe='')}",
        headers={"Authorization": f"Bearer {os.environ['ATTUNE_API_TOKEN']}"},
    )
    with urlopen(request, timeout=15) as response:
        value = json.load(response)["data"]["value"]
    # This fixture must never receive real credentials. Logging the synthetic
    # value is the behavior under test, not an example for production sensors.
    line = json.dumps({"marker": "sdk-key-log-probe", "value": value})
    print(line, flush=True)
    print(line, file=sys.stderr, flush=True)


if __name__ == "__main__":
    try:
        probe()
    except Exception:
        # Avoid exposing an HTTP response, token, or received value in diagnostics.
        print("sdk-key-log-probe-failed", file=sys.stderr, flush=True)
        sys.exit(1)
    while True:
        time.sleep(60)
