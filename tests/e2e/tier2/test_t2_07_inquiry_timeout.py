"""T2.7: inquiry timeout handling for guarded workflow tasks."""

import pytest

from helpers import (
    AttuneClient,
    start_inquiry_workflow,
    unique_ref,
    wait_for_execution_status,
    wait_for_inquiry_status,
    workflow_task_children,
)


pytestmark = [pytest.mark.tier2, pytest.mark.inquiry, pytest.mark.workflow]


def _result_data(execution: dict) -> dict:
    result = execution.get("result") or {}
    return result.get("data", result)


def test_inquiry_timeout_takes_timed_out_transition_without_guarded_child(
    client: AttuneClient, test_pack: dict
):
    run = start_inquiry_workflow(
        client,
        test_pack["ref"],
        purpose=f"timeout-{unique_ref()}",
        prompt="This inquiry should time out",
        timeout_seconds=2,
    )

    inquiry = wait_for_inquiry_status(
        client, run.inquiry["id"], "timeout", timeout=20
    )
    workflow = wait_for_execution_status(
        client, run.workflow_execution["id"], "completed", timeout=30
    )

    assert inquiry["response"] is None
    assert workflow_task_children(
        client, run.workflow_execution["id"], "guarded_task"
    ) == []
    timeout_handlers = workflow_task_children(
        client, run.workflow_execution["id"], "timeout_handler"
    )
    assert len(timeout_handlers) == 1
    assert timeout_handlers[0]["status"] == "completed"
    assert _result_data(workflow)["outcome"] == "timeout"


def test_response_before_timeout_releases_guarded_task(
    client: AttuneClient, test_pack: dict
):
    run = start_inquiry_workflow(
        client,
        test_pack["ref"],
        purpose=f"before-timeout-{unique_ref()}",
        prompt="Respond before this inquiry times out",
        timeout_seconds=10,
    )

    client.respond_to_inquiry(run.inquiry["id"], response={"approved": True})
    inquiry = wait_for_inquiry_status(client, run.inquiry["id"], "responded")
    workflow = wait_for_execution_status(
        client, run.workflow_execution["id"], "completed", timeout=30
    )

    assert inquiry["status"] == "responded"
    assert len(
        workflow_task_children(client, run.workflow_execution["id"], "guarded_task")
    ) == 1
    assert workflow_task_children(
        client, run.workflow_execution["id"], "timeout_handler"
    ) == []
    assert _result_data(workflow)["outcome"] == "approved"
