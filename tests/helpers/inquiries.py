"""Production-path inquiry workflow helpers for E2E tests."""

from dataclasses import dataclass
from typing import Any

from .client_wrapper import AttuneClient
from .polling import wait_for_condition


@dataclass(frozen=True)
class InquiryRun:
    workflow_execution: dict[str, Any]
    creator_execution: dict[str, Any]
    inquiry: dict[str, Any]


def start_inquiry_workflow(
    client: AttuneClient,
    pack_ref: str,
    *,
    purpose: str,
    prompt: str,
    response_schema: dict[str, Any] | None = None,
    response_options: list[dict[str, Any]] | None = None,
    timeout_seconds: int = 300,
    timeout: float = 30.0,
) -> InquiryRun:
    """Start the fixture workflow and wait for its creator child and inquiry."""
    schema = (
        {"approved": {"type": "boolean", "required": True}}
        if response_schema is None
        else response_schema
    )
    options = response_options if response_options is not None else [
        {
            "ref": "approve",
            "label": "Approve",
            "style": "positive",
            "response": {"approved": True},
        },
        {
            "ref": "reject",
            "label": "Reject",
            "style": "destructive",
            "response": {"approved": False},
        },
    ]
    workflow = client.create_execution(
        action_ref=f"{pack_ref}.inquiry_workflow",
        parameters={
            "purpose": purpose,
            "prompt": prompt,
            "response_schema": schema,
            "response_options": options,
            "timeout_seconds": timeout_seconds,
        },
    )

    creator: dict[str, Any] | None = None

    def creator_exists() -> bool:
        nonlocal creator
        matches = [
            execution
            for execution in client.list_executions(parent=workflow["id"], limit=100)
            if (execution.get("workflow_task") or {}).get("task_name")
            == "request_inquiry"
        ]
        if len(matches) == 1:
            creator = matches[0]
            return True
        return False

    wait_for_condition(
        creator_exists,
        timeout=timeout,
        error_message=f"Workflow {workflow['id']} did not create its inquiry task",
    )
    assert creator is not None

    inquiry: dict[str, Any] | None = None

    def inquiry_exists() -> bool:
        nonlocal inquiry
        matches = client.list_inquiries(
            created_by_execution=creator["id"], status="pending", limit=10
        )
        if len(matches) == 1:
            inquiry = client.get_inquiry(matches[0]["id"])
            return True
        return False

    wait_for_condition(
        inquiry_exists,
        timeout=timeout,
        error_message=f"Execution {creator['id']} did not create one pending inquiry",
    )
    assert inquiry is not None
    return InquiryRun(workflow, creator, inquiry)


def workflow_task_children(
    client: AttuneClient, workflow_execution_id: int, task_name: str
) -> list[dict[str, Any]]:
    """Return child executions for one workflow task."""
    return [
        execution
        for execution in client.list_executions(
            parent=workflow_execution_id, limit=100
        )
        if (execution.get("workflow_task") or {}).get("task_name") == task_name
    ]
