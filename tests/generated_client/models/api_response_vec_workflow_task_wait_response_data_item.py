from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.workflow_task_wait_kind import WorkflowTaskWaitKind
from ..models.workflow_task_wait_state import WorkflowTaskWaitState
from ..types import UNSET, Unset

T = TypeVar("T", bound="ApiResponseVecWorkflowTaskWaitResponseDataItem")


@_attrs_define
class ApiResponseVecWorkflowTaskWaitResponseDataItem:
    """Safe operational metadata for one workflow task wait.

    Attributes:
        created (datetime.datetime):
        id (int):
        kind (WorkflowTaskWaitKind):
        state (WorkflowTaskWaitState):
        task_name (str):
        updated (datetime.datetime):
        resolved_at (datetime.datetime | None | Unset):
        target_id (int | None | Unset):
        work_queue_ref (None | str | Unset):
    """

    created: datetime.datetime
    id: int
    kind: WorkflowTaskWaitKind
    state: WorkflowTaskWaitState
    task_name: str
    updated: datetime.datetime
    resolved_at: datetime.datetime | None | Unset = UNSET
    target_id: int | None | Unset = UNSET
    work_queue_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        id = self.id

        kind = self.kind.value

        state = self.state.value

        task_name = self.task_name

        updated = self.updated.isoformat()

        resolved_at: None | str | Unset
        if isinstance(self.resolved_at, Unset):
            resolved_at = UNSET
        elif isinstance(self.resolved_at, datetime.datetime):
            resolved_at = self.resolved_at.isoformat()
        else:
            resolved_at = self.resolved_at

        target_id: int | None | Unset
        if isinstance(self.target_id, Unset):
            target_id = UNSET
        else:
            target_id = self.target_id

        work_queue_ref: None | str | Unset
        if isinstance(self.work_queue_ref, Unset):
            work_queue_ref = UNSET
        else:
            work_queue_ref = self.work_queue_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "id": id,
                "kind": kind,
                "state": state,
                "task_name": task_name,
                "updated": updated,
            }
        )
        if resolved_at is not UNSET:
            field_dict["resolved_at"] = resolved_at
        if target_id is not UNSET:
            field_dict["target_id"] = target_id
        if work_queue_ref is not UNSET:
            field_dict["work_queue_ref"] = work_queue_ref

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        kind = WorkflowTaskWaitKind(d.pop("kind"))

        state = WorkflowTaskWaitState(d.pop("state"))

        task_name = d.pop("task_name")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_resolved_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                resolved_at_type_0 = datetime.datetime.fromisoformat(data)

                return resolved_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        resolved_at = _parse_resolved_at(d.pop("resolved_at", UNSET))

        def _parse_target_id(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        target_id = _parse_target_id(d.pop("target_id", UNSET))

        def _parse_work_queue_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        work_queue_ref = _parse_work_queue_ref(d.pop("work_queue_ref", UNSET))

        api_response_vec_workflow_task_wait_response_data_item = cls(
            created=created,
            id=id,
            kind=kind,
            state=state,
            task_name=task_name,
            updated=updated,
            resolved_at=resolved_at,
            target_id=target_id,
            work_queue_ref=work_queue_ref,
        )

        api_response_vec_workflow_task_wait_response_data_item.additional_properties = d
        return api_response_vec_workflow_task_wait_response_data_item

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
