"""Published releases retain native retry and queue completion authority."""

import socket
import threading
import uuid
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest
import yaml
from helpers.polling import TERMINAL_STATUSES, wait_for_condition, wait_for_execution_status

pytestmark = [pytest.mark.api, pytest.mark.integration, pytest.mark.timeout(180)]


@contextmanager
def _attempt_counter():
    attempts = []
    lock = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            with lock:
                attempts.append(len(attempts) + 1)
                body = str(attempts[-1]).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    class Server(ThreadingHTTPServer):
        daemon_threads = False

    server = Server(("0.0.0.0", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05})
    thread.start()
    try:
        address = socket.gethostbyname(socket.gethostname())
        yield f"http://{address}:{server.server_port}/attempt", attempts
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=10)
        assert not thread.is_alive(), "owned retry counter did not stop"


def _write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(yaml.safe_dump(value))


def _pack(tmp_path):
    ref = "native_completion_" + uuid.uuid4().hex[:10]
    path = tmp_path / ref
    _write(path / "pack.yaml", {
        "ref": ref, "name": "Owned native completion regression", "version": "1.0.0",
        "author": "Attune", "description": "Release-backed completion contracts",
    })
    return ref, path


def _settle(client, execution):
    return wait_for_execution_status(client, execution, list(TERMINAL_STATUSES), timeout=60, poll_interval=0.1)


@pytest.mark.parametrize("success_after,expected,attempt_count", [
    (3, "completed", 3), (9, "failed", 3), (1, "completed", 1),
], ids=["eventual-success", "exhaustion", "first-success"])
def test_worker_managed_retry_completion(client, tmp_path, success_after, expected, attempt_count):
    ref, path = _pack(tmp_path)
    parameters = {"counter_url": {"type": "string", "required": True},
                  "success_after": {"type": "integer", "required": True}}
    _write(path / "actions/attempt.yaml", {
        "ref": ref + ".attempt", "label": "Count one attempt", "runner_type": "python",
        "entry_point": "attempt.py", "parameters": parameters, "output_format": "json",
        "default_execution_permission_set_refs": [],
    })
    (path / "actions/attempt.py").write_text('''import json, sys
from urllib.request import Request, urlopen
parameters = json.load(sys.stdin)
with urlopen(Request(parameters["counter_url"], data=b"", method="POST"), timeout=10) as response:
    attempt = int(response.read())
print(json.dumps({"attempt": attempt}))
sys.exit(7 if attempt < parameters["success_after"] else 0)
''')
    _write(path / "actions/retry.yaml", {
        "ref": ref + ".retry", "label": "Retry a released action",
        "workflow_file": "workflows/retry.workflow.yaml", "parameters": parameters,
        "default_execution_permission_set_refs": [],
    })
    _write(path / "actions/workflows/retry.workflow.yaml", {
        "version": "1.0.0", "tasks": [{
            "name": "attempt", "action": ref + ".attempt",
            "input": {"counter_url": "{{ parameters.counter_url }}", "success_after": "{{ parameters.success_after }}"},
            "retry": {"count": 2, "delay": 1, "backoff": "constant"}, "next": [],
        }],
    })
    client.upload_pack(str(path))
    execution, settled = None, False
    with _attempt_counter() as (url, attempts):
        try:
            execution = client.create_execution(action_ref=ref + ".retry", parameters={
                "counter_url": url, "success_after": success_after,
            })["id"]
            result = _settle(client, execution)
            settled = True
            assert result["status"] == expected, result
            children = sorted((client.get_execution(row["id"]) for row in
                               client.list_executions(parent=execution, limit=100)), key=lambda row: row["id"])
            assert len(children) == attempt_count
            assert attempts == list(range(1, attempt_count + 1))
            assert [row["workflow_task"]["retry_count"] for row in children] == list(range(attempt_count))
            assert [row["status"] for row in children] == ["failed"] * (attempt_count - 1) + [expected]
            assert all(row["parent"] == execution and row["action_ref"] == ref + ".attempt" for row in children)
            assert all(row["original_execution"] == children[0]["id"] for row in children[1:])
            assert all(not row["result"].get("logs_incomplete", False) for row in children)
            assert all(row["result"]["runtime_log_capture"] == {"stdout": "captured", "stderr": "empty"} for row in children)
        finally:
            if execution is not None and not settled:
                client.cancel_execution(execution)
                _settle(client, execution)


def test_worker_managed_queue_completion(client, tmp_path):
    ref, path = _pack(tmp_path)
    _write(path / "actions/ack.yaml", {
        "ref": ref + ".ack", "label": "Acknowledge one leased item", "runner_type": "python",
        "entry_point": "ack.py", "output_format": "json", "default_execution_permission_set_refs": [],
        "parameters": {"queue_item": {"type": "object", "required": True}},
    })
    (path / "actions/ack.py").write_text('''import json, sys
item = json.load(sys.stdin)["queue_item"]["id"]
print(json.dumps({"queue_ack": {"version": 1, "items": [{"id": item, "status": "completed", "summary": {"source": "released-worker"}}]}}))
''')
    _write(path / "queues/work.yaml", {
        "ref": ref + ".work", "label": "Released work queue", "dispatch_action": ref + ".ack",
        "enabled": True, "accepting_new_items": True, "batch_mode": "single",
        "permission_set_refs": [], "action_params": {"queue_item": "{{ queue_item }}"},
        "config": {"ack_contract": {"version": 1}, "dispatch": {"retry_limit": 0}},
    })
    client.upload_pack(str(path))
    base = client.base_url + "/api/v1/queues/" + ref + ".work"
    response = client.session.get(base, timeout=10)
    response.raise_for_status()
    assert response.json()["data"]["is_adhoc"] is False
    response = client.session.post(base + "/items", json={"item_key": "owned-one", "payload": {}}, timeout=10)
    response.raise_for_status()
    item_id, item, settled = response.json()["data"]["id"], {}, False
    try:
        def terminal_item():
            nonlocal item
            response = client.session.get(base + f"/items/{item_id}", timeout=10)
            response.raise_for_status()
            item = response.json()["data"]
            return item["status"] in {"completed", "failed", "cancelled"}

        wait_for_condition(terminal_item, timeout=60, poll_interval=0.1,
                           error_message=f"Owned managed queue item {item_id} did not settle")
        settled = True
        assert item["status"] == "completed", item
        assert item["leased_execution"] is None and item["lease_token"] is None
        assert item["ack_summary"]["status"] == "completed"
        assert item["ack_summary"]["summary"]["source"] == "released-worker"
        rows = client.list_executions(action_ref=ref + ".ack", limit=100)
        assert len(rows) == 1
        execution = client.get_execution(rows[0]["id"])
        assert execution["status"] == "completed", execution
        assert execution["config"]["queue_item"]["id"] == item_id
        assert execution["result"]["queue_ack"]["items"][0]["id"] == item_id
        assert execution["result"]["runtime_log_capture"] == {"stdout": "captured", "stderr": "empty"}
        assert not execution["result"].get("logs_incomplete", False)
    finally:
        # Operational toggles stop this owned queue before any failure cleanup.
        response = client.session.put(base, json={"enabled": False, "accepting_new_items": False}, timeout=10)
        response.raise_for_status()
        if not settled and item.get("leased_execution"):
            client.cancel_execution(item["leased_execution"])
            _settle(client, item["leased_execution"])
