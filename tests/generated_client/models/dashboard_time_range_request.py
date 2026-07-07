from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
import datetime






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

        field_dict.update({
            "end": end,
            "start": start,
        })

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        end = datetime.datetime.fromisoformat(d.pop("end"))




        start = datetime.datetime.fromisoformat(d.pop("start"))




        dashboard_time_range_request = cls(
            end=end,
            start=start,
        )

        return dashboard_time_range_request

