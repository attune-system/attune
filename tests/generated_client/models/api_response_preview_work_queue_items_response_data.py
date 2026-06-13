from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.work_queue_item_response import WorkQueueItemResponse


T = TypeVar("T", bound="ApiResponsePreviewWorkQueueItemsResponseData")


@_attrs_define
class ApiResponsePreviewWorkQueueItemsResponseData:
    """
    Attributes:
        items (list[WorkQueueItemResponse]):
        matched_count (int):  Example: 123.
        preview_count (int):  Example: 100.
    """

    items: list[WorkQueueItemResponse]
    matched_count: int
    preview_count: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        items = []
        for items_item_data in self.items:
            items_item = items_item_data.to_dict()
            items.append(items_item)

        matched_count = self.matched_count

        preview_count = self.preview_count

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "items": items,
                "matched_count": matched_count,
                "preview_count": preview_count,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.work_queue_item_response import WorkQueueItemResponse

        d = dict(src_dict)
        items = []
        _items = d.pop("items")
        for items_item_data in _items:
            items_item = WorkQueueItemResponse.from_dict(items_item_data)

            items.append(items_item)

        matched_count = d.pop("matched_count")

        preview_count = d.pop("preview_count")

        api_response_preview_work_queue_items_response_data = cls(
            items=items,
            matched_count=matched_count,
            preview_count=preview_count,
        )

        api_response_preview_work_queue_items_response_data.additional_properties = d
        return api_response_preview_work_queue_items_response_data

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
