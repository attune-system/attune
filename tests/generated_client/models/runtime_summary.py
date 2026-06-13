from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="RuntimeSummary")


@_attrs_define
class RuntimeSummary:
    """Runtime summary for list views.

    Attributes:
        created (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
        id (int):  Example: 1.
        name (str):  Example: Python.
        ref (str):  Example: core.python.
        updated (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
        description (None | str | Unset):  Example: Python runtime with virtualenv support.
        pack_ref (None | str | Unset):  Example: core.
    """

    created: datetime.datetime
    id: int
    name: str
    ref: str
    updated: datetime.datetime
    description: None | str | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        id = self.id

        name = self.name

        ref = self.ref

        updated = self.updated.isoformat()

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "id": id,
                "name": name,
                "ref": ref,
                "updated": updated,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        name = d.pop("name")

        ref = d.pop("ref")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))

        runtime_summary = cls(
            created=created,
            id=id,
            name=name,
            ref=ref,
            updated=updated,
            description=description,
            pack_ref=pack_ref,
        )

        runtime_summary.additional_properties = d
        return runtime_summary

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
