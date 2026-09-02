from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.cache_entry_response import CacheEntryResponse


T = TypeVar("T", bound="CachePointLookupResponse")


@_attrs_define
class CachePointLookupResponse:
    """Point lookup response. `item = None` is an authorized miss.

    Attributes:
        generation_id (int):
        item (CacheEntryResponse | None):
        stale (bool):
    """

    generation_id: int
    item: CacheEntryResponse | None
    stale: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.cache_entry_response import CacheEntryResponse

        generation_id = self.generation_id

        item: dict[str, Any] | None
        if isinstance(self.item, CacheEntryResponse):
            item = self.item.to_dict()
        else:
            item = self.item

        stale = self.stale

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "generation_id": generation_id,
                "item": item,
                "stale": stale,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.cache_entry_response import CacheEntryResponse

        d = dict(src_dict)
        generation_id = d.pop("generation_id")

        def _parse_item(data: object) -> CacheEntryResponse | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                item_type_1 = CacheEntryResponse.from_dict(data)

                return item_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CacheEntryResponse | None, data)

        item = _parse_item(d.pop("item"))

        stale = d.pop("stale")

        cache_point_lookup_response = cls(
            generation_id=generation_id,
            item=item,
            stale=stale,
        )

        cache_point_lookup_response.additional_properties = d
        return cache_point_lookup_response

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
