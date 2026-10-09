from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.maintenance_schedule_status import MaintenanceScheduleStatus
    from ..models.partition_status import PartitionStatus
    from ..models.summary_status import SummaryStatus


T = TypeVar("T", bound="NativeMaintenanceStatus")


@_attrs_define
class NativeMaintenanceStatus:
    """
    Attributes:
        enabled (bool): Persisted native-maintenance switch, independent of row retention.
        observed_at (datetime.datetime): UTC time at which the API began collecting these observations.
        partitions (list[PartitionStatus]):
        schedule (list[MaintenanceScheduleStatus]):
        summaries (list[SummaryStatus]): Coverage extrema do not imply continuous coverage. Dirty hours use raw reads.
    """

    enabled: bool
    observed_at: datetime.datetime
    partitions: list[PartitionStatus]
    schedule: list[MaintenanceScheduleStatus]
    summaries: list[SummaryStatus]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        enabled = self.enabled

        observed_at = self.observed_at.isoformat()

        partitions = []
        for partitions_item_data in self.partitions:
            partitions_item = partitions_item_data.to_dict()
            partitions.append(partitions_item)

        schedule = []
        for schedule_item_data in self.schedule:
            schedule_item = schedule_item_data.to_dict()
            schedule.append(schedule_item)

        summaries = []
        for summaries_item_data in self.summaries:
            summaries_item = summaries_item_data.to_dict()
            summaries.append(summaries_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "enabled": enabled,
                "observed_at": observed_at,
                "partitions": partitions,
                "schedule": schedule,
                "summaries": summaries,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.maintenance_schedule_status import (
            MaintenanceScheduleStatus,
        )
        from ..models.partition_status import PartitionStatus
        from ..models.summary_status import SummaryStatus

        d = dict(src_dict)
        enabled = d.pop("enabled")

        observed_at = datetime.datetime.fromisoformat(d.pop("observed_at"))

        partitions = []
        _partitions = d.pop("partitions")
        for partitions_item_data in _partitions:
            partitions_item = PartitionStatus.from_dict(partitions_item_data)

            partitions.append(partitions_item)

        schedule = []
        _schedule = d.pop("schedule")
        for schedule_item_data in _schedule:
            schedule_item = MaintenanceScheduleStatus.from_dict(schedule_item_data)

            schedule.append(schedule_item)

        summaries = []
        _summaries = d.pop("summaries")
        for summaries_item_data in _summaries:
            summaries_item = SummaryStatus.from_dict(summaries_item_data)

            summaries.append(summaries_item)

        native_maintenance_status = cls(
            enabled=enabled,
            observed_at=observed_at,
            partitions=partitions,
            schedule=schedule,
            summaries=summaries,
        )

        native_maintenance_status.additional_properties = d
        return native_maintenance_status

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
