from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
  from ..models.cache_entry_response import CacheEntryResponse





T = TypeVar("T", bound="CacheMultiLookupResponse")



@_attrs_define
class CacheMultiLookupResponse:
    """ Bounded multi-ID lookup response. Missing IDs are reported explicitly.

        Attributes:
            generation_id (int):
            items (list[CacheEntryResponse]):
            missing_external_ids (list[str]):
            stale (bool):
     """

    generation_id: int
    items: list[CacheEntryResponse]
    missing_external_ids: list[str]
    stale: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        from ..models.cache_entry_response import CacheEntryResponse
        generation_id = self.generation_id

        items = []
        for items_item_data in self.items:
            items_item = items_item_data.to_dict()
            items.append(items_item)



        missing_external_ids = self.missing_external_ids



        stale = self.stale


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "generation_id": generation_id,
            "items": items,
            "missing_external_ids": missing_external_ids,
            "stale": stale,
        })

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.cache_entry_response import CacheEntryResponse
        d = dict(src_dict)
        generation_id = d.pop("generation_id")

        items = []
        _items = d.pop("items")
        for items_item_data in (_items):
            items_item = CacheEntryResponse.from_dict(items_item_data)



            items.append(items_item)


        missing_external_ids = cast(list[str], d.pop("missing_external_ids"))


        stale = d.pop("stale")

        cache_multi_lookup_response = cls(
            generation_id=generation_id,
            items=items,
            missing_external_ids=missing_external_ids,
            stale=stale,
        )


        cache_multi_lookup_response.additional_properties = d
        return cache_multi_lookup_response

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
