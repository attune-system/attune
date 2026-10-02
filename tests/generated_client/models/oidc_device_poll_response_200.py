from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.oidc_device_poll_response_200_data_type_0 import (
        OidcDevicePollResponse200DataType0,
    )
    from ..models.oidc_device_poll_response_200_data_type_1 import (
        OidcDevicePollResponse200DataType1,
    )
    from ..models.oidc_device_poll_response_200_data_type_2 import (
        OidcDevicePollResponse200DataType2,
    )
    from ..models.oidc_device_poll_response_200_data_type_3 import (
        OidcDevicePollResponse200DataType3,
    )


T = TypeVar("T", bound="OidcDevicePollResponse200")


@_attrs_define
class OidcDevicePollResponse200:
    """Standard API response wrapper

    Attributes:
        data (OidcDevicePollResponse200DataType0 | OidcDevicePollResponse200DataType1 |
            OidcDevicePollResponse200DataType2 | OidcDevicePollResponse200DataType3):
        message (None | str | Unset): Optional message
    """

    data: (
        OidcDevicePollResponse200DataType0
        | OidcDevicePollResponse200DataType1
        | OidcDevicePollResponse200DataType2
        | OidcDevicePollResponse200DataType3
    )
    message: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.oidc_device_poll_response_200_data_type_0 import (
            OidcDevicePollResponse200DataType0,
        )
        from ..models.oidc_device_poll_response_200_data_type_1 import (
            OidcDevicePollResponse200DataType1,
        )
        from ..models.oidc_device_poll_response_200_data_type_2 import (
            OidcDevicePollResponse200DataType2,
        )

        data: dict[str, Any]
        if (
            isinstance(self.data, OidcDevicePollResponse200DataType0)
            or isinstance(self.data, OidcDevicePollResponse200DataType1)
            or isinstance(self.data, OidcDevicePollResponse200DataType2)
        ):
            data = self.data.to_dict()
        else:
            data = self.data.to_dict()

        message: None | str | Unset
        if isinstance(self.message, Unset):
            message = UNSET
        else:
            message = self.message

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "data": data,
            }
        )
        if message is not UNSET:
            field_dict["message"] = message

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.oidc_device_poll_response_200_data_type_0 import (
            OidcDevicePollResponse200DataType0,
        )
        from ..models.oidc_device_poll_response_200_data_type_1 import (
            OidcDevicePollResponse200DataType1,
        )
        from ..models.oidc_device_poll_response_200_data_type_2 import (
            OidcDevicePollResponse200DataType2,
        )
        from ..models.oidc_device_poll_response_200_data_type_3 import (
            OidcDevicePollResponse200DataType3,
        )

        d = dict(src_dict)

        def _parse_data(
            data: object,
        ) -> (
            OidcDevicePollResponse200DataType0
            | OidcDevicePollResponse200DataType1
            | OidcDevicePollResponse200DataType2
            | OidcDevicePollResponse200DataType3
        ):
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                data_type_0 = OidcDevicePollResponse200DataType0.from_dict(data)

                return data_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                data_type_1 = OidcDevicePollResponse200DataType1.from_dict(data)

                return data_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                data_type_2 = OidcDevicePollResponse200DataType2.from_dict(data)

                return data_type_2
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            data_type_3 = OidcDevicePollResponse200DataType3.from_dict(data)

            return data_type_3

        data = _parse_data(d.pop("data"))

        def _parse_message(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        message = _parse_message(d.pop("message", UNSET))

        oidc_device_poll_response_200 = cls(
            data=data,
            message=message,
        )

        oidc_device_poll_response_200.additional_properties = d
        return oidc_device_poll_response_200

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
