"""Real workers must publish output to subscriptions opened before allocation."""

import os
import socket
import threading
import time
import uuid
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import psycopg
import pytest
import requests
import yaml

pytestmark = [pytest.mark.api, pytest.mark.integration, pytest.mark.timeout(180)]
TERMINAL = {"completed", "failed", "cancelled", "timeout", "abandoned"}

PRODUCER = '''import json, sys, time
from urllib.request import urlopen
parameters = json.load(sys.stdin)
deadline = time.monotonic() + 90
while time.monotonic() < deadline:
    with urlopen(parameters["gate_url"], timeout=10) as response:
        if response.read() == b"go":
            print("late-capture", flush=True)
            break
    time.sleep(0.05)
else:
    raise RuntimeError("subscription did not release its owned gate")
'''


@contextmanager
def _gate():
    ready = threading.Event()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            body = b"go" if ready.is_set() else b"wait"
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    class Server(ThreadingHTTPServer):
        daemon_threads = False

    # The E2E test container and workers share the runner-owned Docker network.
    server = Server(("0.0.0.0", 0), Handler)
    address = socket.gethostbyname(socket.gethostname())
    thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05})
    thread.start()
    try:
        yield f"http://{address}:{server.server_port}/gate", ready
    finally:
        ready.set()
        server.shutdown()
        server.server_close()
        thread.join(timeout=10)
        assert not thread.is_alive(), "owned gate server did not stop"


def _events(response):
    response.encoding = "utf-8"
    event, cursor, data = "message", None, []
    for line in response.iter_lines(chunk_size=1, decode_unicode=True):
        if not line:
            if data or event != "message" or cursor is not None:
                yield event, cursor, "\n".join(data)
            event, cursor, data = "message", None, []
        elif line.startswith("event:"):
            event = line[6:].strip()
        elif line.startswith("id:"):
            cursor = line[3:].strip()
        elif line.startswith("data:"):
            data.append(line[5:].lstrip())


def _terminal(session, base, execution):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        response = session.get(f"{base}/executions/{execution}", timeout=10)
        response.raise_for_status()
        result = response.json()["data"]
        if result["status"] in TERMINAL:
            return result
        time.sleep(0.05)
    raise AssertionError(f"execution {execution} did not settle; last status {result['status']}")


@pytest.mark.parametrize("sensitive", [False, True], ids=["plain", "secret"])
def test_worker_log_stream_discovered_after_subscription(client, tmp_path, sensitive):
    reference = "stream_probe_" + uuid.uuid4().hex[:10]
    pack = tmp_path / "pack"
    actions = pack / "actions"
    actions.mkdir(parents=True)
    (pack / "pack.yaml").write_text(yaml.safe_dump({
        "ref": reference, "name": "Owned stream discovery probe", "version": "1.0.0",
        "author": "Attune", "description": "Subscription-first capture regression",
    }))
    parameters = {"gate_url": {"type": "string", "required": True}}
    if sensitive:
        parameters["optional_secret"] = {"type": "string", "secret": True}
    (actions / "wait.py").write_text(PRODUCER)
    (actions / "wait.yaml").write_text(yaml.safe_dump({
        "ref": reference + ".wait", "label": "Wait for subscribed reader",
        "runner_type": "python", "entry_point": "wait.py", "parameters": parameters,
        "output_format": "text", "default_execution_permission_set_refs": [],
    }))
    client.upload_pack(str(pack))
    base = client.base_url + "/api/v1"
    execution, settled = None, False
    with _gate() as (gate_url, ready), requests.Session() as session:
        session.headers.update(client.session.headers)
        try:
            response = session.post(base + "/executions/execute", json={
                "action_ref": reference + ".wait", "parameters": {"gate_url": gate_url},
            }, timeout=30)
            response.raise_for_status()
            execution = response.json()["data"]["id"]
            url = f"{base}/executions/{execution}/logs/stdout/stream"
            with session.get(url, stream=True, timeout=(5, 30)) as response:
                response.raise_for_status()
                events = _events(response)
                event, _, data = next(events)
                assert event not in {"error", "done"} and "late-capture" not in data
                if os.getenv("ATTUNE_WORKER_BENCHMARK_EXPECT_LAZY", "1") == "1":
                    with psycopg.connect(os.environ["DATABASE_URL"]) as observer:
                        assert observer.execute("SELECT count(*) FROM artifact_version WHERE execution=%s", (execution,)).fetchone()[0] == 0
                ready.set()
                content, cursor, done = "", None, False
                for event, offset, data in events:
                    assert event != "error", data
                    if offset is not None:
                        cursor = offset
                    if "late-capture" in data:
                        content += data
                    if event == "done":
                        done = True
                        break
                assert done and content.count("late-capture") == 1
                assert cursor and cursor.isdigit() and int(cursor) > 0
            result = _terminal(session, base, execution)
            settled = True
            assert result["status"] == "completed", result
            with session.get(url, headers={"Last-Event-ID": cursor}, stream=True, timeout=(5, 20)) as response:
                response.raise_for_status()
                replay = list(_events(response))
                assert any(event == "done" for event, _, _ in replay)
                assert all(event != "error" and "late-capture" not in data for event, _, data in replay)
            with session.get(f"{base}/executions/{execution}/logs/stderr/stream", stream=True, timeout=(5, 20)) as response:
                response.raise_for_status()
                empty = list(_events(response))
                assert any(event == "done" for event, _, _ in empty)
                assert all(event != "error" for event, _, _ in empty)
        finally:
            ready.set()
            if execution is not None and not settled:
                _terminal(session, base, execution)
