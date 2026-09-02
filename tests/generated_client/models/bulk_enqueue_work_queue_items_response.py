from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.work_queue_item_response import WorkQueueItemResponse


T = TypeVar("T", bound="BulkEnqueueWorkQueueItemsResponse")


@_attrs_define
class BulkEnqueueWorkQueueItemsResponse:
    """
    Attributes:
        created_count (int):  Example: 2.
        items (list[WorkQueueItemResponse]):
        updated_count (int):  Example: 1.
    """

    created_count: int
    items: list[WorkQueueItemResponse]
    updated_count: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created_count = self.created_count

        items = []
        for items_item_data in self.items:
            items_item = items_item_data.to_dict()
            items.append(items_item)

        updated_count = self.updated_count

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_count": created_count,
                "items": items,
                "updated_count": updated_count,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.work_queue_item_response import (
            WorkQueueItemResponse,
        )

        d = dict(src_dict)
        created_count = d.pop("created_count")

        items = []
        _items = d.pop("items")
        for items_item_data in _items:
            items_item = WorkQueueItemResponse.from_dict(items_item_data)

            items.append(items_item)

        updated_count = d.pop("updated_count")

        bulk_enqueue_work_queue_items_response = cls(
            created_count=created_count,
            items=items,
            updated_count=updated_count,
        )

        bulk_enqueue_work_queue_items_response.additional_properties = d
        return bulk_enqueue_work_queue_items_response

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
