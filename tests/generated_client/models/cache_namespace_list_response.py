from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
  from ..models.cache_namespace_response import CacheNamespaceResponse





T = TypeVar("T", bound="CacheNamespaceListResponse")



@_attrs_define
class CacheNamespaceListResponse:
    """ Wrapper for a namespace list scoped to one owner.

        Attributes:
            namespaces (list[CacheNamespaceResponse]):
            next_cursor (None | str):
     """

    namespaces: list[CacheNamespaceResponse]
    next_cursor: None | str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        from ..models.cache_namespace_response import CacheNamespaceResponse
        namespaces = []
        for namespaces_item_data in self.namespaces:
            namespaces_item = namespaces_item_data.to_dict()
            namespaces.append(namespaces_item)



        next_cursor: None | str
        next_cursor = self.next_cursor


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "namespaces": namespaces,
            "next_cursor": next_cursor,
        })

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.cache_namespace_response import CacheNamespaceResponse
        d = dict(src_dict)
        namespaces = []
        _namespaces = d.pop("namespaces")
        for namespaces_item_data in (_namespaces):
            namespaces_item = CacheNamespaceResponse.from_dict(namespaces_item_data)



            namespaces.append(namespaces_item)


        def _parse_next_cursor(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        next_cursor = _parse_next_cursor(d.pop("next_cursor"))


        cache_namespace_list_response = cls(
            namespaces=namespaces,
            next_cursor=next_cursor,
        )


        cache_namespace_list_response.additional_properties = d
        return cache_namespace_list_response

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
