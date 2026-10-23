"""Real pack uploads and the event -> rule -> worker boundary.

Entities use unique refs and live in the E2E runner's disposable database.
The runner owns teardown; this test never removes unowned packs or executions.
"""

import io
import json
import tarfile
import time
import uuid

import pytest
import requests


def _upload(client, reference, files):
    contents = {
        "pack.yaml": json.dumps(
            {"ref": reference, "label": reference, "version": "1.0.0"}
        ),
        **files,
    }
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for name, text in contents.items():
            body = text.encode()
            entry = tarfile.TarInfo(f"{reference}/{name}")
            entry.size = len(body)
            entry.mode = 0o755 if name.endswith(".sh") else 0o644
            archive.addfile(entry, io.BytesIO(body))
    return client.session.post(
        f"{client.base_url}/api/v1/packs/upload",
        files={"pack": (f"{reference}.tar.gz", output.getvalue(), "application/gzip")},
        data={"skip_tests": "true"},
        timeout=60,
    )


def _source_files(reference, target, *, secret=False):
    return {
        "triggers/tick.yaml": json.dumps(
            {
                "ref": f"{reference}.tick",
                "label": "Binding source",
                "output": {
                    "message": {"type": "string", "required": True, "secret": secret}
                },
            }
        ),
        "rules/forward.yaml": json.dumps(
            {
                "ref": f"{reference}.forward",
                "label": "Cross-pack binding",
                "trigger_ref": f"{reference}.tick",
                "action_ref": target,
                "enabled": True,
                "permission_set_refs": [],
                "action_params": {"message": "{{ event.payload.message }}"},
            }
        ),
    }


@pytest.mark.api
def test_uploaded_cross_pack_event_binds_exact_value_and_rejects_unsupported_sources(client):
    suffix = uuid.uuid4().hex[:12]
    sink = f"binding_sink_{suffix}"
    source = f"binding_source_{suffix}"
    files = {"actions/echo.sh": "#!/bin/sh\nset -eu\ncat\n"}
    for name, secret, visibility in [
        ("public", False, "public"),
        ("private", False, "private"),
        ("secret", True, "public"),
    ]:
        files[f"actions/{name}.yaml"] = json.dumps(
            {
                "ref": f"{sink}.{name}",
                "label": name,
                "runner_type": "shell",
                "entry_point": "echo.sh",
                "parameter_delivery": "stdin",
                "parameter_format": "json",
                "output_format": "json",
                "reference_visibility": visibility,
                "default_execution_permission_set_refs": [],
                "parameters": {
                    "message": {"type": "string", "required": True, "secret": secret}
                },
            }
        )
    response = _upload(client, sink, files)
    assert response.status_code == 201, response.text
    target = f"{sink}.public"
    response = _upload(client, source, _source_files(source, target))
    assert response.status_code == 201, response.text

    private_source = f"binding_private_{suffix}"
    response = _upload(
        client,
        private_source,
        _source_files(private_source, f"{sink}.private"),
    )
    assert 400 <= response.status_code < 500, "foreign private target was accepted"
    assert "because the action is private" in response.text.lower()

    # This is an explicitly unsupported authoring shape, not a successful
    # secret transfer or something that broader runtime grants can authorize.
    secret_source = f"binding_secret_{suffix}"
    response = _upload(
        client,
        secret_source,
        _source_files(secret_source, f"{sink}.secret", secret=True),
    )
    assert 400 <= response.status_code < 500, "direct secret event template was accepted"
    assert "key template" in response.text.lower()

    response = client.session.post(
        f"{client.base_url}/api/v1/triggers/{source}.tick/webhooks/enable",
        timeout=10,
    )
    response.raise_for_status()
    webhook = response.json()["data"]
    webhook_key = webhook.get("webhook_key") or webhook.get("key")
    assert webhook_key
    message = f"cross-pack-value-{suffix}"
    response = client.session.post(
        f"{client.base_url}/api/v1/webhooks/{webhook_key}",
        json={"payload": {"message": message}},
        timeout=10,
    )
    assert response.status_code in (200, 201, 202)

    deadline = time.monotonic() + 60
    execution_id = None
    execution = None
    while time.monotonic() < deadline:
        if execution_id is None:
            response = client.session.get(
                f"{client.base_url}/api/v1/executions",
                params={"action_ref": target, "per_page": 5},
                timeout=10,
            )
            response.raise_for_status()
            executions = response.json()["data"]
            if executions:
                assert len(executions) == 1, "one event must create one execution"
                execution_id = executions[0]["id"]
        else:
            response = client.session.get(
                f"{client.base_url}/api/v1/executions/{execution_id}", timeout=10
            )
            response.raise_for_status()
            execution = response.json()["data"]
            if execution["status"] in {"completed", "failed", "timeout", "cancelled"}:
                break
        time.sleep(0.25)
    assert execution is not None, "the uploaded cross-pack rule created no execution"
    assert execution["status"] == "completed", execution["status"]
    assert execution["config"] == {"message": message}
    assert execution["result"]["data"] == {"message": message}


