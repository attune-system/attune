from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
import datetime






T = TypeVar("T", bound="DashboardEffectiveTimeRange")



@_attrs_define
class DashboardEffectiveTimeRange:
    """ 
        Attributes:
            end (datetime.datetime):
            start (datetime.datetime):
            timezone (str):
     """

    end: datetime.datetime
    start: datetime.datetime
    timezone: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        end = self.end.isoformat()

        start = self.start.isoformat()

        timezone = self.timezone


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "end": end,
            "start": start,
            "timezone": timezone,
        })

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        end = datetime.datetime.fromisoformat(d.pop("end"))




        start = datetime.datetime.fromisoformat(d.pop("start"))




        timezone = d.pop("timezone")

        dashboard_effective_time_range = cls(
            end=end,
            start=start,
            timezone=timezone,
        )


        dashboard_effective_time_range.additional_properties = d
        return dashboard_effective_time_range

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
