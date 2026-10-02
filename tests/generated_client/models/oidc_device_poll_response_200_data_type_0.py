from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.device_wait_reason import DeviceWaitReason
from ..models.oidc_device_poll_response_200_data_type_0_status import (
    OidcDevicePollResponse200DataType0Status,
)

T = TypeVar("T", bound="OidcDevicePollResponse200DataType0")


@_attrs_define
class OidcDevicePollResponse200DataType0:
    """
    Attributes:
        device_code (str):
        interval (int):
        reason (DeviceWaitReason):
        status (OidcDevicePollResponse200DataType0Status):
    """

    device_code: str
    interval: int
    reason: DeviceWaitReason
    status: OidcDevicePollResponse200DataType0Status
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        device_code = self.device_code

        interval = self.interval

        reason = self.reason.value

        status = self.status.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_code": device_code,
                "interval": interval,
                "reason": reason,
                "status": status,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        device_code = d.pop("device_code")

        interval = d.pop("interval")

        reason = DeviceWaitReason(d.pop("reason"))

        status = OidcDevicePollResponse200DataType0Status(d.pop("status"))

        oidc_device_poll_response_200_data_type_0 = cls(
            device_code=device_code,
            interval=interval,
            reason=reason,
            status=status,
        )

        oidc_device_poll_response_200_data_type_0.additional_properties = d
        return oidc_device_poll_response_200_data_type_0

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
