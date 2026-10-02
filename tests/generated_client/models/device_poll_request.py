from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

T = TypeVar("T", bound="DevicePollRequest")


@_attrs_define
class DevicePollRequest:
    """
    Attributes:
        device_code (str):
    """

    device_code: str

    def to_dict(self) -> dict[str, Any]:
        device_code = self.device_code

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "device_code": device_code,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        device_code = d.pop("device_code")

        device_poll_request = cls(
            device_code=device_code,
        )

        return device_poll_request
