"""Recipe execution across the public API, scheduler, worker, and sensor."""

import json
import os
from pathlib import Path
import uuid

import pytest
import yaml

from helpers.polling import wait_for_condition, wait_for_execution_status

pytestmark = [pytest.mark.api, pytest.mark.integration, pytest.mark.timeout(180),
              pytest.mark.skipif(os.getenv("ATTUNE_RUNTIME_SERVICE_SMOKE") != "1",
                                 reason="requires runner --runtime-lifecycle owned fixture admission")]


def _run(client, action, message, expected="completed"):
    execution = client.create_execution(action_ref=action, parameters={"message": message})
    return wait_for_execution_status(client, execution["id"], expected, timeout=90)


def test_recipe_released_and_inline_actions_and_preparation_timeout(client, pack_ref):
    released = _run(client, "core.lifecycle_echo", "released-contract")
    assert released["result"]["message"] == "released-contract"
    response = client._request("POST", "/api/v1/actions", json={
        "ref": pack_ref + ".recipe_inline_" + uuid.uuid4().hex[:8], "label": "Recipe inline fixture",
        "pack_ref": pack_ref, "runtime_ref": "core.lifecycle_fixture", "output_format": "json",
        "entrypoint": "import json,sys; print(json.dumps(json.load(sys.stdin)))",
        "param_schema": {"message": {"type": "string", "required": True}},
    })
    assert response.status_code in (200, 201), response.text
    action = response.json()["data"]
    try:
        inline = _run(client, action["ref"], "inline-contract")
        assert inline["result"]["message"] == "inline-contract"
    finally:
        client.delete_action(action["id"])
    timed_out = _run(client, "core.lifecycle_timeout", "must-not-launch", expected="timeout")
    assert "preparation" in timed_out["result"]["error"].lower()


def test_recipe_sensor_emits_a_scoped_event_and_runs_its_rule(client, pack_ref):
    rule = client.create_rule(pack_ref=pack_ref, name="recipe_sensor_" + uuid.uuid4().hex[:8],
        trigger_ref="core.lifecycle_tick", action_ref="core.lifecycle_echo",
        action_params={"message": "sensor-contract"})
    base = "/api/v1/sensors/core.lifecycle_sensor"
    response = client._request("POST", base + "/enable")
    assert response.status_code == 200, response.text
    try:
        wait_for_condition(lambda: any(event.get("payload", {}).get("message") == "sensor-contract"
            for event in client.list_events(trigger_ref="core.lifecycle_tick", rule_id=rule["id"])), timeout=90,
            error_message="Recipe sensor did not emit its admitted event")
        execution = None

        def received_execution():
            nonlocal execution
            execution = next((row for row in client.list_executions(action_ref="core.lifecycle_echo", limit=100)
                if row.get("rule_ref") == rule["ref"]), None)
            return execution is not None

        wait_for_condition(received_execution, timeout=90,
                           error_message="Recipe sensor event did not reach its action")
        result = wait_for_execution_status(client, execution["id"], "completed", timeout=90)
        assert result["result"]["message"] == "sensor-contract"
    finally:
        response = client._request("POST", base + "/disable")
        assert response.status_code == 200, response.text


@pytest.mark.parametrize("admitted", [True, False], ids=["admitted-worker", "unadmitted-recipe"])
def test_recipe_candidate_tests_use_an_admitted_worker_and_final_only_credentials(client, tmp_path, admitted):
    ref = "recipe_candidate_" + uuid.uuid4().hex[:10]
    root = tmp_path / ref
    (root / "tests").mkdir(parents=True)
    fixture = Path(__file__).resolve().parents[2] / "fixtures/runtime-services/fixture.json"
    recipe = json.loads(fixture.read_text())["execution_config"]["lifecycle"]
    if not admitted:
        recipe["env"]["APP_REGION"] = "not-admitted"
    manifest = {"ref": ref, "name": "Owned recipe candidate", "version": "1.0.0", "author": "Attune",
        "testing": {"enabled": True, "candidate_api_effects": True,
            "discovery": {"method": "explicit"}, "on_failure": "block", "min_pass_rate": 1.0,
            "runners": {"recipe": {"type": "runtime", "lifecycle": recipe,
                "entry_point": "tests/check.py", "timeout": 15, "result_format": "simple"}}}}
    (root / "pack.yaml").write_text(yaml.safe_dump(manifest))
    (root / "tests/check.py").write_text(
        "import os\nassert os.environ.get('ATTUNE_CANDIDATE_EFFECTS_TOKEN')\n"
        "assert 'ATTUNE_API_TOKEN' not in os.environ\n"
        "print('Total Tests: 1\\nPassed: 1\\nFailed: 0')\n")
    if admitted:
        pack = client.upload_pack(str(root), skip_tests=False)
        assert pack["ref"] == ref
    else:
        with pytest.raises(Exception) as error:
            client.upload_pack(str(root), skip_tests=False)
        assert "no active worker" in str(error.value).lower(), str(error.value)
        assert client.get_pack_by_ref(ref) is None
