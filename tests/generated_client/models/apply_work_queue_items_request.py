from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.work_queue_item_bulk_operation import WorkQueueItemBulkOperation
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.apply_work_queue_items_request_payload_patch_type_0 import (
        ApplyWorkQueueItemsRequestPayloadPatchType0,
    )
    from ..models.work_queue_item_json_path_selector import (
        WorkQueueItemJsonPathSelector,
    )


T = TypeVar("T", bound="ApplyWorkQueueItemsRequest")


@_attrs_define
class ApplyWorkQueueItemsRequest:
    """
    Attributes:
        operation (WorkQueueItemBulkOperation):
        payload_patch (ApplyWorkQueueItemsRequestPayloadPatchType0 | None):
        selector (WorkQueueItemJsonPathSelector):
        preview_limit (int | Unset):  Default: 100. Example: 100.
        priority (int | None | Unset):  Example: 25.
    """

    operation: WorkQueueItemBulkOperation
    payload_patch: ApplyWorkQueueItemsRequestPayloadPatchType0 | None
    selector: WorkQueueItemJsonPathSelector
    preview_limit: int | Unset = 100
    priority: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.apply_work_queue_items_request_payload_patch_type_0 import (
            ApplyWorkQueueItemsRequestPayloadPatchType0,
        )

        operation = self.operation.value

        payload_patch: dict[str, Any] | None
        if isinstance(self.payload_patch, ApplyWorkQueueItemsRequestPayloadPatchType0):
            payload_patch = self.payload_patch.to_dict()
        else:
            payload_patch = self.payload_patch

        selector = self.selector.to_dict()

        preview_limit = self.preview_limit

        priority: int | None | Unset
        if isinstance(self.priority, Unset):
            priority = UNSET
        else:
            priority = self.priority

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "operation": operation,
                "payload_patch": payload_patch,
                "selector": selector,
            }
        )
        if preview_limit is not UNSET:
            field_dict["preview_limit"] = preview_limit
        if priority is not UNSET:
            field_dict["priority"] = priority

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.apply_work_queue_items_request_payload_patch_type_0 import (
            ApplyWorkQueueItemsRequestPayloadPatchType0,
        )
        from ..models.work_queue_item_json_path_selector import (
            WorkQueueItemJsonPathSelector,
        )

        d = dict(src_dict)
        operation = WorkQueueItemBulkOperation(d.pop("operation"))

        def _parse_payload_patch(
            data: object,
        ) -> ApplyWorkQueueItemsRequestPayloadPatchType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                payload_patch_type_0 = (
                    ApplyWorkQueueItemsRequestPayloadPatchType0.from_dict(data)
                )

                return payload_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ApplyWorkQueueItemsRequestPayloadPatchType0 | None, data)

        payload_patch = _parse_payload_patch(d.pop("payload_patch"))

        selector = WorkQueueItemJsonPathSelector.from_dict(d.pop("selector"))

        preview_limit = d.pop("preview_limit", UNSET)

        def _parse_priority(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        priority = _parse_priority(d.pop("priority", UNSET))

        apply_work_queue_items_request = cls(
            operation=operation,
            payload_patch=payload_patch,
            selector=selector,
            preview_limit=preview_limit,
            priority=priority,
        )

        apply_work_queue_items_request.additional_properties = d
        return apply_work_queue_items_request

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
