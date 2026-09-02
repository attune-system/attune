from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.owner_type import OwnerType
from ..types import UNSET, Unset

T = TypeVar("T", bound="CachePointLookupRequest")


@_attrs_define
class CachePointLookupRequest:
    """Point lookup request. Identifiers are placed in the body to avoid access-log
    leakage.

        Attributes:
            external_id (str):
            owner_type (OwnerType):
            generation_id (int | None | Unset):
            owner_ref (None | str | Unset):
            require_fresh (bool | Unset):
    """

    external_id: str
    owner_type: OwnerType
    generation_id: int | None | Unset = UNSET
    owner_ref: None | str | Unset = UNSET
    require_fresh: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        external_id = self.external_id

        owner_type = self.owner_type.value

        generation_id: int | None | Unset
        if isinstance(self.generation_id, Unset):
            generation_id = UNSET
        else:
            generation_id = self.generation_id

        owner_ref: None | str | Unset
        if isinstance(self.owner_ref, Unset):
            owner_ref = UNSET
        else:
            owner_ref = self.owner_ref

        require_fresh = self.require_fresh

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "external_id": external_id,
                "owner_type": owner_type,
            }
        )
        if generation_id is not UNSET:
            field_dict["generation_id"] = generation_id
        if owner_ref is not UNSET:
            field_dict["owner_ref"] = owner_ref
        if require_fresh is not UNSET:
            field_dict["require_fresh"] = require_fresh

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        external_id = d.pop("external_id")

        owner_type = OwnerType(d.pop("owner_type"))

        def _parse_generation_id(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        generation_id = _parse_generation_id(d.pop("generation_id", UNSET))

        def _parse_owner_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_ref = _parse_owner_ref(d.pop("owner_ref", UNSET))

        require_fresh = d.pop("require_fresh", UNSET)

        cache_point_lookup_request = cls(
            external_id=external_id,
            owner_type=owner_type,
            generation_id=generation_id,
            owner_ref=owner_ref,
            require_fresh=require_fresh,
        )

        cache_point_lookup_request.additional_properties = d
        return cache_point_lookup_request

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
