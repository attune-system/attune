from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.time_series_point import TimeSeriesPoint


T = TypeVar("T", bound="GetEventVolumeAnalyticsResponse200Data")


@_attrs_define
class GetEventVolumeAnalyticsResponse200Data:
    """Response for event volume over time.

    Attributes:
        data (list[TimeSeriesPoint]): Data points: one per bucket (total events created)
        since (datetime.datetime): Time range start
        until (datetime.datetime): Time range end
    """

    data: list[TimeSeriesPoint]
    since: datetime.datetime
    until: datetime.datetime
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        data = []
        for data_item_data in self.data:
            data_item = data_item_data.to_dict()
            data.append(data_item)

        since = self.since.isoformat()

        until = self.until.isoformat()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "data": data,
                "since": since,
                "until": until,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.time_series_point import TimeSeriesPoint

        d = dict(src_dict)
        data = []
        _data = d.pop("data")
        for data_item_data in _data:
            data_item = TimeSeriesPoint.from_dict(data_item_data)

            data.append(data_item)

        since = datetime.datetime.fromisoformat(d.pop("since"))

        until = datetime.datetime.fromisoformat(d.pop("until"))

        get_event_volume_analytics_response_200_data = cls(
            data=data,
            since=since,
            until=until,
        )

        get_event_volume_analytics_response_200_data.additional_properties = d
        return get_event_volume_analytics_response_200_data

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
