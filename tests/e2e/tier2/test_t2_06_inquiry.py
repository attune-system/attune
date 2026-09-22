"""T2.6: inquiry creation, response validation, and workflow release."""

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


def test_inquiry_response_releases_guarded_workflow_task(
    client: AttuneClient, test_pack: dict
):
    response = {"approved": True, "comment": "Approved by E2E"}
    run = start_inquiry_workflow(
        client,
        test_pack["ref"],
        purpose=f"approval-{unique_ref()}",
        prompt="Approve the E2E deployment?",
        response_schema={
            "approved": {"type": "boolean", "required": True},
            "comment": {"type": "string"},
        },
    )

    assert run.creator_execution["parent"] == run.workflow_execution["id"]
    assert run.inquiry["created_by_execution"] == run.creator_execution["id"]
    assert run.inquiry["workflow_execution"] is not None
    assert run.inquiry["workflow_task_name"] == "request_inquiry"
    assert run.inquiry["status"] == "pending"
    assert workflow_task_children(
        client, run.workflow_execution["id"], "guarded_task"
    ) == []

    client.respond_to_inquiry(run.inquiry["id"], response=response)
    inquiry = wait_for_inquiry_status(client, run.inquiry["id"], "responded")
    workflow = wait_for_execution_status(
        client, run.workflow_execution["id"], "completed", timeout=30
    )
    guarded = workflow_task_children(
        client, run.workflow_execution["id"], "guarded_task"
    )

    assert inquiry["response"] == response
    assert len(guarded) == 1
    assert guarded[0]["status"] == "completed"
    assert _result_data(workflow)["outcome"] == "approved"
    assert _result_data(workflow)["response"] == response
    assert len(
        workflow_task_children(
            client, run.workflow_execution["id"], "approved_handler"
        )
    ) == 1
    assert workflow_task_children(
        client, run.workflow_execution["id"], "denied_handler"
    ) == []


def test_inquiry_rejects_invalid_response_before_releasing_task(
    client: AttuneClient, test_pack: dict
):
    schema = {
        "approved": {"type": "boolean", "required": True},
        "priority": {
            "type": "string",
            "enum": ["low", "high"],
            "required": True,
        },
    }
    run = start_inquiry_workflow(
        client,
        test_pack["ref"],
        purpose=f"typed-response-{unique_ref()}",
        prompt="Choose an approval result and priority",
        response_schema=schema,
        response_options=[
            {
                "ref": "approve_high",
                "label": "Approve high priority",
                "style": "positive",
                "response": {"approved": True, "priority": "high"},
            }
        ],
    )

    with pytest.raises(Exception, match="Failed to respond to inquiry: 400"):
        client.respond_to_inquiry(
            run.inquiry["id"], response={"approved": "yes", "priority": "urgent"}
        )

    assert client.get_inquiry(run.inquiry["id"])["status"] == "pending"
    assert workflow_task_children(
        client, run.workflow_execution["id"], "guarded_task"
    ) == []

    response = {"approved": False, "priority": "low"}
    client.respond_to_inquiry(run.inquiry["id"], response=response)
    workflow = wait_for_execution_status(
        client, run.workflow_execution["id"], "completed", timeout=30
    )

    assert _result_data(workflow)["response"] == response
    assert _result_data(workflow)["outcome"] == "denied"
    assert workflow_task_children(
        client, run.workflow_execution["id"], "approved_handler"
    ) == []
    assert len(
        workflow_task_children(client, run.workflow_execution["id"], "denied_handler")
    ) == 1


def test_inquiry_list_filters_by_creator_execution(
    client: AttuneClient, test_pack: dict
):
    run = start_inquiry_workflow(
        client,
        test_pack["ref"],
        purpose=f"list-filter-{unique_ref()}",
        prompt="Find this inquiry by its creator execution",
    )

    inquiries = client.list_inquiries(
        created_by_execution=run.creator_execution["id"], limit=10
    )

    assert [inquiry["id"] for inquiry in inquiries] == [run.inquiry["id"]]
