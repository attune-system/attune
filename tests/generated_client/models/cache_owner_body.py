from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.owner_type import OwnerType
from ..types import UNSET, Unset

T = TypeVar("T", bound="CacheOwnerBody")


@_attrs_define
class CacheOwnerBody:
    """Owner selector accepted in cache request bodies.

    `owner_ref` is the pack/action/sensor reference; it is omitted for the
    `system` scope and resolved to the authenticated identity for `identity`.

        Attributes:
            owner_type (OwnerType):
            owner_ref (None | str | Unset):
    """

    owner_type: OwnerType
    owner_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        owner_type = self.owner_type.value

        owner_ref: None | str | Unset
        if isinstance(self.owner_ref, Unset):
            owner_ref = UNSET
        else:
            owner_ref = self.owner_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "owner_type": owner_type,
            }
        )
        if owner_ref is not UNSET:
            field_dict["owner_ref"] = owner_ref

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        owner_type = OwnerType(d.pop("owner_type"))

        def _parse_owner_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_ref = _parse_owner_ref(d.pop("owner_ref", UNSET))

        cache_owner_body = cls(
            owner_type=owner_type,
            owner_ref=owner_ref,
        )

        cache_owner_body.additional_properties = d
        return cache_owner_body

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
