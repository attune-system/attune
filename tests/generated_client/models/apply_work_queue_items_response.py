from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.work_queue_item_bulk_operation import WorkQueueItemBulkOperation

if TYPE_CHECKING:
    from ..models.work_queue_item_response import WorkQueueItemResponse


T = TypeVar("T", bound="ApplyWorkQueueItemsResponse")


@_attrs_define
class ApplyWorkQueueItemsResponse:
    """
    Attributes:
        affected_count (int):  Example: 120.
        items (list[WorkQueueItemResponse]):
        matched_count (int):  Example: 123.
        operation (WorkQueueItemBulkOperation):
        preview_count (int):  Example: 100.
        skipped_count (int):  Example: 3.
    """

    affected_count: int
    items: list[WorkQueueItemResponse]
    matched_count: int
    operation: WorkQueueItemBulkOperation
    preview_count: int
    skipped_count: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        affected_count = self.affected_count

        items = []
        for items_item_data in self.items:
            items_item = items_item_data.to_dict()
            items.append(items_item)

        matched_count = self.matched_count

        operation = self.operation.value

        preview_count = self.preview_count

        skipped_count = self.skipped_count

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "affected_count": affected_count,
                "items": items,
                "matched_count": matched_count,
                "operation": operation,
                "preview_count": preview_count,
                "skipped_count": skipped_count,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.work_queue_item_response import WorkQueueItemResponse

        d = dict(src_dict)
        affected_count = d.pop("affected_count")

        items = []
        _items = d.pop("items")
        for items_item_data in _items:
            items_item = WorkQueueItemResponse.from_dict(items_item_data)

            items.append(items_item)

        matched_count = d.pop("matched_count")

        operation = WorkQueueItemBulkOperation(d.pop("operation"))

        preview_count = d.pop("preview_count")

        skipped_count = d.pop("skipped_count")

        apply_work_queue_items_response = cls(
            affected_count=affected_count,
            items=items,
            matched_count=matched_count,
            operation=operation,
            preview_count=preview_count,
            skipped_count=skipped_count,
        )

        apply_work_queue_items_response.additional_properties = d
        return apply_work_queue_items_response

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
