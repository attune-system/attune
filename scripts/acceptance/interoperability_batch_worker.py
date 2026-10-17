#!/usr/bin/env python3
"""Real batch action. Returns identities and hashes, never item payload values."""
import hashlib
import json
import os
import sys


def main():
    params = json.loads(sys.stdin.read(1024 * 1024))
    items = params["queue_items"]
    if not isinstance(items, list) or not 1 <= len(items) <= 2:
        raise ValueError("Batch size outside owned fixture")
    receipts = []
    for item in items:
        payload = item["payload"]
        encoded = json.dumps(payload["secret"], sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
        receipts.append({"id": item["id"], "nonce": payload["nonce"], "secret_sha256": hashlib.sha256(encoded).hexdigest()})
    print(json.dumps({"protocol":"interop-batch-v1", "api_token_present":bool(os.environ.get("ATTUNE_API_TOKEN")), "items":sorted(receipts, key=lambda item:item["id"])}), flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        print("Owned batch probe failed; payload suppressed", file=sys.stderr)
        sys.exit(1)
