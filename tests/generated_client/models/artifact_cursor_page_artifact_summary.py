from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, Self, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.artifact_cursor_page_artifact_summary_items_item import (
        ArtifactCursorPageArtifactSummaryItemsItem,
    )


T = TypeVar("T", bound="ArtifactCursorPageArtifactSummary")


@_attrs_define
class ArtifactCursorPageArtifactSummary:
    """Count-free v1 artifact collection response. Pages are not historical snapshots.

    Attributes:
        has_more (bool):
        items (list[ArtifactCursorPageArtifactSummaryItemsItem]):
        next_cursor (None | str | Unset):
    """

    has_more: bool
    items: list[ArtifactCursorPageArtifactSummaryItemsItem]
    next_cursor: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        has_more = self.has_more

        items = []
        for items_item_data in self.items:
            items_item = items_item_data.to_dict()
            items.append(items_item)

        next_cursor: None | str | Unset
        if isinstance(self.next_cursor, Unset):
            next_cursor = UNSET
        else:
            next_cursor = self.next_cursor

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "has_more": has_more,
                "items": items,
            }
        )
        if next_cursor is not UNSET:
            field_dict["next_cursor"] = next_cursor

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.artifact_cursor_page_artifact_summary_items_item import (
            ArtifactCursorPageArtifactSummaryItemsItem,
        )

        d = dict(src_dict)
        has_more = d.pop("has_more")

        items = []
        _items = d.pop("items")
        for items_item_data in _items:
            items_item = ArtifactCursorPageArtifactSummaryItemsItem.from_dict(
                items_item_data
            )

            items.append(items_item)

        def _parse_next_cursor(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        next_cursor = _parse_next_cursor(d.pop("next_cursor", UNSET))

        artifact_cursor_page_artifact_summary = cls(
            has_more=has_more,
            items=items,
            next_cursor=next_cursor,
        )

        artifact_cursor_page_artifact_summary.additional_properties = d
        return artifact_cursor_page_artifact_summary

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
