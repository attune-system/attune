from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.dashboard_freshness_mode import DashboardFreshnessMode
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.analytics_read_range import AnalyticsReadRange


T = TypeVar("T", bound="AnalyticsReadMetadata")


@_attrs_define
class AnalyticsReadMetadata:
    """Coverage describes only this read's source-time bounds, including ledger holes.

    Attributes:
        mode (DashboardFreshnessMode):
        raw_ranges (list[AnalyticsReadRange]):
        summary_ranges (list[AnalyticsReadRange]):
        oldest_refresh (datetime.datetime | None | Unset): Oldest refresh actually used. This does not guarantee global
            coverage.
    """

    mode: DashboardFreshnessMode
    raw_ranges: list[AnalyticsReadRange]
    summary_ranges: list[AnalyticsReadRange]
    oldest_refresh: datetime.datetime | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        mode = self.mode.value

        raw_ranges = []
        for raw_ranges_item_data in self.raw_ranges:
            raw_ranges_item = raw_ranges_item_data.to_dict()
            raw_ranges.append(raw_ranges_item)

        summary_ranges = []
        for summary_ranges_item_data in self.summary_ranges:
            summary_ranges_item = summary_ranges_item_data.to_dict()
            summary_ranges.append(summary_ranges_item)

        oldest_refresh: None | str | Unset
        if isinstance(self.oldest_refresh, Unset):
            oldest_refresh = UNSET
        elif isinstance(self.oldest_refresh, datetime.datetime):
            oldest_refresh = self.oldest_refresh.isoformat()
        else:
            oldest_refresh = self.oldest_refresh

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "mode": mode,
                "raw_ranges": raw_ranges,
                "summary_ranges": summary_ranges,
            }
        )
        if oldest_refresh is not UNSET:
            field_dict["oldest_refresh"] = oldest_refresh

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.analytics_read_range import AnalyticsReadRange

        d = dict(src_dict)
        mode = DashboardFreshnessMode(d.pop("mode"))

        raw_ranges = []
        _raw_ranges = d.pop("raw_ranges")
        for raw_ranges_item_data in _raw_ranges:
            raw_ranges_item = AnalyticsReadRange.from_dict(raw_ranges_item_data)

            raw_ranges.append(raw_ranges_item)

        summary_ranges = []
        _summary_ranges = d.pop("summary_ranges")
        for summary_ranges_item_data in _summary_ranges:
            summary_ranges_item = AnalyticsReadRange.from_dict(summary_ranges_item_data)

            summary_ranges.append(summary_ranges_item)

        def _parse_oldest_refresh(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                oldest_refresh_type_0 = datetime.datetime.fromisoformat(data)

                return oldest_refresh_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        oldest_refresh = _parse_oldest_refresh(d.pop("oldest_refresh", UNSET))

        analytics_read_metadata = cls(
            mode=mode,
            raw_ranges=raw_ranges,
            summary_ranges=summary_ranges,
            oldest_refresh=oldest_refresh,
        )

        analytics_read_metadata.additional_properties = d
        return analytics_read_metadata

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
