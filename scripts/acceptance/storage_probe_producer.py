#!/usr/bin/env python3
"""Worker-side known-request file producer. No installer/provider credentials."""
import hashlib
import json
import os
from pathlib import Path
import sys
import time
import urllib.parse
import urllib.request


def api_origin(value):
    parsed = urllib.parse.urlsplit(value)
    if parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment or parsed.path not in ("", "/", "/api/v1", "/api/v1/"):
        raise RuntimeError("invalid worker API base")
    return urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, "", "", ""))


def main(run_id, hold_seconds):
    base = api_origin(os.environ["ATTUNE_API_URL"])
    token = os.environ["ATTUNE_API_TOKEN"]
    payload = ("F owned lifecycle " + run_id + "\n").encode() * 4096

    def request(path, body, method="POST", content_type="application/json"):
        data = json.dumps(body).encode() if content_type == "application/json" else body
        headers = {"Authorization": "Bearer " + token, "Content-Type": content_type,
                   "Content-Length": str(len(data))}
        if method == "PUT":
            headers["x-attune-sha256"] = hashlib.sha256(data).hexdigest()
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *args, **kwargs):
                return None
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect)
        with opener.open(urllib.request.Request(base + path, data, headers, method=method), timeout=90) as response:
            raw = response.read(1024 * 1024)
            return json.loads(raw) if raw else {}

    ref = "core.f_probe_" + run_id.replace("-", "_")
    version = request("/api/v1/artifacts/ref/" + ref + "/versions/file", {
        "scope": "action", "owner": "core.run_agent_command", "type": "file_text",
        "meta": {"f_probe_run": run_id},
    })["data"]
    # An actual run-owned staging file, not a synthetic cleanup status flag.
    root = Path(os.environ["ATTUNE_ARTIFACTS_DIR"]).resolve(strict=True)
    relative = Path(version["file_path"])
    if relative.is_absolute() or not relative.parts or any(p in (".", "..") for p in relative.parts):
        raise RuntimeError("unsafe API staging path")
    parent = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    for part in relative.parts[:-1]:
        try:
            os.mkdir(part, mode=0o700, dir_fd=parent)
        except FileExistsError:
            pass
        child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
        os.close(parent)
        parent = child
    fd = os.open(relative.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=parent)
    os.close(parent)
    with os.fdopen(fd, "wb") as stream:
        stream.write(payload)
        stream.flush()
        os.fsync(stream.fileno())
    request("/api/v1/internal/files/" + version["file_path"], payload, "PUT", "text/plain")
    print(json.dumps({"f_probe_run": run_id, "version": version["id"], "uploaded": True}), flush=True)
    # The parent driver cancels and independently observes native process exit.
    # Sleep expiry is never used as writer settlement proof.
    time.sleep(hold_seconds)


if __name__ == "__main__":
    main(sys.argv[1], int(sys.argv[2]))