@pytest.mark.api
@pytest.mark.parametrize("successor", [False, True], ids=["output", "successor"])
def test_uploaded_empty_iteration_preserves_result_and_completes(client, successor):
    reference = f"empty_iteration_{uuid.uuid4().hex[:12]}"
    tasks = [
        {
            "name": "each",
            "action": f"{reference}.echo",
            "with_items": "{{ parameters.items }}",
            "input": {"items": "{{ parameters.items }}"},
        }
    ]
    if successor:
        tasks[0]["next"] = [{"when": "{{ succeeded() }}", "do": ["after"]}]
        tasks.append(
            {
                "name": "after",
                "action": f"{reference}.echo",
                "input": {"items": "{{ parameters.items }}"},
            }
        )
    response = _upload(
        client,
        reference,
        {
            "actions/echo.sh": "#!/bin/sh\nset -eu\ncat\n",
            "actions/echo.yaml": json.dumps(
                {
                    "ref": f"{reference}.echo",
                    "runner_type": "shell",
                    "entry_point": "echo.sh",
                    "parameter_delivery": "stdin",
                    "parameter_format": "json",
                    "output_format": "json",
                    "parameters": {
                        "items": {"type": "array", "items": {"type": "string"}}
                    },
                }
            ),
            "actions/run.yaml": json.dumps(
                {
                    "ref": f"{reference}.run",
                    "workflow_file": "workflows/run.workflow.yaml",
                    "parameters": {
                        "items": {
                            "type": "array",
                            "items": {"type": "string"},
                            "required": True,
                        }
                    },
                }
            ),
            "actions/workflows/run.workflow.yaml": json.dumps(
                {
                    "version": "1.0.0",
                    "tasks": tasks,
                    "output_map": {"items": "{{ parameters.items }}"},
                }
            ),
        },
    )
    assert response.status_code == 201, response.text
    response = client.session.post(
        f"{client.base_url}/api/v1/executions/execute",
        json={"action_ref": f"{reference}.run", "parameters": {"items": []}},
        timeout=15,
    )
    assert response.status_code in (200, 201, 202), response.text
    execution_id = response.json()["data"]["id"]
    deadline = time.monotonic() + 60
    execution = None
    while time.monotonic() < deadline:
        response = client.session.get(
            f"{client.base_url}/api/v1/executions/{execution_id}", timeout=10
        )
        response.raise_for_status()
        execution = response.json()["data"]
        if execution["status"] in {"completed", "failed", "timeout", "cancelled"}:
            break
        time.sleep(0.25)
    assert execution["status"] == "completed", execution
    assert execution["result"] == {"$attune_secret": True, "redacted": True}
    response = client.session.get(
        f"{client.base_url}/api/v1/executions/{execution_id}",
        params={"include_secret_values": "true"},
        timeout=10,
    )
    response.raise_for_status()
    revealed = response.json()["data"]
    assert revealed["status"] == "completed", revealed["status"]
    assert revealed["result"].get("items") == [], revealed["result"]
    response = client.session.get(
        f"{client.base_url}/api/v1/executions",
        params={"parent": execution_id},
        timeout=10,
    )
    response.raise_for_status()
    children = response.json()["data"]
    assert not any((child.get("workflow_task") or {}).get("task_name") == "each" for child in children)
    after = [child for child in children if (child.get("workflow_task") or {}).get("task_name") == "after"]
    assert len(after) == int(successor), children
    if successor:
        assert after[0]["status"] == "completed", after[0]


