"""Shared safety boundaries for central acceptance fixtures, never onboarding bypasses."""
import json
import os
from pathlib import Path
import re
import signal
import stat
import sys
import urllib.error
import urllib.parse
import urllib.request


class FixtureError(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise FixtureError(message)


def context():
    require(sys.argv[1:] == ["--execute"], "Use --execute with one driver context JSON on stdin; no standalone implicit execution")
    value = json.loads(sys.stdin.read(1024 * 1024))
    require(re.fullmatch(r"[a-z][a-z0-9-]{0,19}", value["run_id"]), "Invalid run ID")
    require(value["project"].startswith("eaccept-" + value["run_id"] + "-"), "Foreign project")
    require(value["owner_label"] == "io.attune.acceptance.owner", "Unknown owner label")
    require(Path(value["compose_file"]).is_file(), "Missing private owned Compose file")
    return value


def read_secret(path):
    path = Path(path)
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor) as source:
        metadata = os.fstat(source.fileno())
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == os.getuid() and metadata.st_mode & 0o077 == 0 and metadata.st_size <= 16384, "Credential must be a small regular run-user-owned mode-0600 file")
        return source.read()


def write_private(path, value):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w") as target:
        target.write(value if isinstance(value, str) else json.dumps(value))


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None


class API:
    def __init__(self, base):
        parsed = urllib.parse.urlsplit(base)
        require(parsed.scheme == "http" and parsed.hostname == "127.0.0.1" and parsed.port and parsed.path in ("", "/") and not parsed.username and not parsed.query and not parsed.fragment, "Fixture requires the explicit owned loopback API URL")
        self.base = base.rstrip("/")
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def call(self, path, token=None, body=None, method="GET", expected=(200, 201)):
        require(path.startswith("/") and not path.startswith("//"), "Invalid API path")
        headers = {"Content-Type": "application/json"}
        if token:
            headers["Authorization"] = "Bearer " + token
        request = urllib.request.Request(self.base + path, headers=headers, data=json.dumps(body).encode() if body is not None else None, method=method)
        try:
            response = self.opener.open(request, timeout=15)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            require(response.code in expected, "API " + method + " " + path + " returned " + str(response.code) + "; check ordinary onboarding/permissions, never seed accounts")
            if response.code in (401, 403):
                return {"denied_status": response.code}
            return json.loads(response.read(4 * 1024 * 1024))


def run(main):
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    try:
        print(json.dumps(main(context())))
    except KeyboardInterrupt:
        print("Fixture interrupted; check driver cleanup evidence before removing any volume.", file=sys.stderr)
        sys.exit(130)
    except FixtureError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except Exception:
        print("Fixture failed; secret-bearing transport/response details suppressed. No onboarding fallback performed.", file=sys.stderr)
        sys.exit(1)
