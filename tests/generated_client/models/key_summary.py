from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.owner_type import OwnerType
from ..types import UNSET, Unset

T = TypeVar("T", bound="KeySummary")


@_attrs_define
class KeySummary:
    """Summary key response for list views (value redacted)

    Attributes:
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        encrypted (bool): Whether the value is encrypted Example: True.
        id (int):
        name (str): Human-readable name Example: GitHub API Token.
        owner_type (OwnerType):
        ref (str): Unique reference identifier Example: github_token.
        owner (None | str | Unset): Owner identifier Example: github-integration.
    """

    created: datetime.datetime
    encrypted: bool
    id: int
    name: str
    owner_type: OwnerType
    ref: str
    owner: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        encrypted = self.encrypted

        id = self.id

        name = self.name

        owner_type = self.owner_type.value

        ref = self.ref

        owner: None | str | Unset
        if isinstance(self.owner, Unset):
            owner = UNSET
        else:
            owner = self.owner

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "encrypted": encrypted,
                "id": id,
                "name": name,
                "owner_type": owner_type,
                "ref": ref,
            }
        )
        if owner is not UNSET:
            field_dict["owner"] = owner

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        encrypted = d.pop("encrypted")

        id = d.pop("id")

        name = d.pop("name")

        owner_type = OwnerType(d.pop("owner_type"))

        ref = d.pop("ref")

        def _parse_owner(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner = _parse_owner(d.pop("owner", UNSET))

        key_summary = cls(
            created=created,
            encrypted=encrypted,
            id=id,
            name=name,
            owner_type=owner_type,
            ref=ref,
            owner=owner,
        )

        key_summary.additional_properties = d
        return key_summary

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
