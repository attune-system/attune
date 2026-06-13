from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.integration_token_response import IntegrationTokenResponse


T = TypeVar("T", bound="CreateIntegrationTokenResponse201Data")


@_attrs_define
class CreateIntegrationTokenResponse201Data:
    """
    Attributes:
        integration_token (IntegrationTokenResponse):
        token (str):
    """

    integration_token: IntegrationTokenResponse
    token: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        integration_token = self.integration_token.to_dict()

        token = self.token

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "integration_token": integration_token,
                "token": token,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.integration_token_response import IntegrationTokenResponse

        d = dict(src_dict)
        integration_token = IntegrationTokenResponse.from_dict(
            d.pop("integration_token")
        )

        token = d.pop("token")

        create_integration_token_response_201_data = cls(
            integration_token=integration_token,
            token=token,
        )

        create_integration_token_response_201_data.additional_properties = d
        return create_integration_token_response_201_data

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
