from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="UpdateKeyRequest")


@_attrs_define
class UpdateKeyRequest:
    """Request to update an existing key/secret

    Attributes:
        encrypted (bool | None | Unset): Update encryption status (re-encrypts if changing from false to true) Example:
            True.
        name (None | str | Unset): Update the human-readable name Example: GitHub API Token (Updated).
        rebind_schema (bool | Unset): Explicitly select current installed named revisions and revalidate.
        schema_digest (None | str | Unset):
        value (Any | Unset): Update the secret value. Can be a string, object, array, number, or boolean.
        value_schema (Any | None | Unset):
    """

    encrypted: bool | None | Unset = UNSET
    name: None | str | Unset = UNSET
    rebind_schema: bool | Unset = UNSET
    schema_digest: None | str | Unset = UNSET
    value: Any | Unset = UNSET
    value_schema: Any | None | Unset = UNSET
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

        rebind_schema = self.rebind_schema

        schema_digest: None | str | Unset
        if isinstance(self.schema_digest, Unset):
            schema_digest = UNSET
        else:
            schema_digest = self.schema_digest

        value = self.value

        value_schema: Any | None | Unset
        if isinstance(self.value_schema, Unset):
            value_schema = UNSET
        else:
            value_schema = self.value_schema

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if encrypted is not UNSET:
            field_dict["encrypted"] = encrypted
        if name is not UNSET:
            field_dict["name"] = name
        if rebind_schema is not UNSET:
            field_dict["rebind_schema"] = rebind_schema
        if schema_digest is not UNSET:
            field_dict["schema_digest"] = schema_digest
        if value is not UNSET:
            field_dict["value"] = value
        if value_schema is not UNSET:
            field_dict["value_schema"] = value_schema

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
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

        rebind_schema = d.pop("rebind_schema", UNSET)

        def _parse_schema_digest(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        schema_digest = _parse_schema_digest(d.pop("schema_digest", UNSET))

        value = d.pop("value", UNSET)

        def _parse_value_schema(data: object) -> Any | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Any | None | Unset, data)

        value_schema = _parse_value_schema(d.pop("value_schema", UNSET))

        update_key_request = cls(
            encrypted=encrypted,
            name=name,
            rebind_schema=rebind_schema,
            schema_digest=schema_digest,
            value=value,
            value_schema=value_schema,
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
