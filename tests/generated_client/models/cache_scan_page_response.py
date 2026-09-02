from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.cache_entry_response import CacheEntryResponse


T = TypeVar("T", bound="CacheScanPageResponse")


@_attrs_define
class CacheScanPageResponse:
    """One generation-pinned scan page.

    Attributes:
        cursor_expires_at (datetime.datetime | None):
        generation_id (int):
        items (list[CacheEntryResponse]):
        next_cursor (None | str):
        record_count (int | None):
        stale (bool):
    """

    cursor_expires_at: datetime.datetime | None
    generation_id: int
    items: list[CacheEntryResponse]
    next_cursor: None | str
    record_count: int | None
    stale: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        cursor_expires_at: None | str
        if isinstance(self.cursor_expires_at, datetime.datetime):
            cursor_expires_at = self.cursor_expires_at.isoformat()
        else:
            cursor_expires_at = self.cursor_expires_at

        generation_id = self.generation_id

        items = []
        for items_item_data in self.items:
            items_item = items_item_data.to_dict()
            items.append(items_item)

        next_cursor: None | str
        next_cursor = self.next_cursor

        record_count: int | None
        record_count = self.record_count

        stale = self.stale

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "cursor_expires_at": cursor_expires_at,
                "generation_id": generation_id,
                "items": items,
                "next_cursor": next_cursor,
                "record_count": record_count,
                "stale": stale,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.cache_entry_response import CacheEntryResponse

        d = dict(src_dict)

        def _parse_cursor_expires_at(data: object) -> datetime.datetime | None:
            if data is None:
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                cursor_expires_at_type_0 = datetime.datetime.fromisoformat(data)

                return cursor_expires_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None, data)

        cursor_expires_at = _parse_cursor_expires_at(d.pop("cursor_expires_at"))

        generation_id = d.pop("generation_id")

        items = []
        _items = d.pop("items")
        for items_item_data in _items:
            items_item = CacheEntryResponse.from_dict(items_item_data)

            items.append(items_item)

        def _parse_next_cursor(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        next_cursor = _parse_next_cursor(d.pop("next_cursor"))

        def _parse_record_count(data: object) -> int | None:
            if data is None:
                return data
            return cast(int | None, data)

        record_count = _parse_record_count(d.pop("record_count"))

        stale = d.pop("stale")

        cache_scan_page_response = cls(
            cursor_expires_at=cursor_expires_at,
            generation_id=generation_id,
            items=items,
            next_cursor=next_cursor,
            record_count=record_count,
            stale=stale,
        )

        cache_scan_page_response.additional_properties = d
        return cache_scan_page_response

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
