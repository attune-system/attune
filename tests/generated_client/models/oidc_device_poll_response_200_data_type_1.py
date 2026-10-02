from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.oidc_device_poll_response_200_data_type_1_status import (
    OidcDevicePollResponse200DataType1Status,
)

if TYPE_CHECKING:
    from ..models.token_response import TokenResponse


T = TypeVar("T", bound="OidcDevicePollResponse200DataType1")


@_attrs_define
class OidcDevicePollResponse200DataType1:
    """
    Attributes:
        status (OidcDevicePollResponse200DataType1Status):
        tokens (TokenResponse): Token response
    """

    status: OidcDevicePollResponse200DataType1Status
    tokens: TokenResponse
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        status = self.status.value

        tokens = self.tokens.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "status": status,
                "tokens": tokens,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.token_response import TokenResponse

        d = dict(src_dict)
        status = OidcDevicePollResponse200DataType1Status(d.pop("status"))

        tokens = TokenResponse.from_dict(d.pop("tokens"))

        oidc_device_poll_response_200_data_type_1 = cls(
            status=status,
            tokens=tokens,
        )

        oidc_device_poll_response_200_data_type_1.additional_properties = d
        return oidc_device_poll_response_200_data_type_1

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
