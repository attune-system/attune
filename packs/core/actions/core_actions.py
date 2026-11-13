"""Core shell entrypoints: one flat JSON object on stdin, never physical lines."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlencode, urlsplit, urlunsplit


def text(params, key, default=""):
    value = params.get(key, default)
    if not isinstance(value, str) or "\0" in value:
        raise ValueError(f"{key} must be a string without NUL")
    return value


def integer(params, key, default, minimum, maximum):
    value = params.get(key, default)
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValueError(f"{key} must be an integer between {minimum} and {maximum}")
    return value


def boolean(params, key, default):
    value = params.get(key, default)
    if type(value) is not bool:
        raise ValueError(f"{key} must be a boolean")
    return value


def run_agent_command(params):
    command = text(params, "command")
    if not command:
        raise ValueError("command parameter is required")
    for name in ("ATTUNE_API_TOKEN", "ATTUNE_API_URL"):
        if not os.environ.get(name):
            raise ValueError(f"{name} is required for execution-scoped MCP access")
    working_dir = text(params, "working_dir")
    mcp_command = text(params, "mcp_command", "/opt/attune/agent/attune-mcp")
    state_dir = text(params, "state_dir")
    require_binary = boolean(params, "require_mcp_binary", True)
    if working_dir:
        os.chdir(working_dir)
    if not state_dir and os.environ.get("ATTUNE_ARTIFACTS_DIR") and os.environ.get("ATTUNE_EXEC_ID"):
        state_dir = str(Path(os.environ["ATTUNE_ARTIFACTS_DIR"]) / "agent" / os.environ["ATTUNE_EXEC_ID"])
    env = os.environ.copy()
    if state_dir:
        Path(state_dir).mkdir(parents=True, exist_ok=True)
        env["ATTUNE_AGENT_STATE_DIR"] = state_dir
    env["ATTUNE_MCP_COMMAND"] = mcp_command
    env["ATTUNE_MCP_TRANSPORT"] = "stdio"
    if require_binary and not (Path(mcp_command).is_file() and os.access(mcp_command, os.X_OK)):
        raise ValueError("attune-mcp binary is not executable")
    print("Running agent command with execution-scoped MCP access", file=sys.stderr)
    # This parameter intentionally contains shell code. Other fields stay data.
    os.execve("/bin/sh", ["/bin/sh", "-c", command], env)


def http_request(params):
    url = text(params, "url")
    if not url:
        raise ValueError("url parameter is required")
    query = params.get("query_params", {})
    if not isinstance(query, dict):
        raise ValueError("query_params must be an object")
    parts = urlsplit(url)
    if query:
        encoded = urlencode(query)
        url = urlunsplit(parts._replace(query=f"{parts.query}&{encoded}" if parts.query else encoded))
    method = text(params, "method", "GET").upper()
    timeout = integer(params, "timeout", 30, 1, 300)
    redirects = integer(params, "max_redirects", 10, 0, 1000)
    headers = params.get("headers", {})
    if not isinstance(headers, dict):
        raise ValueError("headers must be an object")
    args = ["curl", "--disable", "--silent", "--show-error", "--globoff",
            "--proto", "=http,https", "--proto-redir", "=http,https",
            "--request", method, "--max-time", str(timeout), "--connect-timeout", "10"]
    if not boolean(params, "verify_ssl", True):
        args.append("--insecure")
    if boolean(params, "follow_redirects", True):
        args.extend(["--location", "--max-redirs", str(redirects)])
    for name, value in headers.items():
        args.extend(["--header", f"{name}: {value}"])
    auth = text(params, "auth_type", "none")
    if auth == "basic":
        args.extend(["--user", text(params, "auth_username") + ":" + text(params, "auth_password")])
    elif auth == "bearer":
        args.extend(["--header", "Authorization: Bearer " + text(params, "auth_token")])
    elif auth != "none":
        raise ValueError("unsupported auth_type")
    with tempfile.TemporaryDirectory(prefix="attune-http-") as directory:
        root = Path(directory)
        if "json_body" in params:
            if not isinstance(params["json_body"], dict):
                raise ValueError("json_body must be an object")
            body = json.dumps(params["json_body"], ensure_ascii=False)
            args.extend(["--header", "Content-Type: application/json"])
        else:
            body = text(params, "body")
        if body or "body" in params or "json_body" in params:
            (root / "request").write_bytes(body.encode("utf-8"))
            args.extend(["--data-binary", "@" + str(root / "request")])
        args.extend(["--dump-header", str(root / "headers"), "--output", str(root / "response"),
                     "--write-out", "%{http_code}\n%{url_effective}", "--url", url])
        start = time.monotonic()
        completed = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
        elapsed = int((time.monotonic() - start) * 1000)
        metadata = completed.stdout.decode("utf-8", errors="replace").split("\n", 1)
        status = int(metadata[0]) if metadata[0].isdigit() else 0
        result = dict(status_code=status, headers={}, body="", json=None, elapsed_ms=elapsed,
                      url=metadata[1] if len(metadata) == 2 else url, success=False)
        if completed.returncode:
            # Do not copy curl diagnostics, which may contain credentials, into results.
            result["error"] = f"curl error code {completed.returncode}"
        else:
            result["body"] = (root / "response").read_bytes().decode("utf-8", errors="replace")
            for line in (root / "headers").read_text().splitlines():
                if line.startswith("HTTP/"):
                    result["headers"] = {}
                elif ":" in line:
                    name, value = line.split(":", 1)
                    result["headers"][name] = value.strip()
            try:
                result["json"] = json.loads(result["body"])
            except ValueError:
                pass
            result["success"] = 200 <= status < 300
        print(json.dumps(result, ensure_ascii=False))
        return 0 if result["success"] else 1


def main():
    action = sys.argv[1]
    try:
        params = json.load(sys.stdin)
        if not isinstance(params, dict):
            raise ValueError("parameters must be a JSON object")
        if action == "http_request":
            return http_request(params)
        if action == "run_agent_command":
            return run_agent_command(params)
        message = text(params, "message")
        if action == "echo":
            print(message, end="")
            return 0
        if action == "noop":
            code = integer(params, "exit_code", 0, 0, 255)
            if message:
                print("[NOOP] " + message)
            print("No operation completed successfully")
            return code
        if action == "sleep":
            seconds = integer(params, "seconds", 1, 0, 3600)
            if message:
                print(message, flush=True)
            time.sleep(seconds)
            print(f"Slept for {seconds} seconds")
            return 0
        raise ValueError("unknown action")
    except (ValueError, OSError) as error:
        # Input and OS errors must not echo parameter values or file paths.
        message = str(error) if isinstance(error, ValueError) and not isinstance(error, json.JSONDecodeError) else "invalid JSON parameters or action I/O failure"
        if action == "http_request":
            print(json.dumps(dict(status_code=0, headers={}, body="", json=None,
                                  elapsed_ms=0, url="", success=False, error=message)))
        else:
            print("ERROR: " + message, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
