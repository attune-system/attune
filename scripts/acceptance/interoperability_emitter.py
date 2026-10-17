#!/usr/bin/env python3
"""Real execution-token event producer. Access principals never emit directly."""
import json
import os
import sys
import urllib.parse
import urllib.request


def main():
    params = json.loads(sys.stdin.read(1024 * 1024))
    origin = urllib.parse.urlsplit(os.environ["ATTUNE_API_URL"])
    if origin.scheme not in ("http", "https") or origin.username or origin.query or origin.fragment:
        raise ValueError("Invalid worker API origin")
    base = urllib.parse.urlunsplit((origin.scheme, origin.netloc, "", "", ""))
    token = os.environ["ATTUNE_API_TOKEN"]
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args):
            return None
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    body = {"trigger_ref":params["trigger_ref"], "payload":{"nonce":params["nonce"], "runtime_action_ref":params["runtime_action_ref"]}}
    request = urllib.request.Request(base + "/api/v1/events", json.dumps(body).encode(), {"Authorization":"Bearer " + token,"Content-Type":"application/json"}, method="POST")
    with opener.open(request, timeout=15) as response:
        if response.status != 201:
            raise ValueError("Real event emission did not return 201")
    print(json.dumps({"protocol":"interop-v1","nonce":params["nonce"],"api_token_present":bool(token),"runtime_read_status":201}), flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        print("Owned event producer failed; credentials and response suppressed", file=sys.stderr)
        sys.exit(1)
