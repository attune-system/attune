from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.nullable_string_patch_type_1 import NullableStringPatchType1
    from ..models.set_string import SetString
    from ..models.update_work_queue_item_request_metadata_type_0 import (
        UpdateWorkQueueItemRequestMetadataType0,
    )
    from ..models.update_work_queue_item_request_payload_type_0 import (
        UpdateWorkQueueItemRequestPayloadType0,
    )


T = TypeVar("T", bound="UpdateWorkQueueItemRequest")


@_attrs_define
class UpdateWorkQueueItemRequest:
    """
    Attributes:
        metadata (None | UpdateWorkQueueItemRequestMetadataType0):
        payload (None | UpdateWorkQueueItemRequestPayloadType0):
        item_key (None | NullableStringPatchType1 | SetString | Unset):
        priority (int | None | Unset):  Example: 10.
    """

    metadata: None | UpdateWorkQueueItemRequestMetadataType0
    payload: None | UpdateWorkQueueItemRequestPayloadType0
    item_key: None | NullableStringPatchType1 | SetString | Unset = UNSET
    priority: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.nullable_string_patch_type_1 import NullableStringPatchType1
        from ..models.set_string import SetString
        from ..models.update_work_queue_item_request_metadata_type_0 import (
            UpdateWorkQueueItemRequestMetadataType0,
        )
        from ..models.update_work_queue_item_request_payload_type_0 import (
            UpdateWorkQueueItemRequestPayloadType0,
        )

        metadata: dict[str, Any] | None
        if isinstance(self.metadata, UpdateWorkQueueItemRequestMetadataType0):
            metadata = self.metadata.to_dict()
        else:
            metadata = self.metadata

        payload: dict[str, Any] | None
        if isinstance(self.payload, UpdateWorkQueueItemRequestPayloadType0):
            payload = self.payload.to_dict()
        else:
            payload = self.payload

        item_key: dict[str, Any] | None | Unset
        if isinstance(self.item_key, Unset):
            item_key = UNSET
        elif isinstance(self.item_key, SetString):
            item_key = self.item_key.to_dict()
        elif isinstance(self.item_key, NullableStringPatchType1):
            item_key = self.item_key.to_dict()
        else:
            item_key = self.item_key

        priority: int | None | Unset
        if isinstance(self.priority, Unset):
            priority = UNSET
        else:
            priority = self.priority

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "metadata": metadata,
                "payload": payload,
            }
        )
        if item_key is not UNSET:
            field_dict["item_key"] = item_key
        if priority is not UNSET:
            field_dict["priority"] = priority

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.nullable_string_patch_type_1 import NullableStringPatchType1
        from ..models.set_string import SetString
        from ..models.update_work_queue_item_request_metadata_type_0 import (
            UpdateWorkQueueItemRequestMetadataType0,
        )
        from ..models.update_work_queue_item_request_payload_type_0 import (
            UpdateWorkQueueItemRequestPayloadType0,
        )

        d = dict(src_dict)

        def _parse_metadata(
            data: object,
        ) -> None | UpdateWorkQueueItemRequestMetadataType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                metadata_type_0 = UpdateWorkQueueItemRequestMetadataType0.from_dict(
                    data
                )

                return metadata_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateWorkQueueItemRequestMetadataType0, data)

        metadata = _parse_metadata(d.pop("metadata"))

        def _parse_payload(
            data: object,
        ) -> None | UpdateWorkQueueItemRequestPayloadType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                payload_type_0 = UpdateWorkQueueItemRequestPayloadType0.from_dict(data)

                return payload_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateWorkQueueItemRequestPayloadType0, data)

        payload = _parse_payload(d.pop("payload"))

        def _parse_item_key(
            data: object,
        ) -> None | NullableStringPatchType1 | SetString | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_nullable_string_patch_set_string = (
                    SetString.from_dict(data)
                )

                return componentsschemas_nullable_string_patch_set_string
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_nullable_string_patch_type_1 = (
                    NullableStringPatchType1.from_dict(data)
                )

                return componentsschemas_nullable_string_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | NullableStringPatchType1 | SetString | Unset, data)

        item_key = _parse_item_key(d.pop("item_key", UNSET))

        def _parse_priority(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        priority = _parse_priority(d.pop("priority", UNSET))

        update_work_queue_item_request = cls(
            metadata=metadata,
            payload=payload,
            item_key=item_key,
            priority=priority,
        )

        update_work_queue_item_request.additional_properties = d
        return update_work_queue_item_request

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
