from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.failure_rate_response import FailureRateResponse
    from ..models.time_series_point import TimeSeriesPoint


T = TypeVar("T", bound="GetDashboardAnalyticsResponse200Data")


@_attrs_define
class GetDashboardAnalyticsResponse200Data:
    """Combined dashboard analytics response.

    Returns all key metrics in a single response for the dashboard page,
    avoiding multiple round-trips.

        Attributes:
            enforcement_volume (list[TimeSeriesPoint]): Enforcement volume per hour
            event_volume (list[TimeSeriesPoint]): Event volume per hour
            execution_status (list[TimeSeriesPoint]): Execution status transitions per hour
            execution_throughput (list[TimeSeriesPoint]): Execution throughput per hour
            failure_rate (FailureRateResponse): Response for the execution failure rate summary.
            since (datetime.datetime): Time range start
            until (datetime.datetime): Time range end
            worker_status (list[TimeSeriesPoint]): Worker status transitions per hour
    """

    enforcement_volume: list[TimeSeriesPoint]
    event_volume: list[TimeSeriesPoint]
    execution_status: list[TimeSeriesPoint]
    execution_throughput: list[TimeSeriesPoint]
    failure_rate: FailureRateResponse
    since: datetime.datetime
    until: datetime.datetime
    worker_status: list[TimeSeriesPoint]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        enforcement_volume = []
        for enforcement_volume_item_data in self.enforcement_volume:
            enforcement_volume_item = enforcement_volume_item_data.to_dict()
            enforcement_volume.append(enforcement_volume_item)

        event_volume = []
        for event_volume_item_data in self.event_volume:
            event_volume_item = event_volume_item_data.to_dict()
            event_volume.append(event_volume_item)

        execution_status = []
        for execution_status_item_data in self.execution_status:
            execution_status_item = execution_status_item_data.to_dict()
            execution_status.append(execution_status_item)

        execution_throughput = []
        for execution_throughput_item_data in self.execution_throughput:
            execution_throughput_item = execution_throughput_item_data.to_dict()
            execution_throughput.append(execution_throughput_item)

        failure_rate = self.failure_rate.to_dict()

        since = self.since.isoformat()

        until = self.until.isoformat()

        worker_status = []
        for worker_status_item_data in self.worker_status:
            worker_status_item = worker_status_item_data.to_dict()
            worker_status.append(worker_status_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "enforcement_volume": enforcement_volume,
                "event_volume": event_volume,
                "execution_status": execution_status,
                "execution_throughput": execution_throughput,
                "failure_rate": failure_rate,
                "since": since,
                "until": until,
                "worker_status": worker_status,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.failure_rate_response import FailureRateResponse
        from ..models.time_series_point import TimeSeriesPoint

        d = dict(src_dict)
        enforcement_volume = []
        _enforcement_volume = d.pop("enforcement_volume")
        for enforcement_volume_item_data in _enforcement_volume:
            enforcement_volume_item = TimeSeriesPoint.from_dict(
                enforcement_volume_item_data
            )

            enforcement_volume.append(enforcement_volume_item)

        event_volume = []
        _event_volume = d.pop("event_volume")
        for event_volume_item_data in _event_volume:
            event_volume_item = TimeSeriesPoint.from_dict(event_volume_item_data)

            event_volume.append(event_volume_item)

        execution_status = []
        _execution_status = d.pop("execution_status")
        for execution_status_item_data in _execution_status:
            execution_status_item = TimeSeriesPoint.from_dict(
                execution_status_item_data
            )

            execution_status.append(execution_status_item)

        execution_throughput = []
        _execution_throughput = d.pop("execution_throughput")
        for execution_throughput_item_data in _execution_throughput:
            execution_throughput_item = TimeSeriesPoint.from_dict(
                execution_throughput_item_data
            )

            execution_throughput.append(execution_throughput_item)

        failure_rate = FailureRateResponse.from_dict(d.pop("failure_rate"))

        since = datetime.datetime.fromisoformat(d.pop("since"))

        until = datetime.datetime.fromisoformat(d.pop("until"))

        worker_status = []
        _worker_status = d.pop("worker_status")
        for worker_status_item_data in _worker_status:
            worker_status_item = TimeSeriesPoint.from_dict(worker_status_item_data)

            worker_status.append(worker_status_item)

        get_dashboard_analytics_response_200_data = cls(
            enforcement_volume=enforcement_volume,
            event_volume=event_volume,
            execution_status=execution_status,
            execution_throughput=execution_throughput,
            failure_rate=failure_rate,
            since=since,
            until=until,
            worker_status=worker_status,
        )

        get_dashboard_analytics_response_200_data.additional_properties = d
        return get_dashboard_analytics_response_200_data

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
