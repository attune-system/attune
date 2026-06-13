from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.execution_status import ExecutionStatus
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.execution_summary_workflow_task_type_0 import (
        ExecutionSummaryWorkflowTaskType0,
    )


T = TypeVar("T", bound="ExecutionSummary")


@_attrs_define
class ExecutionSummary:
    """Simplified execution response (for list endpoints)

    Attributes:
        action_ref (str): Action reference Example: slack.post_message.
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        id (int): Execution ID Example: 1.
        status (ExecutionStatus):
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:35:00Z.
        enforcement (int | None | Unset): Enforcement ID Example: 1.
        original_execution (int | None | Unset): ID of the original execution if this execution is a retry. Example: 1.
        parent (int | None | Unset): Parent execution ID Example: 1.
        rule_ref (None | str | Unset): Rule reference (if triggered by a rule) Example: core.on_timer.
        started_at (datetime.datetime | None | Unset): When the execution actually started running (worker picked it
            up).
            Null if the execution hasn't started running yet. Example: 2024-01-13T10:31:00Z.
        timeout_seconds (int | None | Unset): Resolved execution timeout in seconds, snapshotted at creation time.
            Example: 600.
        trigger_ref (None | str | Unset): Trigger reference (if triggered by a trigger) Example: core.timer.
        workflow_task (ExecutionSummaryWorkflowTaskType0 | None | Unset): Workflow task metadata (only populated for
            workflow task executions)
    """

    action_ref: str
    created: datetime.datetime
    id: int
    status: ExecutionStatus
    updated: datetime.datetime
    enforcement: int | None | Unset = UNSET
    original_execution: int | None | Unset = UNSET
    parent: int | None | Unset = UNSET
    rule_ref: None | str | Unset = UNSET
    started_at: datetime.datetime | None | Unset = UNSET
    timeout_seconds: int | None | Unset = UNSET
    trigger_ref: None | str | Unset = UNSET
    workflow_task: ExecutionSummaryWorkflowTaskType0 | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.execution_summary_workflow_task_type_0 import (
            ExecutionSummaryWorkflowTaskType0,
        )

        action_ref = self.action_ref

        created = self.created.isoformat()

        id = self.id

        status = self.status.value

        updated = self.updated.isoformat()

        enforcement: int | None | Unset
        if isinstance(self.enforcement, Unset):
            enforcement = UNSET
        else:
            enforcement = self.enforcement

        original_execution: int | None | Unset
        if isinstance(self.original_execution, Unset):
            original_execution = UNSET
        else:
            original_execution = self.original_execution

        parent: int | None | Unset
        if isinstance(self.parent, Unset):
            parent = UNSET
        else:
            parent = self.parent

        rule_ref: None | str | Unset
        if isinstance(self.rule_ref, Unset):
            rule_ref = UNSET
        else:
            rule_ref = self.rule_ref

        started_at: None | str | Unset
        if isinstance(self.started_at, Unset):
            started_at = UNSET
        elif isinstance(self.started_at, datetime.datetime):
            started_at = self.started_at.isoformat()
        else:
            started_at = self.started_at

        timeout_seconds: int | None | Unset
        if isinstance(self.timeout_seconds, Unset):
            timeout_seconds = UNSET
        else:
            timeout_seconds = self.timeout_seconds

        trigger_ref: None | str | Unset
        if isinstance(self.trigger_ref, Unset):
            trigger_ref = UNSET
        else:
            trigger_ref = self.trigger_ref

        workflow_task: dict[str, Any] | None | Unset
        if isinstance(self.workflow_task, Unset):
            workflow_task = UNSET
        elif isinstance(self.workflow_task, ExecutionSummaryWorkflowTaskType0):
            workflow_task = self.workflow_task.to_dict()
        else:
            workflow_task = self.workflow_task

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "action_ref": action_ref,
                "created": created,
                "id": id,
                "status": status,
                "updated": updated,
            }
        )
        if enforcement is not UNSET:
            field_dict["enforcement"] = enforcement
        if original_execution is not UNSET:
            field_dict["original_execution"] = original_execution
        if parent is not UNSET:
            field_dict["parent"] = parent
        if rule_ref is not UNSET:
            field_dict["rule_ref"] = rule_ref
        if started_at is not UNSET:
            field_dict["started_at"] = started_at
        if timeout_seconds is not UNSET:
            field_dict["timeout_seconds"] = timeout_seconds
        if trigger_ref is not UNSET:
            field_dict["trigger_ref"] = trigger_ref
        if workflow_task is not UNSET:
            field_dict["workflow_task"] = workflow_task

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.execution_summary_workflow_task_type_0 import (
            ExecutionSummaryWorkflowTaskType0,
        )

        d = dict(src_dict)
        action_ref = d.pop("action_ref")

        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        status = ExecutionStatus(d.pop("status"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_enforcement(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        enforcement = _parse_enforcement(d.pop("enforcement", UNSET))

        def _parse_original_execution(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        original_execution = _parse_original_execution(
            d.pop("original_execution", UNSET)
        )

        def _parse_parent(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        parent = _parse_parent(d.pop("parent", UNSET))

        def _parse_rule_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        rule_ref = _parse_rule_ref(d.pop("rule_ref", UNSET))

        def _parse_started_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                started_at_type_0 = datetime.datetime.fromisoformat(data)

                return started_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        started_at = _parse_started_at(d.pop("started_at", UNSET))

        def _parse_timeout_seconds(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        timeout_seconds = _parse_timeout_seconds(d.pop("timeout_seconds", UNSET))

        def _parse_trigger_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        trigger_ref = _parse_trigger_ref(d.pop("trigger_ref", UNSET))

        def _parse_workflow_task(
            data: object,
        ) -> ExecutionSummaryWorkflowTaskType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                workflow_task_type_0 = ExecutionSummaryWorkflowTaskType0.from_dict(data)

                return workflow_task_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ExecutionSummaryWorkflowTaskType0 | None | Unset, data)

        workflow_task = _parse_workflow_task(d.pop("workflow_task", UNSET))

        execution_summary = cls(
            action_ref=action_ref,
            created=created,
            id=id,
            status=status,
            updated=updated,
            enforcement=enforcement,
            original_execution=original_execution,
            parent=parent,
            rule_ref=rule_ref,
            started_at=started_at,
            timeout_seconds=timeout_seconds,
            trigger_ref=trigger_ref,
            workflow_task=workflow_task,
        )

        execution_summary.additional_properties = d
        return execution_summary

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
