from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="PaginationMeta")


@_attrs_define
class PaginationMeta:
    """Pagination metadata

    Attributes:
        has_next (bool): Whether a next page exists. Example: True.
        has_previous (bool): Whether a previous page exists.
        page (int): Current page number (1-based) Example: 1.
        page_size (int): Number of items per page Example: 50.
        total_items (int | None | Unset): Total number of items, when an exact count was requested. Example: 150.
        total_pages (int | None | Unset): Total number of pages, when an exact count was requested. Example: 3.
    """

    has_next: bool
    has_previous: bool
    page: int
    page_size: int
    total_items: int | None | Unset = UNSET
    total_pages: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        has_next = self.has_next

        has_previous = self.has_previous

        page = self.page

        page_size = self.page_size

        total_items: int | None | Unset
        if isinstance(self.total_items, Unset):
            total_items = UNSET
        else:
            total_items = self.total_items

        total_pages: int | None | Unset
        if isinstance(self.total_pages, Unset):
            total_pages = UNSET
        else:
            total_pages = self.total_pages

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "has_next": has_next,
                "has_previous": has_previous,
                "page": page,
                "page_size": page_size,
            }
        )
        if total_items is not UNSET:
            field_dict["total_items"] = total_items
        if total_pages is not UNSET:
            field_dict["total_pages"] = total_pages

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        has_next = d.pop("has_next")

        has_previous = d.pop("has_previous")

        page = d.pop("page")

        page_size = d.pop("page_size")

        def _parse_total_items(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        total_items = _parse_total_items(d.pop("total_items", UNSET))

        def _parse_total_pages(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        total_pages = _parse_total_pages(d.pop("total_pages", UNSET))

        pagination_meta = cls(
            has_next=has_next,
            has_previous=has_previous,
            page=page,
            page_size=page_size,
            total_items=total_items,
            total_pages=total_pages,
        )

        pagination_meta.additional_properties = d
        return pagination_meta

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
