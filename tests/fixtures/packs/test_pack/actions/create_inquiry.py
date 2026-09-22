#!/usr/bin/env python3
"""Create an inquiry without exposing provider response handles."""

import json
import os
import sys
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


def api_base() -> str:
    base = os.environ.get("ATTUNE_API_URL", "").rstrip("/")
    return base if base.endswith("/api/v1") else f"{base}/api/v1"


def fail(message: str) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(1)


def main() -> None:
    params = json.load(sys.stdin)
    token = os.environ.get("ATTUNE_API_TOKEN")
    if not token:
        fail("ATTUNE_API_TOKEN is required")

    payload = {
        "purpose": params["purpose"],
        "prompt": params["prompt"],
        "response_options": params["response_options"],
    }
    for optional_field in ("response_schema", "timeout_seconds"):
        if params.get(optional_field) is not None:
            payload[optional_field] = params[optional_field]

    try:
        with urlopen(
            Request(
                f"{api_base()}/inquiries",
                method="POST",
                data=json.dumps(payload).encode(),
                headers={
                    "Authorization": f"Bearer {token}",
                    "Content-Type": "application/json",
                },
            ),
            timeout=15,
        ) as response:
            body = json.loads(response.read())
    except HTTPError as error:
        fail(f"Inquiry API returned HTTP {error.code}")
    except URLError as error:
        fail(f"Inquiry API request failed: {type(error).__name__}")

    data = body.get("data", body)
    inquiry = data.get("inquiry", data)
    print(json.dumps({"inquiry_id": inquiry["id"], "status": inquiry["status"]}))


if __name__ == "__main__":
    main()
