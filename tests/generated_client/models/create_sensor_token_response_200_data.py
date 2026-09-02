from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="CreateSensorTokenResponse200Data")


@_attrs_define
class CreateSensorTokenResponse200Data:
    """Response for sensor token creation

    Attributes:
        expires_at (str):
        identity_id (int):
        permission_set_refs (list[str]):
        sensor_ref (str):
        token (str):
        trigger_types (list[str]):
        pack_ref (None | str | Unset):
    """

    expires_at: str
    identity_id: int
    permission_set_refs: list[str]
    sensor_ref: str
    token: str
    trigger_types: list[str]
    pack_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        expires_at = self.expires_at

        identity_id = self.identity_id

        permission_set_refs = self.permission_set_refs

        sensor_ref = self.sensor_ref

        token = self.token

        trigger_types = self.trigger_types

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "expires_at": expires_at,
                "identity_id": identity_id,
                "permission_set_refs": permission_set_refs,
                "sensor_ref": sensor_ref,
                "token": token,
                "trigger_types": trigger_types,
            }
        )
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        expires_at = d.pop("expires_at")

        identity_id = d.pop("identity_id")

        permission_set_refs = cast(list[str], d.pop("permission_set_refs"))

        sensor_ref = d.pop("sensor_ref")

        token = d.pop("token")

        trigger_types = cast(list[str], d.pop("trigger_types"))

        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))

        create_sensor_token_response_200_data = cls(
            expires_at=expires_at,
            identity_id=identity_id,
            permission_set_refs=permission_set_refs,
            sensor_ref=sensor_ref,
            token=token,
            trigger_types=trigger_types,
            pack_ref=pack_ref,
        )

        create_sensor_token_response_200_data.additional_properties = d
        return create_sensor_token_response_200_data

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
