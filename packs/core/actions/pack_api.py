"""JSON transport for the four shipped pack API actions. No admission shortcuts."""

import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request


OPERATIONS = {
    "download_packs": ("download", {"packs": list, "destination_dir": str, "ref_spec": str,
                                  "timeout": int, "verify_ssl": bool}),
    "get_pack_dependencies": ("dependencies", {"pack_paths": list, "skip_validation": bool}),
    "build_pack_envs": ("build-envs", {"pack_paths": list, "packs_base_dir": str,
                                    "python_version": str, "nodejs_version": str,
                                    "skip_python": bool, "skip_nodejs": bool,
                                    "force_rebuild": bool, "timeout": int}),
    "register_packs": ("register-batch", {"pack_paths": list, "packs_base_dir": str,
                                        "skip_validation": bool, "skip_tests": bool, "force": bool}),
}
MAX_RESPONSE_BYTES = 16 * 1024 * 1024


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        return None


def call(operation, parameters):
    endpoint, fields = OPERATIONS[operation]
    if not isinstance(parameters, dict):
        raise ValueError("Parameters must be a JSON object")
    if set(parameters) - (set(fields) | {"api_url", "api_token"}):
        raise ValueError("Unknown pack API parameter")
    body = {}
    for name, kind in fields.items():
        if name not in parameters:
            continue
        value = parameters[name]
        if type(value) is not kind:
            raise ValueError(f"Invalid type for {name}")
        if kind is list and (not value or any(not isinstance(item, str) or not item for item in value)):
            raise ValueError(f"{name} must contain nonempty strings")
        if kind is int and not 1 <= value <= 7200:
            raise ValueError(f"Invalid range for {name}")
        if name == "ref_spec" and not value:
            continue
        body[name] = value
    required = "packs" if operation == "download_packs" else "pack_paths"
    if required not in body:
        raise ValueError(f"{required} is required")
    if operation == "download_packs" and not body.get("destination_dir"):
        raise ValueError("destination_dir is required")
    base = parameters.get("api_url") or os.environ.get("ATTUNE_API_URL", "http://localhost:8080")
    if not isinstance(base, str):
        raise ValueError("api_url must be a string")
    parsed = urllib.parse.urlsplit(base)
    if (parsed.scheme not in ("http", "https") or not parsed.netloc or parsed.username
            or parsed.password or parsed.query or parsed.fragment):
        raise ValueError("api_url must be an HTTP(S) base URL without credentials, query or fragment")
    supplied_token = parameters.get("api_token")
    token = supplied_token or os.environ.get("ATTUNE_API_TOKEN")
    if not isinstance(token, str) or not token or any(char.isspace() for char in token):
        raise ValueError("An existing bearer api_token or authorized execution token is required")
    if not supplied_token and base.rstrip("/") != os.environ.get("ATTUNE_API_URL", "http://localhost:8080").rstrip("/"):
        raise ValueError("Execution tokens may only be sent to their context API URL")
    request = urllib.request.Request(
        base.rstrip("/") + "/api/v1/packs/" + endpoint,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "Accept": "application/json",
                 "Authorization": "Bearer " + token},
        method="POST",
    )
    # Never forward a credential through an environment-selected proxy or redirect.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    started = time.monotonic()
    with opener.open(request, timeout=body.get("timeout", 300) + 30) as response:
        raw = response.read(MAX_RESPONSE_BYTES + 1)
    if len(raw) > MAX_RESPONSE_BYTES:
        raise ValueError("Pack API response exceeded the size limit")
    envelope = json.loads(raw)
    data = envelope.get("data") if isinstance(envelope, dict) else None
    if not isinstance(data, dict):
        raise ValueError("Pack API response must contain an object in data")
    expected = {
        "download_packs": {"downloaded_packs": list, "failed_packs": list, "success_count": int},
        "get_pack_dependencies": {"missing_dependencies": list, "errors": list},
        "build_pack_envs": {"summary": dict, "failed_environments": list, "detection_only": bool},
        "register_packs": {"summary": dict, "registered_packs": list, "failed_packs": list},
    }[operation]
    if any(type(data.get(name)) is not kind for name, kind in expected.items()):
        raise ValueError("Pack API response is missing typed result fields")
    if "summary" in data and any(type(data["summary"].get(name)) is not int
                                 for name in ("success_count", "failure_count")):
        raise ValueError("Pack API summary is missing typed counts")
    if operation == "download_packs":
        # Avoid unsupported expression-language map/filter syntax in workflows.
        data["pack_paths"] = [pack["pack_path"] for pack in data["downloaded_packs"]]
        if any(not isinstance(path, str) or not path for path in data["pack_paths"]):
            raise ValueError("Pack API returned an invalid pack path")
    data["duration_seconds"] = int(time.monotonic() - started)
    return data


def main(operation):
    try:
        result = call(operation, json.load(sys.stdin))
    except urllib.error.HTTPError as error:
        # An error body may echo secrets. Retain the status, never the body/token.
        print(f"Pack API rejected the request: HTTP {error.code}", file=sys.stderr)
        error.close()
        return 1
    except (ValueError, KeyError, TypeError, OSError):
        print("Pack API request failed. Check parameter types, API availability and ordinary authorization.", file=sys.stderr)
        return 1
    print(json.dumps(result, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