@pytest.mark.api
@pytest.mark.parametrize("predecessor_condition", ["{{ timed_out() }}", None], ids=["condition", "unconditional"])
def test_uploaded_terminal_wait_without_recovery_settles_failed(client, predecessor_condition):
    reference = f"terminal_wait_{uuid.uuid4().hex[:12]}"
    key_ref = f"pack.{reference}.target"
    transition = {"do": ["guarded"]}
    if predecessor_condition is not None:
        transition["when"] = predecessor_condition
    response = _upload(
        client,
        reference,
        {
            "permission_sets/wait_reads.yaml": json.dumps(
                {
                    "ref": f"{reference}.wait_reads",
                    "grants": [
                        {
                            "resource": "executions",
                            "actions": ["read", "decrypt"],
                            "constraints": {"pack_refs": [reference]},
                        }
                    ],
                }
            ),
            "actions/request.sh": "#!/bin/sh\nsleep 60\n",
            "actions/request.yaml": json.dumps(
                {
                    "ref": f"{reference}.request",
                    "runner_type": "shell",
                    "entry_point": "request.sh",
                    "parameter_delivery": "stdin",
                    "parameter_format": "json",
                    "timeout_seconds": 15,
                }
            ),
            "actions/guarded.sh": "#!/bin/sh\nexit 99\n",
            "actions/guarded.yaml": json.dumps(
                {
                    "ref": f"{reference}.guarded",
                    "runner_type": "shell",
                    "entry_point": "guarded.sh",
                    "parameter_delivery": "stdin",
                    "parameter_format": "json",
                }
            ),
            "actions/run.yaml": json.dumps(
                {
                    "ref": f"{reference}.run",
                    "workflow_file": "workflows/run.workflow.yaml",
                }
            ),
            "actions/workflows/run.workflow.yaml": json.dumps(
                {
                    "version": "1.0.0",
                    "tasks": [
                        {
                            "name": "request",
                            "action": f"{reference}.request",
                            "next": [transition],
                        },
                        {
                            "name": "guarded",
                            "action": f"{reference}.guarded",
                            "permission_set_refs": [f"{reference}.wait_reads", "standard"],
                            "wait_for": {"execution": "{{ keystore['" + key_ref + "'] }}"},
                        }
                    ],
                }
            ),
        },
    )
    assert response.status_code == 201, response.text
    response = client.session.post(
        f"{client.base_url}/api/v1/keys",
        json={
            "local_ref": "target",
            "name": "Owned release wait target",
            "owner_type": "pack",
            "owner_pack_ref": reference,
            "value": 0,
            "encrypted": False,
        },
        timeout=15,
    )
    assert response.status_code == 201, response.text
    assert response.json()["data"]["ref"] == key_ref
    response = client.session.post(
        f"{client.base_url}/api/v1/executions/execute",
        json={
            "action_ref": f"{reference}.run",
            "parameters": {},
        },
        timeout=15,
    )
    assert response.status_code in (200, 201, 202), response.text
    execution_id = response.json()["data"]["id"]
    # Bind the genuine owned child's ID before its real runtime timeout. The
    # worker receives no API credential, and the test never edits runtime rows.
    source_deadline = time.monotonic() + 5
    request_id = None
    while time.monotonic() < source_deadline:
        response = client.session.get(
            f"{client.base_url}/api/v1/executions",
            params={"parent": execution_id},
            timeout=10,
        )
        response.raise_for_status()
        requests = [
            child for child in response.json()["data"]
            if child["action_ref"] == f"{reference}.request"
        ]
        if requests:
            assert len(requests) == 1
            request_id = requests[0]["id"]
            break
        time.sleep(0.1)
    assert request_id is not None, "the workflow created no owned timeout source"
    response = client.session.put(
        f"{client.base_url}/api/v1/keys/{key_ref}",
        json={"value": request_id},
        timeout=10,
    )
    assert response.status_code == 200, response.text
    deadline = time.monotonic() + 60
    execution = None
    while time.monotonic() < deadline:
        response = client.session.get(
            f"{client.base_url}/api/v1/executions/{execution_id}", timeout=10
        )
        response.raise_for_status()
        execution = response.json()["data"]
        if execution["status"] in {"completed", "failed", "timeout", "cancelled"}:
            break
        time.sleep(0.25)
    assert execution["status"] == "failed", execution
    response = client.session.get(
        f"{client.base_url}/api/v1/executions",
        params={"parent": execution_id},
        timeout=10,
    )
    response.raise_for_status()
    children = response.json()["data"]
    request_children = [child for child in children if child["action_ref"] == f"{reference}.request"]
    assert len(request_children) == 1
    assert request_children[0]["status"] == "timeout", request_children[0]
    assert not any(child["action_ref"] == f"{reference}.guarded" for child in children), (
        "a timed-out prerequisite must not run its guarded action"
    )
    response = client.session.get(
        f"{client.base_url}/api/v1/executions/{execution_id}/workflow-task-waits", timeout=10
    )
    response.raise_for_status()
    waits = response.json()["data"]
    assert len(waits) == 1, waits
    assert waits[0]["task_name"] == "guarded"
    assert waits[0]["state"] == "timed_out"
    assert waits[0]["target_id"] == request_id
    assert waits[0]["resolved_at"] is not None
    assert execution["result"].get("$attune_secret") is True
    key_grant = {"resource": "keys", "actions": ["read"], "constraints": {"refs": [key_ref]}}
    cases = [
        ("root_only", [{"resource": "executions", "actions": ["read", "decrypt"],
                         "constraints": {"refs": [f"{reference}.run"]}}], "redacted"),
        ("source_read_only", [
            {"resource": "executions", "actions": ["read", "decrypt"],
             "constraints": {"refs": [f"{reference}.run"]}},
            {"resource": "executions", "actions": ["read"],
             "constraints": {"refs": [f"{reference}.request"]}},
        ], "redacted"),
        ("source_only", [{"resource": "executions", "actions": ["read", "decrypt"],
                           "constraints": {"refs": [f"{reference}.request"]}}], "forbidden"),
        ("both", [{"resource": "executions", "actions": ["read", "decrypt"],
                    "constraints": {"refs": [f"{reference}.run", f"{reference}.request"]}}], "plaintext"),
    ]
    for label, grants, expected in cases:
        reader, assignment = _scoped_reader(client, label, [*grants, key_grant])
        try:
            response = reader.get(
                f"{client.base_url}/api/v1/executions/{execution_id}",
                params={"include_secret_values": "true"}, timeout=10,
            )
            if expected == "forbidden":
                assert response.status_code == 403, response.text
                continue
            assert response.status_code == 200, response.text
            value = response.json()["data"]["result"]
            if expected == "redacted":
                assert value.get("$attune_secret") is True, label
            else:
                assert value["succeeded"] is False
                assert "guarded" in value["error"]
                removed = client.session.delete(
                    f"{client.base_url}/api/v1/identities/roles/{assignment}", timeout=10,
                )
                assert removed.status_code in (200, 204), removed.text
                denied = reader.get(
                    f"{client.base_url}/api/v1/executions/{execution_id}",
                    params={"include_secret_values": "true"}, timeout=10,
                )
                assert denied.status_code == 403, denied.text
        finally:
            reader.close()


