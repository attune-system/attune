from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="UpdateKeyRequest")


@_attrs_define
class UpdateKeyRequest:
    """Request to update an existing key/secret

    Attributes:
        encrypted (bool | None | Unset): Update encryption status (re-encrypts if changing from false to true) Example:
            True.
        name (None | str | Unset): Update the human-readable name Example: GitHub API Token (Updated).
        value (Any | Unset): Update the secret value. Can be a string, object, array, number, or boolean.
    """

    encrypted: bool | None | Unset = UNSET
    name: None | str | Unset = UNSET
    value: Any | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        encrypted: bool | None | Unset
        if isinstance(self.encrypted, Unset):
            encrypted = UNSET
        else:
            encrypted = self.encrypted

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        value = self.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if encrypted is not UNSET:
            field_dict["encrypted"] = encrypted
        if name is not UNSET:
            field_dict["name"] = name
        if value is not UNSET:
            field_dict["value"] = value

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_encrypted(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        encrypted = _parse_encrypted(d.pop("encrypted", UNSET))

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        value = d.pop("value", UNSET)

        update_key_request = cls(
            encrypted=encrypted,
            name=name,
            value=value,
        )

        update_key_request.additional_properties = d
        return update_key_request

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
