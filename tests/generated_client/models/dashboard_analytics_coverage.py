from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.analytics_read_metadata import AnalyticsReadMetadata


T = TypeVar("T", bound="DashboardAnalyticsCoverage")


@_attrs_define
class DashboardAnalyticsCoverage:
    """
    Attributes:
        event_volume (AnalyticsReadMetadata): Coverage describes only this read's source-time bounds, including ledger
            holes.
        execution_status (AnalyticsReadMetadata): Coverage describes only this read's source-time bounds, including
            ledger holes.
        execution_throughput (AnalyticsReadMetadata): Coverage describes only this read's source-time bounds, including
            ledger holes.
        worker_status (AnalyticsReadMetadata): Coverage describes only this read's source-time bounds, including ledger
            holes.
    """

    event_volume: AnalyticsReadMetadata
    execution_status: AnalyticsReadMetadata
    execution_throughput: AnalyticsReadMetadata
    worker_status: AnalyticsReadMetadata
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        event_volume = self.event_volume.to_dict()

        execution_status = self.execution_status.to_dict()

        execution_throughput = self.execution_throughput.to_dict()

        worker_status = self.worker_status.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "event_volume": event_volume,
                "execution_status": execution_status,
                "execution_throughput": execution_throughput,
                "worker_status": worker_status,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.analytics_read_metadata import (
            AnalyticsReadMetadata,
        )

        d = dict(src_dict)
        event_volume = AnalyticsReadMetadata.from_dict(d.pop("event_volume"))

        execution_status = AnalyticsReadMetadata.from_dict(d.pop("execution_status"))

        execution_throughput = AnalyticsReadMetadata.from_dict(
            d.pop("execution_throughput")
        )

        worker_status = AnalyticsReadMetadata.from_dict(d.pop("worker_status"))

        dashboard_analytics_coverage = cls(
            event_volume=event_volume,
            execution_status=execution_status,
            execution_throughput=execution_throughput,
            worker_status=worker_status,
        )

        dashboard_analytics_coverage.additional_properties = d
        return dashboard_analytics_coverage

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
