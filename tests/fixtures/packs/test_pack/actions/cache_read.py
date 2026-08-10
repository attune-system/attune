#!/usr/bin/env python3
"""E2E action probe: cache values stay in the cache API response, never stdout."""

import json
import os
import sys
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen


def api_base() -> str:
    base = os.environ.get("ATTUNE_API_URL", "").rstrip("/")
    return base if base.endswith("/api/v1") else f"{base}/api/v1"


def main() -> None:
    request_document = json.load(sys.stdin)
    token = os.environ.get("ATTUNE_API_TOKEN")
    stdin_has_cache_payload = any(
        key in request_document for key in ("value", "values", "entries", "items")
    )
    if not token:
        print(
            json.dumps(
                {
                    "marker": "cache-token-missing",
                    "token_present": False,
                    "stdin_cache_payload": stdin_has_cache_payload,
                }
            )
        )
        return

    owner_ref = request_document.get("owner_ref") or os.environ["ATTUNE_PACK_REF"]
    owner_type = request_document.get("owner_type") or "pack"
    namespace = request_document["namespace"]
    payload = json.dumps(
        {
            "owner_type": owner_type,
            "owner_ref": owner_ref,
            "external_id": request_document["external_id"],
            "require_fresh": False,
        }
    ).encode()
    url = f"{api_base()}/cache/namespaces/{quote(namespace, safe='')}/entries/lookup"
    try:
        with urlopen(
            Request(
                url,
                method="POST",
                data=payload,
                headers={
                    "Authorization": f"Bearer {token}",
                    "Content-Type": "application/json",
                },
            ),
            timeout=15,
        ) as response:
            body = json.loads(response.read())
            data = body.get("data", body)
            print(
                json.dumps(
                    {
                        "marker": "cache-read-ok",
                        "generation_id": data.get(
                            "generation_id", data.get("generation")
                        ),
                        "stdin_cache_payload": stdin_has_cache_payload,
                    }
                )
            )
    except HTTPError as error:
        print(
            json.dumps(
                {
                    "marker": "cache-read-denied",
                    "status": error.code,
                    "stdin_cache_payload": stdin_has_cache_payload,
                }
            )
        )
    except URLError as error:
        print(
            json.dumps(
                {
                    "marker": "cache-read-transport-error",
                    "error_type": type(error).__name__,
                    "stdin_cache_payload": stdin_has_cache_payload,
                }
            )
        )


if __name__ == "__main__":
    main()