def _scoped_reader(client, label, grants):
    suffix = uuid.uuid4().hex[:12]
    login = f"wait_{label}_{suffix}@example.com"
    password = f"Disposable-{uuid.uuid4().hex}"
    registered = client.register(login=login, password=password, display_name="Wait reader")
    identity = registered["user"]["id"]
    response = client.session.post(
        f"{client.base_url}/api/v1/permissions/sets",
        json={"ref": f"wait_{suffix}.reader", "label": "Scoped Wait reader", "grants": grants},
        timeout=10,
    )
    assert response.status_code == 201, response.text
    permission_set = response.json()["data"]["id"]
    role = f"wait_reader_{suffix}"
    response = client.session.post(
        f"{client.base_url}/api/v1/permissions/sets/{permission_set}/roles",
        json={"role": role}, timeout=10,
    )
    assert response.status_code in (200, 201), response.text
    response = client.session.post(
        f"{client.base_url}/api/v1/identities/{identity}/roles", json={"role": role}, timeout=10,
    )
    assert response.status_code in (200, 201), response.text
    assignment = response.json()["data"]["id"]
    reader = requests.Session()
    try:
        login_response = reader.post(
            f"{client.base_url}/auth/login", json={"login": login, "password": password}, timeout=10,
        )
        assert login_response.status_code == 200, "Scoped reader login failed"
        reader.headers["Authorization"] = "Bearer " + login_response.json()["data"]["access_token"]
    except BaseException:
        reader.close()
        raise
    return reader, assignment
