from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
  from ..models.event_summary import EventSummary
  from ..models.execution_summary import ExecutionSummary
  from ..models.trace_enforcement_summary import TraceEnforcementSummary
  from ..models.trace_work_queue_dispatch_summary import TraceWorkQueueDispatchSummary
  from ..models.work_queue_item_response import WorkQueueItemResponse





T = TypeVar("T", bound="ApiResponseTraceReportResponseData")



@_attrs_define
class ApiResponseTraceReportResponseData:
    """ 
        Attributes:
            enforcements (list[TraceEnforcementSummary]):
            events (list[EventSummary]):
            executions (list[ExecutionSummary]):
            origins (list[str]):  Example: ['event', 'work_queue_item'].
            queue_dispatches (list[TraceWorkQueueDispatchSummary]):
            queue_items (list[WorkQueueItemResponse]):
            trace_tag (str):  Example: core.timer.1234.
     """

    enforcements: list[TraceEnforcementSummary]
    events: list[EventSummary]
    executions: list[ExecutionSummary]
    origins: list[str]
    queue_dispatches: list[TraceWorkQueueDispatchSummary]
    queue_items: list[WorkQueueItemResponse]
    trace_tag: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        from ..models.event_summary import EventSummary
        from ..models.execution_summary import ExecutionSummary
        from ..models.trace_enforcement_summary import TraceEnforcementSummary
        from ..models.trace_work_queue_dispatch_summary import TraceWorkQueueDispatchSummary
        from ..models.work_queue_item_response import WorkQueueItemResponse
        enforcements = []
        for enforcements_item_data in self.enforcements:
            enforcements_item = enforcements_item_data.to_dict()
            enforcements.append(enforcements_item)



        events = []
        for events_item_data in self.events:
            events_item = events_item_data.to_dict()
            events.append(events_item)



        executions = []
        for executions_item_data in self.executions:
            executions_item = executions_item_data.to_dict()
            executions.append(executions_item)



        origins = self.origins



        queue_dispatches = []
        for queue_dispatches_item_data in self.queue_dispatches:
            queue_dispatches_item = queue_dispatches_item_data.to_dict()
            queue_dispatches.append(queue_dispatches_item)



        queue_items = []
        for queue_items_item_data in self.queue_items:
            queue_items_item = queue_items_item_data.to_dict()
            queue_items.append(queue_items_item)



        trace_tag = self.trace_tag


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "enforcements": enforcements,
            "events": events,
            "executions": executions,
            "origins": origins,
            "queue_dispatches": queue_dispatches,
            "queue_items": queue_items,
            "trace_tag": trace_tag,
        })

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.event_summary import EventSummary
        from ..models.execution_summary import ExecutionSummary
        from ..models.trace_enforcement_summary import TraceEnforcementSummary
        from ..models.trace_work_queue_dispatch_summary import TraceWorkQueueDispatchSummary
        from ..models.work_queue_item_response import WorkQueueItemResponse
        d = dict(src_dict)
        enforcements = []
        _enforcements = d.pop("enforcements")
        for enforcements_item_data in (_enforcements):
            enforcements_item = TraceEnforcementSummary.from_dict(enforcements_item_data)



            enforcements.append(enforcements_item)


        events = []
        _events = d.pop("events")
        for events_item_data in (_events):
            events_item = EventSummary.from_dict(events_item_data)



            events.append(events_item)


        executions = []
        _executions = d.pop("executions")
        for executions_item_data in (_executions):
            executions_item = ExecutionSummary.from_dict(executions_item_data)



            executions.append(executions_item)


        origins = cast(list[str], d.pop("origins"))


        queue_dispatches = []
        _queue_dispatches = d.pop("queue_dispatches")
        for queue_dispatches_item_data in (_queue_dispatches):
            queue_dispatches_item = TraceWorkQueueDispatchSummary.from_dict(queue_dispatches_item_data)



            queue_dispatches.append(queue_dispatches_item)


        queue_items = []
        _queue_items = d.pop("queue_items")
        for queue_items_item_data in (_queue_items):
            queue_items_item = WorkQueueItemResponse.from_dict(queue_items_item_data)



            queue_items.append(queue_items_item)


        trace_tag = d.pop("trace_tag")

        api_response_trace_report_response_data = cls(
            enforcements=enforcements,
            events=events,
            executions=executions,
            origins=origins,
            queue_dispatches=queue_dispatches,
            queue_items=queue_items,
            trace_tag=trace_tag,
        )


        api_response_trace_report_response_data.additional_properties = d
        return api_response_trace_report_response_data

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
