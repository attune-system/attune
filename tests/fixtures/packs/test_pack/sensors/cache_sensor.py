#!/usr/bin/env python3
"""Managed-sensor cache probe used only by the cache E2E security scenario."""

import json
import os
import time
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen


def api_base() -> str:
    base = os.environ["ATTUNE_API_URL"].rstrip("/")
    return base if base.endswith("/api/v1") else f"{base}/api/v1"


def request_json(path: str, *, method: str, payload: dict) -> tuple[int, dict]:
    request = Request(
        f"{api_base()}{path}",
        method=method,
        data=json.dumps(payload).encode(),
        headers={
            "Authorization": f"Bearer {os.environ['ATTUNE_API_TOKEN']}",
            "Content-Type": "application/json",
        },
    )
    try:
        with urlopen(request, timeout=15) as response:
            body = json.loads(response.read())
            return response.status, body.get("data", body)
    except HTTPError as error:
        return error.code, {}
    except URLError:
        return 599, {}


def probe() -> None:
    instances = json.loads(os.environ.get("ATTUNE_SENSOR_TRIGGERS", "[]"))
    config = next(
        (
            item.get("config", {})
            for item in instances
            if item.get("trigger_ref", "").endswith(".cache_probe")
        ),
        {},
    )
    namespace = config.get("namespace")
    external_id = config.get("external_id")
    denied_owner_ref = config.get("denied_owner_ref")
    if not namespace or not external_id or not denied_owner_ref:
        print(json.dumps({"marker": "cache-sensor-missing-config"}), flush=True)
        return

    pack_ref = os.environ["ATTUNE_PACK_REF"]
    lookup_payload = {
        "owner_type": "pack",
        "owner_ref": pack_ref,
        "external_id": external_id,
        "require_fresh": False,
    }
    own_status, own_body = request_json(
        f"/cache/namespaces/{quote(namespace, safe='')}/entries/lookup",
        method="POST",
        payload=lookup_payload,
    )
    other_status, _ = request_json(
        f"/cache/namespaces/{quote(namespace, safe='')}/entries/lookup",
        method="POST",
        payload={**lookup_payload, "owner_ref": denied_owner_ref},
    )
    generation_id = own_body.get("generation_id", own_body.get("generation"))
    write_status, _ = request_json(
        f"/cache/namespaces/{quote(namespace, safe='')}/generations",
        method="POST",
        payload={
            "owner_type": "pack",
            "owner_ref": pack_ref,
            "client_refresh_id": f"sensor-write-probe-{int(time.time() * 1000)}",
            "expected_active_generation_id": generation_id,
            "expected_chunk_count": 1,
            "expected_record_count": 1,
        },
    )
    print(
        json.dumps(
            {
                "marker": "cache-sensor-probe",
                "read_status": own_status,
                "other_scope_status": other_status,
                "write_status": write_status,
                "generation_id": generation_id,
            }
        ),
        flush=True,
    )


if __name__ == "__main__":
    probe()
    while True:
        time.sleep(60)
