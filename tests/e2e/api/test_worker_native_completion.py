"""Published releases retain native retry and queue completion authority."""

import socket
import threading
import uuid
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest
import yaml
from helpers import AttuneClient
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


def _publish_single_item_queue(client, tmp_path, *, ack_status="completed", retry_limit=0):
    ref, path = _pack(tmp_path)
    _write(path / "actions/ack.yaml", {
        "ref": ref + ".ack", "label": "Acknowledge one leased item", "runner_type": "python",
        "entry_point": "ack.py", "output_format": "json", "default_execution_permission_set_refs": [],
        "parameters": {"queue_item": {"type": "object", "required": True}},
    })
    (path / "actions/ack.py").write_text(f"ACK_STATUS = {ack_status!r}\n" + '''import json, sys
item = json.load(sys.stdin)["queue_item"]
print(json.dumps({"queue_ack": {"version": 1, "items": [{
    "id": item["id"], "status": ACK_STATUS,
    "summary": {"source": "released-worker", "attempt": item["attempt_count"]},
}]}}))
''')
    _write(path / "queues/work.yaml", {
        "ref": ref + ".work", "label": "Released work queue", "dispatch_action": ref + ".ack",
        "enabled": True, "accepting_new_items": True, "batch_mode": "single",
        "permission_set_refs": [], "action_params": {"queue_item": "{{ queue_item }}"},
        "config": {"ack_contract": {"version": 1}, "dispatch": {"retry_limit": retry_limit}},
    })
    client.upload_pack(str(path))
    return ref


def test_worker_managed_queue_completion(client, tmp_path):
    ref = _publish_single_item_queue(client, tmp_path)
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


def test_managed_queue_retries_until_retained_limit(client, tmp_path):
    ref = _publish_single_item_queue(client, tmp_path, ack_status="retry", retry_limit=1)
    base = f"/api/v1/queues/{ref}.work"
    try:
        response = client.post(base + "/items", json={"payload": {}})
        response.raise_for_status()
        identifier = response.json()["data"]["id"]
        settled = {}

        def terminal():
            item = client.get(base + f"/items/{identifier}")
            item.raise_for_status()
            settled.update(item.json()["data"])
            return settled["status"] in {"completed", "failed", "cancelled"}

        wait_for_condition(terminal, timeout=60, poll_interval=0.1,
                           error_message="Managed queue item did not reach its retry limit")
        assert settled["status"] == "failed"
        assert settled["attempt_count"] == 2
        assert settled["lease_token"] is None and settled["leased_execution"] is None
        assert settled["last_error"]["code"] == "queue_dispatch_retry_limit_exhausted"
        executions = client.list_executions(action_ref=ref + ".ack", limit=20)
        assert len(executions) == 2
        attempts = [client.get_execution(execution["id"])["config"]["queue_item"]["attempt_count"]
                    for execution in executions]
        assert sorted(attempts) == [1, 2]
    finally:
        client.put(base, json={"enabled": False, "accepting_new_items": False}).raise_for_status()


