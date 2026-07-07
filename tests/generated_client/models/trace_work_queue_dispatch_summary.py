from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.work_queue_dispatch_status import WorkQueueDispatchStatus
from typing import cast
import datetime






T = TypeVar("T", bound="TraceWorkQueueDispatchSummary")



@_attrs_define
class TraceWorkQueueDispatchSummary:
    """ 
        Attributes:
            created (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
            execution (int):
            id (int):
            leased_item_count (int):  Example: 5.
            queue (int):
            queue_ref (str):  Example: core.my_queue.
            status (WorkQueueDispatchStatus):
            updated (datetime.datetime):  Example: 2024-01-13T10:31:00Z.
     """

    created: datetime.datetime
    execution: int
    id: int
    leased_item_count: int
    queue: int
    queue_ref: str
    status: WorkQueueDispatchStatus
    updated: datetime.datetime
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        execution = self.execution

        id = self.id

        leased_item_count = self.leased_item_count

        queue = self.queue

        queue_ref = self.queue_ref

        status = self.status.value

        updated = self.updated.isoformat()


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "created": created,
            "execution": execution,
            "id": id,
            "leased_item_count": leased_item_count,
            "queue": queue,
            "queue_ref": queue_ref,
            "status": status,
            "updated": updated,
        })

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))




        execution = d.pop("execution")

        id = d.pop("id")

        leased_item_count = d.pop("leased_item_count")

        queue = d.pop("queue")

        queue_ref = d.pop("queue_ref")

        status = WorkQueueDispatchStatus(d.pop("status"))




        updated = datetime.datetime.fromisoformat(d.pop("updated"))




        trace_work_queue_dispatch_summary = cls(
            created=created,
            execution=execution,
            id=id,
            leased_item_count=leased_item_count,
            queue=queue,
            queue_ref=queue_ref,
            status=status,
            updated=updated,
        )


        trace_work_queue_dispatch_summary.additional_properties = d
        return trace_work_queue_dispatch_summary

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
