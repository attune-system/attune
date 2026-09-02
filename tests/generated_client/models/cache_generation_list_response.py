from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.cache_generation_response import CacheGenerationResponse


T = TypeVar("T", bound="CacheGenerationListResponse")


@_attrs_define
class CacheGenerationListResponse:
    """Wrapper for a generation list.

    Attributes:
        generations (list[CacheGenerationResponse]):
        next_cursor (None | str):
    """

    generations: list[CacheGenerationResponse]
    next_cursor: None | str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        generations = []
        for generations_item_data in self.generations:
            generations_item = generations_item_data.to_dict()
            generations.append(generations_item)

        next_cursor: None | str
        next_cursor = self.next_cursor

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "generations": generations,
                "next_cursor": next_cursor,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.cache_generation_response import (
            CacheGenerationResponse,
        )

        d = dict(src_dict)
        generations = []
        _generations = d.pop("generations")
        for generations_item_data in _generations:
            generations_item = CacheGenerationResponse.from_dict(generations_item_data)

            generations.append(generations_item)

        def _parse_next_cursor(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        next_cursor = _parse_next_cursor(d.pop("next_cursor"))

        cache_generation_list_response = cls(
            generations=generations,
            next_cursor=next_cursor,
        )

        cache_generation_list_response.additional_properties = d
        return cache_generation_list_response

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
