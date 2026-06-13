from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.enqueue_work_queue_item_request_metadata import (
        EnqueueWorkQueueItemRequestMetadata,
    )
    from ..models.enqueue_work_queue_item_request_payload import (
        EnqueueWorkQueueItemRequestPayload,
    )


T = TypeVar("T", bound="EnqueueWorkQueueItemRequest")


@_attrs_define
class EnqueueWorkQueueItemRequest:
    """
    Attributes:
        payload (EnqueueWorkQueueItemRequestPayload):
        item_key (None | str | Unset):  Example: order-123.
        metadata (EnqueueWorkQueueItemRequestMetadata | Unset):
        priority (int | None | Unset):  Example: 5.
    """

    payload: EnqueueWorkQueueItemRequestPayload
    item_key: None | str | Unset = UNSET
    metadata: EnqueueWorkQueueItemRequestMetadata | Unset = UNSET
    priority: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        payload = self.payload.to_dict()

        item_key: None | str | Unset
        if isinstance(self.item_key, Unset):
            item_key = UNSET
        else:
            item_key = self.item_key

        metadata: dict[str, Any] | Unset = UNSET
        if not isinstance(self.metadata, Unset):
            metadata = self.metadata.to_dict()

        priority: int | None | Unset
        if isinstance(self.priority, Unset):
            priority = UNSET
        else:
            priority = self.priority

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "payload": payload,
            }
        )
        if item_key is not UNSET:
            field_dict["item_key"] = item_key
        if metadata is not UNSET:
            field_dict["metadata"] = metadata
        if priority is not UNSET:
            field_dict["priority"] = priority

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.enqueue_work_queue_item_request_metadata import (
            EnqueueWorkQueueItemRequestMetadata,
        )
        from ..models.enqueue_work_queue_item_request_payload import (
            EnqueueWorkQueueItemRequestPayload,
        )

        d = dict(src_dict)
        payload = EnqueueWorkQueueItemRequestPayload.from_dict(d.pop("payload"))

        def _parse_item_key(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        item_key = _parse_item_key(d.pop("item_key", UNSET))

        _metadata = d.pop("metadata", UNSET)
        metadata: EnqueueWorkQueueItemRequestMetadata | Unset
        if isinstance(_metadata, Unset):
            metadata = UNSET
        else:
            metadata = EnqueueWorkQueueItemRequestMetadata.from_dict(_metadata)

        def _parse_priority(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        priority = _parse_priority(d.pop("priority", UNSET))

        enqueue_work_queue_item_request = cls(
            payload=payload,
            item_key=item_key,
            metadata=metadata,
            priority=priority,
        )

        enqueue_work_queue_item_request.additional_properties = d
        return enqueue_work_queue_item_request

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
