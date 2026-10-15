from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.owner_type import OwnerType
from ..types import UNSET, Unset

T = TypeVar("T", bound="GetKeyResponse200Data")


@_attrs_define
class GetKeyResponse200Data:
    """Full key response with all details (value redacted in list views)

    Attributes:
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        encrypted (bool): Whether the value is encrypted Example: True.
        id (int):
        local_ref (str): Key identifier within its owner scope Example: github_token.
        name (str): Human-readable name Example: GitHub API Token.
        owner_type (OwnerType):
        ref (str): Unique reference identifier Example: system.github_token.
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:30:00Z.
        value (Any): JSON value, or null when the caller cannot read and decrypt encrypted material.
        owner (None | str | Unset): Authoritative owner reference Example: github.
        owner_action (int | None | Unset):
        owner_action_ref (None | str | Unset): Owner action reference Example: github.create_issue.
        owner_identity (int | None | Unset):
        owner_pack (int | None | Unset):
        owner_pack_ref (None | str | Unset): Owner pack reference Example: github.
        owner_sensor (int | None | Unset):
        owner_sensor_ref (None | str | Unset): Owner sensor reference Example: github.webhook.
        schema_binding (Any | None | Unset):
        schema_digest (None | str | Unset):
        value_schema (Any | None | Unset):
    """

    created: datetime.datetime
    encrypted: bool
    id: int
    local_ref: str
    name: str
    owner_type: OwnerType
    ref: str
    updated: datetime.datetime
    value: Any
    owner: None | str | Unset = UNSET
    owner_action: int | None | Unset = UNSET
    owner_action_ref: None | str | Unset = UNSET
    owner_identity: int | None | Unset = UNSET
    owner_pack: int | None | Unset = UNSET
    owner_pack_ref: None | str | Unset = UNSET
    owner_sensor: int | None | Unset = UNSET
    owner_sensor_ref: None | str | Unset = UNSET
    schema_binding: Any | None | Unset = UNSET
    schema_digest: None | str | Unset = UNSET
    value_schema: Any | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        encrypted = self.encrypted

        id = self.id

        local_ref = self.local_ref

        name = self.name

        owner_type = self.owner_type.value

        ref = self.ref

        updated = self.updated.isoformat()

        value = self.value

        owner: None | str | Unset
        if isinstance(self.owner, Unset):
            owner = UNSET
        else:
            owner = self.owner

        owner_action: int | None | Unset
        if isinstance(self.owner_action, Unset):
            owner_action = UNSET
        else:
            owner_action = self.owner_action

        owner_action_ref: None | str | Unset
        if isinstance(self.owner_action_ref, Unset):
            owner_action_ref = UNSET
        else:
            owner_action_ref = self.owner_action_ref

        owner_identity: int | None | Unset
        if isinstance(self.owner_identity, Unset):
            owner_identity = UNSET
        else:
            owner_identity = self.owner_identity

        owner_pack: int | None | Unset
        if isinstance(self.owner_pack, Unset):
            owner_pack = UNSET
        else:
            owner_pack = self.owner_pack

        owner_pack_ref: None | str | Unset
        if isinstance(self.owner_pack_ref, Unset):
            owner_pack_ref = UNSET
        else:
            owner_pack_ref = self.owner_pack_ref

        owner_sensor: int | None | Unset
        if isinstance(self.owner_sensor, Unset):
            owner_sensor = UNSET
        else:
            owner_sensor = self.owner_sensor

        owner_sensor_ref: None | str | Unset
        if isinstance(self.owner_sensor_ref, Unset):
            owner_sensor_ref = UNSET
        else:
            owner_sensor_ref = self.owner_sensor_ref

        schema_binding: Any | None | Unset
        if isinstance(self.schema_binding, Unset):
            schema_binding = UNSET
        else:
            schema_binding = self.schema_binding

        schema_digest: None | str | Unset
        if isinstance(self.schema_digest, Unset):
            schema_digest = UNSET
        else:
            schema_digest = self.schema_digest

        value_schema: Any | None | Unset
        if isinstance(self.value_schema, Unset):
            value_schema = UNSET
        else:
            value_schema = self.value_schema

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "encrypted": encrypted,
                "id": id,
                "local_ref": local_ref,
                "name": name,
                "owner_type": owner_type,
                "ref": ref,
                "updated": updated,
                "value": value,
            }
        )
        if owner is not UNSET:
            field_dict["owner"] = owner
        if owner_action is not UNSET:
            field_dict["owner_action"] = owner_action
        if owner_action_ref is not UNSET:
            field_dict["owner_action_ref"] = owner_action_ref
        if owner_identity is not UNSET:
            field_dict["owner_identity"] = owner_identity
        if owner_pack is not UNSET:
            field_dict["owner_pack"] = owner_pack
        if owner_pack_ref is not UNSET:
            field_dict["owner_pack_ref"] = owner_pack_ref
        if owner_sensor is not UNSET:
            field_dict["owner_sensor"] = owner_sensor
        if owner_sensor_ref is not UNSET:
            field_dict["owner_sensor_ref"] = owner_sensor_ref
        if schema_binding is not UNSET:
            field_dict["schema_binding"] = schema_binding
        if schema_digest is not UNSET:
            field_dict["schema_digest"] = schema_digest
        if value_schema is not UNSET:
            field_dict["value_schema"] = value_schema

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        encrypted = d.pop("encrypted")

        id = d.pop("id")

        local_ref = d.pop("local_ref")

        name = d.pop("name")

        owner_type = OwnerType(d.pop("owner_type"))

        ref = d.pop("ref")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        value = d.pop("value")

        def _parse_owner(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner = _parse_owner(d.pop("owner", UNSET))

        def _parse_owner_action(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        owner_action = _parse_owner_action(d.pop("owner_action", UNSET))

        def _parse_owner_action_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_action_ref = _parse_owner_action_ref(d.pop("owner_action_ref", UNSET))

        def _parse_owner_identity(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        owner_identity = _parse_owner_identity(d.pop("owner_identity", UNSET))

        def _parse_owner_pack(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        owner_pack = _parse_owner_pack(d.pop("owner_pack", UNSET))

        def _parse_owner_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_pack_ref = _parse_owner_pack_ref(d.pop("owner_pack_ref", UNSET))

        def _parse_owner_sensor(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        owner_sensor = _parse_owner_sensor(d.pop("owner_sensor", UNSET))

        def _parse_owner_sensor_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_sensor_ref = _parse_owner_sensor_ref(d.pop("owner_sensor_ref", UNSET))

        def _parse_schema_binding(data: object) -> Any | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Any | None | Unset, data)

        schema_binding = _parse_schema_binding(d.pop("schema_binding", UNSET))

        def _parse_schema_digest(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        schema_digest = _parse_schema_digest(d.pop("schema_digest", UNSET))

        def _parse_value_schema(data: object) -> Any | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Any | None | Unset, data)

        value_schema = _parse_value_schema(d.pop("value_schema", UNSET))

        get_key_response_200_data = cls(
            created=created,
            encrypted=encrypted,
            id=id,
            local_ref=local_ref,
            name=name,
            owner_type=owner_type,
            ref=ref,
            updated=updated,
            value=value,
            owner=owner,
            owner_action=owner_action,
            owner_action_ref=owner_action_ref,
            owner_identity=owner_identity,
            owner_pack=owner_pack,
            owner_pack_ref=owner_pack_ref,
            owner_sensor=owner_sensor,
            owner_sensor_ref=owner_sensor_ref,
            schema_binding=schema_binding,
            schema_digest=schema_digest,
            value_schema=value_schema,
        )

        get_key_response_200_data.additional_properties = d
        return get_key_response_200_data

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
