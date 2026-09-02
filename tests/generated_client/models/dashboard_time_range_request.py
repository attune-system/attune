from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

T = TypeVar("T", bound="DashboardTimeRangeRequest")


@_attrs_define
class DashboardTimeRangeRequest:
    """
    Attributes:
        end (datetime.datetime):
        start (datetime.datetime):
    """

    end: datetime.datetime
    start: datetime.datetime

    def to_dict(self) -> dict[str, Any]:
        end = self.end.isoformat()

        start = self.start.isoformat()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "end": end,
                "start": start,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        end = datetime.datetime.fromisoformat(d.pop("end"))

        start = datetime.datetime.fromisoformat(d.pop("start"))

        dashboard_time_range_request = cls(
            end=end,
            start=start,
        )

        return dashboard_time_range_request
