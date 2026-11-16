"""Create one empty staging generation in this synthetic pack's cache."""

import json
import os
import sys
import urllib.error
import urllib.request
import uuid


def main():
    parameters = json.load(sys.stdin)
    namespace = parameters.get("namespace", "repro.users")
    if namespace not in ("repro.users", "cache_auth_repro.control"):
        raise ValueError("Only this fixture's namespaces are supported")
    request = urllib.request.Request(
        os.environ["ATTUNE_API_URL"].rstrip("/")
        + "/api/v1/cache/namespaces/"
        + namespace
        + "/generations",
        data=json.dumps(
            {
                "owner_type": "pack",
                "owner_ref": "cache_auth_repro",
                "client_refresh_id": "auth-repro-" + uuid.uuid4().hex,
                "expected_active_generation_id": None,
                "expected_chunk_count": 0,
                "expected_record_count": 0,
            }
        ).encode(),
        headers={
            "Authorization": "Bearer " + os.environ["ATTUNE_API_TOKEN"],
            "Content-Type": "application/json",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            status = response.status
    except urllib.error.HTTPError as error:
        status = error.code
    print(json.dumps({"namespace": namespace, "http_status": status}))
    return 0 if status == 201 else 1


if __name__ == "__main__":
    sys.exit(main())
