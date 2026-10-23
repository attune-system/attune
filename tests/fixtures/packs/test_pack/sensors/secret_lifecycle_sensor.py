#!/usr/bin/env python3
"""Exercise only synthetic secrets delivered to a managed sensor."""

import json
import os
import sys
import time
from urllib.parse import quote
from urllib.request import Request, urlopen


def request(path, payload=None):
    base = os.environ["ATTUNE_API_URL"].rstrip("/")
    if not base.endswith("/api/v1"):
        base += "/api/v1"
    headers = {"Authorization": f"Bearer {os.environ['ATTUNE_API_TOKEN']}"}
    body = None
    if payload is not None:
        headers["Content-Type"] = "application/json"
        body = json.dumps(payload).encode()
    with urlopen(Request(base + path, data=body, headers=headers), timeout=15) as response:
        return json.load(response)["data"]


def log(marker, config, generation):
    line = json.dumps({"marker": marker, "config": config, "generation": generation})
    print(line, flush=True)
    print(line, file=sys.stderr, flush=True)


def run():
    bootstrap = json.load(sys.stdin)
    generation = int(os.environ["ATTUNE_SENSOR_ASSIGNMENT_GENERATION"])
    log("secret-bootstrap", bootstrap["config"], generation)
    for instance in bootstrap["triggers"]:
        if not instance["trigger_ref"].endswith(".secret_lifecycle_probe"):
            continue
        config = instance["config"]
        log("secret-initial-rule", config, generation)
        previous = None
        while True:
            rule = request("/rules/" + quote(instance["ref"], safe=""))
            config = rule["trigger_params"]
            if config != previous:
                if config.get("rotate"):
                    # Public padding crosses the owned probe stack's 4 KiB
                    # rotation threshold before the next synthetic value.
                    padding = "x" * 4096
                    print(padding, flush=True)
                    print(padding, file=sys.stderr, flush=True)
                log("secret-live-rule", config, generation)
                request("/events", {
                    "trigger_ref": instance["trigger_ref"],
                    "trigger_instance_id": f"rule_{rule['id']}",
                    "payload": {"credential": config["credential"], "generation": generation},
                    "config": {"credential": config["credential"]},
                })
                previous = config
            time.sleep(0.2)


if __name__ == "__main__":
    try:
        run()
    except Exception:
        print("secret-lifecycle-probe-failed", file=sys.stderr, flush=True)
        sys.exit(1)
