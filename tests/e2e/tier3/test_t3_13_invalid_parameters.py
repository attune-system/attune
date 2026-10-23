"""Parameter admission is strict; accepted flat values are delivered through stdin."""

import pytest

from helpers import AttuneClient, unique_ref
from helpers.polling import wait_for_execution_status

pytestmark = [pytest.mark.tier3, pytest.mark.validation, pytest.mark.parameters]


def create_action(client, test_pack, schema):
    return client.create_action({
        "ref": f"parameter_contract_{unique_ref()}",
        "label": "Parameter admission fixture",
        "pack_ref": test_pack["ref"],
        "runtime_ref": "core.python",
        "entrypoint": "parameter_contract_probe.py",
        "enabled": True,
        "param_schema": schema,
    })


def assert_rejected_without_execution(client, action_ref, parameters):
    before = client.get("/api/v1/executions", params={"action_ref": action_ref}).json()
    response = client.post("/api/v1/executions/execute", json={
        "action_ref": action_ref, "parameters": parameters,
    })
    assert response.status_code == 400
    assert response.json()["code"] == "BAD_REQUEST"
    assert "Invalid execution parameters" in response.json()["error"]
    after = client.get("/api/v1/executions", params={"action_ref": action_ref}).json()
    assert before["items"] == after["items"] == []


def execute_and_check(client, action_ref, supplied, expected):
    execution = client.execute_action({"action_ref": action_ref, "parameters": supplied})
    assert execution["config"] == expected
    completed = wait_for_execution_status(client=client, execution_id=execution["id"],
        expected_status="completed", timeout=30)
    assert completed["config"] == expected


def test_missing_required_parameter(client: AttuneClient, test_pack):
    action = create_action(client, test_pack, {
        "url": {"type": "string", "required": True},
        "timeout": {"type": "integer", "default": 30},
    })
    assert_rejected_without_execution(client, action["ref"], {"timeout": 60})


def test_invalid_parameter_type(client: AttuneClient, test_pack):
    action = create_action(client, test_pack, {
        "port": {"type": "integer", "required": True},
        "enabled": {"type": "boolean", "required": True},
    })
    assert_rejected_without_execution(client, action["ref"], {"port": "8080", "enabled": True})
    assert_rejected_without_execution(client, action["ref"], {"port": 8080, "enabled": "true"})
    execute_and_check(client, action["ref"], {"port": 8080, "enabled": True},
        {"port": 8080, "enabled": True})


def test_extra_parameters_ignored(client: AttuneClient, test_pack):
    # Flat documents permit additional fields unless their contract forbids them.
    # The action may ignore them, but admission must not silently discard them.
    action = create_action(client, test_pack, {"message": {"type": "string"}})
    values = {"message": "hello", "extra": {"count": 0}}
    execute_and_check(client, action["ref"], values, values)


def test_parameter_default_values(client: AttuneClient, test_pack):
    action = create_action(client, test_pack, {
        "message": {"type": "string", "default": "Hello from defaults"},
        "count": {"type": "integer", "default": 3},
        "debug": {"type": "boolean", "default": True},
    })
    execute_and_check(client, action["ref"], {},
        {"message": "Hello from defaults", "count": 3, "debug": True})
    explicit = {"message": "", "count": 0, "debug": False}
    execute_and_check(client, action["ref"], explicit, explicit)
