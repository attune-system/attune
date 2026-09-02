from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.workflow_cache_iteration_state import WorkflowCacheIterationState
from ..types import UNSET, Unset

T = TypeVar("T", bound="ListWorkflowCacheIterationsResponse200DataItem")


@_attrs_define
class ListWorkflowCacheIterationsResponse200DataItem:
    """Safe operational status for one workflow cache iteration.

    Attributes:
        batch_size (int):
        concurrency (int):
        created (datetime.datetime):
        dispatched_count (int):
        generation_id (int):
        namespace_id (int):
        page_size (int):
        scanned_count (int):
        state (WorkflowCacheIterationState):
        task_name (str):
        updated (datetime.datetime):
        completed_at (datetime.datetime | None | Unset):
        error_summary (None | str | Unset):
    """

    batch_size: int
    concurrency: int
    created: datetime.datetime
    dispatched_count: int
    generation_id: int
    namespace_id: int
    page_size: int
    scanned_count: int
    state: WorkflowCacheIterationState
    task_name: str
    updated: datetime.datetime
    completed_at: datetime.datetime | None | Unset = UNSET
    error_summary: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        batch_size = self.batch_size

        concurrency = self.concurrency

        created = self.created.isoformat()

        dispatched_count = self.dispatched_count

        generation_id = self.generation_id

        namespace_id = self.namespace_id

        page_size = self.page_size

        scanned_count = self.scanned_count

        state = self.state.value

        task_name = self.task_name

        updated = self.updated.isoformat()

        completed_at: None | str | Unset
        if isinstance(self.completed_at, Unset):
            completed_at = UNSET
        elif isinstance(self.completed_at, datetime.datetime):
            completed_at = self.completed_at.isoformat()
        else:
            completed_at = self.completed_at

        error_summary: None | str | Unset
        if isinstance(self.error_summary, Unset):
            error_summary = UNSET
        else:
            error_summary = self.error_summary

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "batch_size": batch_size,
                "concurrency": concurrency,
                "created": created,
                "dispatched_count": dispatched_count,
                "generation_id": generation_id,
                "namespace_id": namespace_id,
                "page_size": page_size,
                "scanned_count": scanned_count,
                "state": state,
                "task_name": task_name,
                "updated": updated,
            }
        )
        if completed_at is not UNSET:
            field_dict["completed_at"] = completed_at
        if error_summary is not UNSET:
            field_dict["error_summary"] = error_summary

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        batch_size = d.pop("batch_size")

        concurrency = d.pop("concurrency")

        created = datetime.datetime.fromisoformat(d.pop("created"))

        dispatched_count = d.pop("dispatched_count")

        generation_id = d.pop("generation_id")

        namespace_id = d.pop("namespace_id")

        page_size = d.pop("page_size")

        scanned_count = d.pop("scanned_count")

        state = WorkflowCacheIterationState(d.pop("state"))

        task_name = d.pop("task_name")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_completed_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                completed_at_type_0 = datetime.datetime.fromisoformat(data)

                return completed_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        completed_at = _parse_completed_at(d.pop("completed_at", UNSET))

        def _parse_error_summary(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        error_summary = _parse_error_summary(d.pop("error_summary", UNSET))

        list_workflow_cache_iterations_response_200_data_item = cls(
            batch_size=batch_size,
            concurrency=concurrency,
            created=created,
            dispatched_count=dispatched_count,
            generation_id=generation_id,
            namespace_id=namespace_id,
            page_size=page_size,
            scanned_count=scanned_count,
            state=state,
            task_name=task_name,
            updated=updated,
            completed_at=completed_at,
            error_summary=error_summary,
        )

        list_workflow_cache_iterations_response_200_data_item.additional_properties = d
        return list_workflow_cache_iterations_response_200_data_item

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
