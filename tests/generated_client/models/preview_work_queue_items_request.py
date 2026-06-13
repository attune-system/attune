from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.work_queue_item_json_path_selector import (
        WorkQueueItemJsonPathSelector,
    )


T = TypeVar("T", bound="PreviewWorkQueueItemsRequest")


@_attrs_define
class PreviewWorkQueueItemsRequest:
    """
    Attributes:
        selector (WorkQueueItemJsonPathSelector):
        limit (int | Unset):  Default: 100. Example: 100.
    """

    selector: WorkQueueItemJsonPathSelector
    limit: int | Unset = 100
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        selector = self.selector.to_dict()

        limit = self.limit

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "selector": selector,
            }
        )
        if limit is not UNSET:
            field_dict["limit"] = limit

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.work_queue_item_json_path_selector import (
            WorkQueueItemJsonPathSelector,
        )

        d = dict(src_dict)
        selector = WorkQueueItemJsonPathSelector.from_dict(d.pop("selector"))

        limit = d.pop("limit", UNSET)

        preview_work_queue_items_request = cls(
            selector=selector,
            limit=limit,
        )

        preview_work_queue_items_request.additional_properties = d
        return preview_work_queue_items_request

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
