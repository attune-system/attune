from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset







T = TypeVar("T", bound="RateLimitPolicyRequest")



@_attrs_define
class RateLimitPolicyRequest:
    """ 
        Attributes:
            max_executions (int):  Example: 100.
            window_seconds (int):  Example: 3600.
     """

    max_executions: int
    window_seconds: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        max_executions = self.max_executions

        window_seconds = self.window_seconds


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "max_executions": max_executions,
            "window_seconds": window_seconds,
        })

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        max_executions = d.pop("max_executions")

        window_seconds = d.pop("window_seconds")

        rate_limit_policy_request = cls(
            max_executions=max_executions,
            window_seconds=window_seconds,
        )


        rate_limit_policy_request.additional_properties = d
        return rate_limit_policy_request

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