def test_cross_pack_queue_batches_enqueue_only_contributors(client, tmp_path, api_base_url, test_timeout):
    provider, provider_path = _pack(tmp_path)
    owner, owner_path = _pack(tmp_path)
    _write(provider_path / "actions/ack.yaml", {
        "ref": provider + ".ack", "label": "Acknowledge business records",
        "runner_type": "python", "entry_point": "ack.py", "output_format": "json",
        "reference_visibility": "public", "default_execution_permission_set_refs": [],
        "parameters": {
            "records": {"type": "array", "items": {"type": "object"}, "required": True},
            "credential": {"type": "string", "secret": True, "required": True},
        },
        "out_schema": {"queue_ack": {"type": "object", "secret": True}},
    })
    (provider_path / "actions/ack.py").write_text('''import json, os, sys
assert "ATTUNE_API_TOKEN" not in os.environ
parameters = json.load(sys.stdin)
print(json.dumps({"queue_ack": {"version": 1, "items": [
    {"id": record["id"], "status": "completed",
     "summary": {"business": parameters["credential"], "message": record["payload"]["message"]}}
    for record in parameters["records"]
]}}))
''')
    client.upload_pack(str(provider_path))
    client.upload_pack(str(owner_path))
    credential, replacement = (f"owned-{uuid.uuid4().hex}" for _ in range(2))
    key = client.post("/api/v1/keys", json={
        "local_ref": "queue_credential", "owner_type": "pack", "owner_pack_ref": owner,
        "name": "Owned managed queue credential", "encrypted": True, "value": credential,
    })
    key.raise_for_status()
    key_ref = key.json()["data"]["ref"]
    queue_ref = owner + ".work"
    # Use a new release to publish the now-existing owner-qualified Key selector.
    pack_path = owner_path / "pack.yaml"
    pack = yaml.safe_load(pack_path.read_text())
    _write(pack_path, {**pack, "version": "1.0.1"})
    _write(owner_path / "queues/work.yaml", {
        "ref": queue_ref, "label": "Business-owned work", "dispatch_action": provider + ".ack",
        "enabled": False, "accepting_new_items": True, "batch_mode": "batch",
        "allow_pending_update": True, "update_strategy": "replace",
        "permission_set_refs": [],
        "item_schema": {"message": {"type": "string", "required": True}},
        "action_params": {"records": "{{ queue_items }}", "credential": "{{ keystore['" + key_ref + "'] }}"},
        "config": {"dispatch": {"batch_size": {"source": "literal", "value": 3}, "retry_limit": 0}},
    })
    _write(owner_path / "permission_sets/enqueue.yaml", {
        "ref": owner + ".enqueue",
        "grants": [{"resource": "queue_items", "actions": ["create"], "constraints": {"refs": [queue_ref]}}],
    })
    client.upload_pack(str(owner_path), force=True)
    base = f"/api/v1/queues/{queue_ref}"
    contributors, assignments, accepted = [], [], {}

    def require_redacted(text):
        if credential in text or replacement in text:
            pytest.fail("Managed queue credential appeared in a public projection")

    try:
        for _ in range(2):
            member = AttuneClient(base_url=api_base_url, timeout=test_timeout, auto_login=False)
            login, password = f"queue-{uuid.uuid4().hex}@attune.local", f"Test-{uuid.uuid4().hex}!"
            account = member.register(login=login, password=password, display_name="Queue contributor")
            member.login(login=login, password=password)
            contributors.append(member)
            assigned = client.post("/api/v1/permissions/assignments", json={
                "identity_id": account["user"]["id"], "permission_set_ref": owner + ".enqueue",
            })
            assigned.raise_for_status()
            assignments.append(assigned.json()["data"]["id"])
            key_attempt = member.get(f"/api/v1/keys/{key_ref}", params={"decrypt": "true"})
            assert key_attempt.status_code in (403, 404)
            require_redacted(key_attempt.text)

        single = contributors[0].post(base + "/items", json={
            "item_key": "first", "payload": {"message": "first"},
        })
        require_redacted(single.text)
        assert single.status_code == 201, single.json().get("error")
        first_id = single.json()["data"]["id"]
        assert single.json()["data"]["status"] == "queued"
        bulk = contributors[1].post(base + "/items/bulk", json={"items": [
            {"payload": {"message": message}} for message in ("second", "third")
        ]})
        require_redacted(bulk.text)
        assert bulk.status_code == 201, bulk.json().get("error")
        assert bulk.json()["data"]["created_count"] == 2
        accepted.update((item["id"], item["payload"]["message"]) for item in bulk.json()["data"]["items"])

        update = client.put(f"/api/v1/keys/{key_ref}", json={"value": replacement})
        update.raise_for_status()
        denied = contributors[0].put(base + f"/items/{first_id}", json={
            "payload": {"message": "unauthorized edit"},
        })
        assert denied.status_code == 403
        edited = client.put(base + f"/items/{first_id}", json={"payload": {"message": "edited"}})
        assert edited.status_code == 200, edited.json().get("error")
        upsert = client.post(base + "/items", json={
            "item_key": "first", "payload": {"message": "first-updated"},
        })
        assert upsert.status_code == 200, upsert.json().get("error")
        assert upsert.json()["data"]["id"] == first_id
        accepted[first_id] = "first-updated"
        started = client.put(base, json={"enabled": True})
        started.raise_for_status()

        items = []

        def settle_items():
            items.clear()
            for identifier in accepted:
                response = client.get(base + f"/items/{identifier}")
                response.raise_for_status()
                require_redacted(response.text)
                items.append(response.json()["data"])
            return all(item["status"] in {"completed", "failed", "cancelled"} for item in items)

        wait_for_condition(settle_items, timeout=60, poll_interval=0.1,
                           error_message="Business-owned managed items did not settle")
        assert all(item["status"] == "completed" and item["lease_token"] is None for item in items)
        rows = client.list_executions(action_ref=provider + ".ack", limit=100)
        assert len(rows) == 1
        execution = client.get_execution(rows[0]["id"])
        assert execution["status"] == "completed"
        require_redacted(str(execution))
        response = client.get(f"/api/v1/executions/{execution['id']}", params={"include_secret_values": "true"})
        response.raise_for_status()
        source = response.json()["data"]["result"]["queue_ack"]["items"]
        assert {item["id"]: item["summary"]["message"] for item in source} == accepted
        assert all(item["summary"]["business"] == credential for item in source)
    finally:
        disabled = client.put(base, json={"enabled": False, "accepting_new_items": False})
        disabled.raise_for_status()
        for assignment in assignments:
            client.delete(f"/api/v1/permissions/assignments/{assignment}").raise_for_status()
        for member in contributors:
            member.logout()


