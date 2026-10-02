from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="OidcDeviceStartResponse200Data")


@_attrs_define
class OidcDeviceStartResponse200Data:
    """Device-code instructions returned by Attune's OIDC broker.

    Attributes:
        device_code (str): Opaque, encrypted authorization session. Never display this value.
        expires_in (int):
        user_code (str):
        verification_uri (str):
        interval (int | Unset):
        verification_uri_complete (None | str | Unset):
    """

    device_code: str
    expires_in: int
    user_code: str
    verification_uri: str
    interval: int | Unset = UNSET
    verification_uri_complete: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        device_code = self.device_code

        expires_in = self.expires_in

        user_code = self.user_code

        verification_uri = self.verification_uri

        interval = self.interval

        verification_uri_complete: None | str | Unset
        if isinstance(self.verification_uri_complete, Unset):
            verification_uri_complete = UNSET
        else:
            verification_uri_complete = self.verification_uri_complete

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "device_code": device_code,
                "expires_in": expires_in,
                "user_code": user_code,
                "verification_uri": verification_uri,
            }
        )
        if interval is not UNSET:
            field_dict["interval"] = interval
        if verification_uri_complete is not UNSET:
            field_dict["verification_uri_complete"] = verification_uri_complete

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        device_code = d.pop("device_code")

        expires_in = d.pop("expires_in")

        user_code = d.pop("user_code")

        verification_uri = d.pop("verification_uri")

        interval = d.pop("interval", UNSET)

        def _parse_verification_uri_complete(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        verification_uri_complete = _parse_verification_uri_complete(
            d.pop("verification_uri_complete", UNSET)
        )

        oidc_device_start_response_200_data = cls(
            device_code=device_code,
            expires_in=expires_in,
            user_code=user_code,
            verification_uri=verification_uri,
            interval=interval,
            verification_uri_complete=verification_uri_complete,
        )

        oidc_device_start_response_200_data.additional_properties = d
        return oidc_device_start_response_200_data

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
