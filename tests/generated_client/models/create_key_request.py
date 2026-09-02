from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.owner_type import OwnerType
from ..types import UNSET, Unset

T = TypeVar("T", bound="CreateKeyRequest")


@_attrs_define
class CreateKeyRequest:
    """Request to create a new key/secret

    Attributes:
        local_ref (str): Identifier within the selected owner scope. The server uses it to construct the canonical ref.
            Example: github_token.
        name (str): Human-readable name for the key Example: GitHub API Token.
        owner_type (OwnerType):
        value (Any): The secret value to store. Can be a string, object, array, number, or boolean.
        encrypted (bool | Unset): Whether to encrypt the value at rest (default: false; use --encrypt / -e from CLI)
            Example: False.
        owner_action_ref (None | str | Unset): Optional owner action reference Example: github.create_issue.
        owner_identity_login (None | str | Unset): Optional owner identity login Example: alice@example.com.
        owner_pack_ref (None | str | Unset): Optional owner pack reference Example: github.
        owner_sensor_ref (None | str | Unset): Optional owner sensor reference Example: github.webhook.
    """

    local_ref: str
    name: str
    owner_type: OwnerType
    value: Any
    encrypted: bool | Unset = UNSET
    owner_action_ref: None | str | Unset = UNSET
    owner_identity_login: None | str | Unset = UNSET
    owner_pack_ref: None | str | Unset = UNSET
    owner_sensor_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        local_ref = self.local_ref

        name = self.name

        owner_type = self.owner_type.value

        value = self.value

        encrypted = self.encrypted

        owner_action_ref: None | str | Unset
        if isinstance(self.owner_action_ref, Unset):
            owner_action_ref = UNSET
        else:
            owner_action_ref = self.owner_action_ref

        owner_identity_login: None | str | Unset
        if isinstance(self.owner_identity_login, Unset):
            owner_identity_login = UNSET
        else:
            owner_identity_login = self.owner_identity_login

        owner_pack_ref: None | str | Unset
        if isinstance(self.owner_pack_ref, Unset):
            owner_pack_ref = UNSET
        else:
            owner_pack_ref = self.owner_pack_ref

        owner_sensor_ref: None | str | Unset
        if isinstance(self.owner_sensor_ref, Unset):
            owner_sensor_ref = UNSET
        else:
            owner_sensor_ref = self.owner_sensor_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "local_ref": local_ref,
                "name": name,
                "owner_type": owner_type,
                "value": value,
            }
        )
        if encrypted is not UNSET:
            field_dict["encrypted"] = encrypted
        if owner_action_ref is not UNSET:
            field_dict["owner_action_ref"] = owner_action_ref
        if owner_identity_login is not UNSET:
            field_dict["owner_identity_login"] = owner_identity_login
        if owner_pack_ref is not UNSET:
            field_dict["owner_pack_ref"] = owner_pack_ref
        if owner_sensor_ref is not UNSET:
            field_dict["owner_sensor_ref"] = owner_sensor_ref

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        local_ref = d.pop("local_ref")

        name = d.pop("name")

        owner_type = OwnerType(d.pop("owner_type"))

        value = d.pop("value")

        encrypted = d.pop("encrypted", UNSET)

        def _parse_owner_action_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_action_ref = _parse_owner_action_ref(d.pop("owner_action_ref", UNSET))

        def _parse_owner_identity_login(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_identity_login = _parse_owner_identity_login(
            d.pop("owner_identity_login", UNSET)
        )

        def _parse_owner_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_pack_ref = _parse_owner_pack_ref(d.pop("owner_pack_ref", UNSET))

        def _parse_owner_sensor_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_sensor_ref = _parse_owner_sensor_ref(d.pop("owner_sensor_ref", UNSET))

        create_key_request = cls(
            local_ref=local_ref,
            name=name,
            owner_type=owner_type,
            value=value,
            encrypted=encrypted,
            owner_action_ref=owner_action_ref,
            owner_identity_login=owner_identity_login,
            owner_pack_ref=owner_pack_ref,
            owner_sensor_ref=owner_sensor_ref,
        )

        create_key_request.additional_properties = d
        return create_key_request

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
