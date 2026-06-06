"""T2.9: Workflow task execution timeout policy."""

import time

import pytest
from helpers import AttuneClient
from helpers.fixtures import unique_ref
from helpers.polling import wait_for_execution_status


def _create_shell_action(
    client: AttuneClient,
    pack_ref: str,
    name_prefix: str,
    entrypoint: str,
) -> dict:
    return client.create_action(
        pack_ref=pack_ref,
        data={
            "name": f"{name_prefix}_{unique_ref()}",
            "description": f"{name_prefix} timeout test action",
            "runtime_ref": "core.shell",
            "entrypoint": entrypoint,
            "enabled": True,
            "parameters": {},
        },
    )


def _child_for_action(client: AttuneClient, parent_id: int, action_ref: str) -> dict:
    children = client.list_executions(parent=parent_id, limit=100)
    matches = [child for child in children if child.get("action_ref") == action_ref]
    assert len(matches) == 1, f"Expected one child execution for {action_ref}"
    return matches[0]


def test_workflow_task_timeout_times_out_child_and_fails_parent(
    client: AttuneClient, test_pack
):
    """A workflow task timeout should terminate the child process promptly."""
    pack_ref = test_pack["ref"]
    slow_action = _create_shell_action(
        client,
        pack_ref,
        "timeout_slow",
        'echo "starting slow task"; sleep 20; echo "should not finish"',
    )
    workflow = client.create_workflow(
        pack_ref=pack_ref,
        name=f"task_timeout_{unique_ref()}",
        label="Task Timeout Workflow",
        tasks=[
            {
                "name": "slow",
                "action": slow_action["ref"],
                "input": {},
                "timeout": 2,
            }
        ],
    )

    start = time.time()
    execution = client.create_execution(action_ref=workflow["ref"], parameters={})
    parent = wait_for_execution_status(
        client=client,
        execution_id=execution["id"],
        expected_status="failed",
        timeout=12,
    )
    elapsed = time.time() - start
    child = _child_for_action(client, execution["id"], slow_action["ref"])
    child_details = client.get_execution(child["id"])
    result = child_details.get("result") or {}

    assert elapsed < 12, f"Timed-out workflow took too long: {elapsed:.1f}s"
    assert parent["status"] == "failed"
    assert child_details["status"] == "timeout"
    assert "timed out" in str(result).lower()


def test_execution_no_timeout_completes_normally(client: AttuneClient, test_pack):
    """A workflow task without an explicit timeout should complete normally."""
    pack_ref = test_pack["ref"]
    normal_action = _create_shell_action(
        client,
        pack_ref,
        "no_timeout",
        'echo "Action starting..."; sleep 3; echo "Action succeeded normally"',
    )
    workflow = client.create_workflow(
        pack_ref=pack_ref,
        name=f"no_timeout_{unique_ref()}",
        label="No Timeout Workflow",
        tasks=[{"name": "normal", "action": normal_action["ref"], "input": {}}],
    )

    start = time.time()
    execution = client.create_execution(action_ref=workflow["ref"], parameters={})
    result = wait_for_execution_status(
        client=client,
        execution_id=execution["id"],
        expected_status="completed",
        timeout=15,
    )
    elapsed = time.time() - start
    child = _child_for_action(client, execution["id"], normal_action["ref"])

    assert result["status"] == "completed"
    assert child["status"] == "completed"
    assert elapsed >= 3


def test_execution_timeout_vs_regular_failure(client: AttuneClient, test_pack):
    """Timeout failures should be distinguishable from ordinary exit failures."""
    pack_ref = test_pack["ref"]
    fail_action = _create_shell_action(
        client,
        pack_ref,
        "immediate_fail",
        'echo "Action failed intentionally" >&2; exit 1',
    )
    timeout_action = _create_shell_action(
        client,
        pack_ref,
        "timeout_fail",
        'echo "starting timeout task"; sleep 20; echo "should not finish"',
    )

    fail_workflow = client.create_workflow(
        pack_ref=pack_ref,
        name=f"regular_failure_{unique_ref()}",
        label="Regular Failure Workflow",
        tasks=[{"name": "fail", "action": fail_action["ref"], "input": {}}],
    )
    timeout_workflow = client.create_workflow(
        pack_ref=pack_ref,
        name=f"timeout_failure_{unique_ref()}",
        label="Timeout Failure Workflow",
        tasks=[
            {
                "name": "timeout",
                "action": timeout_action["ref"],
                "input": {},
                "timeout": 2,
            }
        ],
    )

    fail_execution = client.create_execution(
        action_ref=fail_workflow["ref"], parameters={}
    )
    timeout_execution = client.create_execution(
        action_ref=timeout_workflow["ref"], parameters={}
    )

    wait_for_execution_status(
        client=client,
        execution_id=fail_execution["id"],
        expected_status="failed",
        timeout=12,
    )
    wait_for_execution_status(
        client=client,
        execution_id=timeout_execution["id"],
        expected_status="failed",
        timeout=12,
    )

    fail_child = client.get_execution(
        _child_for_action(client, fail_execution["id"], fail_action["ref"])["id"]
    )
    timeout_child = client.get_execution(
        _child_for_action(client, timeout_execution["id"], timeout_action["ref"])["id"]
    )

    assert fail_child["status"] == "failed"
    assert timeout_child["status"] == "timeout"
    assert "timed out" not in str(fail_child.get("result")).lower()
    assert "timed out" in str(timeout_child.get("result")).lower()
