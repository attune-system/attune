#!/usr/bin/env python3
"""Package this exact source in the admitted probe action. Never print values."""
import hashlib
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request


def main():
    params = json.loads(sys.stdin.read(1024 * 1024))
    token = os.environ.get("ATTUNE_API_TOKEN", "")
    read_status = None
    runtime_value_digest = None
    if token:
        base = urllib.parse.urlsplit(os.environ["ATTUNE_API_URL"])
        if base.scheme not in ("http", "https") or base.username or base.query or base.fragment:
            raise ValueError("Invalid worker API origin")
        origin = urllib.parse.urlunsplit((base.scheme, base.netloc, "", "", ""))
        # The fixture gives the execution this one pack-local action read. Only the
        # status is retained, not the value, headers, request or token contents.
        reference = urllib.parse.quote(params.get("runtime_key_ref", params.get("runtime_action_ref", "")), safe="")
        path = "/api/v1/keys/" if "runtime_key_ref" in params else "/api/v1/actions/"
        request = urllib.request.Request(origin + path + reference,
                                         headers={"Authorization": "Bearer " + token})
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *args):
                return None
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        try:
            with opener.open(request, timeout=15) as response:
                read_status = response.status
                if "runtime_key_ref" in params:
                    returned = json.loads(response.read(1024 * 1024))["data"]["value"]
                    runtime_value_digest = hashlib.sha256(json.dumps(returned,sort_keys=True,separators=(",", ":"),ensure_ascii=False).encode()).hexdigest()
        except urllib.error.HTTPError as response:
            read_status = response.code
            response.close()
    receipt = {"protocol": "interop-v1", "nonce": params["nonce"],
               "api_token_present": bool(token), "runtime_read_status": read_status}
    if "input" in params:
        encoded = json.dumps(params["input"], sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
        receipt["input_sha256"] = hashlib.sha256(encoded).hexdigest()
    if runtime_value_digest is not None:
        receipt["runtime_value_sha256"] = runtime_value_digest
    print(json.dumps(receipt), flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        print("Owned interoperability worker probe failed; input suppressed", file=sys.stderr)
        sys.exit(1)