def test_managed_queue_native_workflow_acknowledges_item(client, tmp_path):
    ref, path = _pack(tmp_path)
    action = ref + ".acknowledge"
    _write(path / "actions/acknowledge.yaml", {
        "ref": action, "label": "Acknowledge in the workflow executor",
        "workflow_file": "workflows/acknowledge.workflow.yaml",
        "parameters": {"ack": {"type": "object", "required": True}},
        "out_schema": {"queue_ack": {"type": "object"}},
        "default_execution_permission_set_refs": [],
    })
    _write(path / "actions/validate.yaml", {
        "ref": ref + ".validate", "label": "Validate the item",
        "runner_type": "shell", "entry_point": "validate.sh",
        "default_execution_permission_set_refs": [],
    })
    (path / "actions/validate.sh").write_text("#!/bin/sh\nexit 0\n")
    _write(path / "actions/workflows/acknowledge.workflow.yaml", {
        "version": "1.0.0",
        "tasks": [{"name": "validate", "action": ref + ".validate", "next": []}],
        "output_map": {"queue_ack": "{{ parameters.ack }}"},
    })
    _write(path / "queues/work.yaml", {
        "ref": ref + ".work", "label": "Native completion work", "dispatch_action": action,
        "enabled": True, "accepting_new_items": True, "batch_mode": "single",
        "permission_set_refs": [], "action_params": {"ack": {"version": 1, "items": [{
            "id": "{{ queue_item.id }}", "status": "completed",
            "summary": {"source": "native-workflow"},
        }]}},
    })
    client.upload_pack(str(path))
    base = f"/api/v1/queues/{ref}.work"
    try:
        response = client.post(base + "/items", json={"payload": {}})
        response.raise_for_status()
        identifier = response.json()["data"]["id"]
        settled = {}

        def terminal():
            item = client.get(base + f"/items/{identifier}")
            item.raise_for_status()
            settled.update(item.json()["data"])
            return settled["status"] in {"completed", "failed", "cancelled"}

        wait_for_condition(terminal, timeout=60, poll_interval=0.1,
                           error_message="Native workflow did not settle its queue item")
        assert settled["status"] == "completed" and settled["lease_token"] is None
        assert settled["ack_summary"]["summary"]["source"] == "native-workflow"
        executions = client.list_executions(action_ref=action, limit=20)
        assert len(executions) == 1 and executions[0]["status"] == "completed"
        execution = client.get_execution(executions[0]["id"])
        assert execution["worker"] is None
        assert execution["result"]["queue_ack"]["items"][0]["id"] == identifier

        def workflow_log_ready():
            response = client.get(f"/api/v1/executions/{execution['id']}/artifacts", params={"type": "file_text"})
            response.raise_for_status()
            matches = [artifact for artifact in response.json()["items"] if artifact["ref"] == action + ".workflow.log"]
            if not matches:
                return False
            assert len(matches) == 1
            artifact = matches[0]
            version = artifact["selected_version"]
            assert version["execution"] == execution["id"]
            response = client.get(f"/api/v1/artifacts/{artifact['id']}/versions/{version['version']}/download",
                                  params={"version_id": version["id"]})
            assert response.status_code in (200, 404)
            return response.status_code == 200 and "Workflow Completed" in response.text

        wait_for_condition(workflow_log_ready, timeout=30, poll_interval=0.25,
                           error_message="Native workflow activity log did not finalize")
    finally:
        client.put(base, json={"enabled": False, "accepting_new_items": False}).raise_for_status()
