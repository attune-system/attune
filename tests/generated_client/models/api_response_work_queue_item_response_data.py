from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.work_queue_item_status import WorkQueueItemStatus
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.api_response_work_queue_item_response_data_ack_summary_type_0 import (
        ApiResponseWorkQueueItemResponseDataAckSummaryType0,
    )
    from ..models.api_response_work_queue_item_response_data_last_error_type_0 import (
        ApiResponseWorkQueueItemResponseDataLastErrorType0,
    )
    from ..models.api_response_work_queue_item_response_data_metadata import (
        ApiResponseWorkQueueItemResponseDataMetadata,
    )
    from ..models.api_response_work_queue_item_response_data_payload import (
        ApiResponseWorkQueueItemResponseDataPayload,
    )


T = TypeVar("T", bound="ApiResponseWorkQueueItemResponseData")


@_attrs_define
class ApiResponseWorkQueueItemResponseData:
    """
    Attributes:
        ack_summary (ApiResponseWorkQueueItemResponseDataAckSummaryType0 | None):
        attempt_count (int):
        created (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
        enqueue_source (str):  Example: api.
        id (int):
        last_error (ApiResponseWorkQueueItemResponseDataLastErrorType0 | None):
        metadata (ApiResponseWorkQueueItemResponseDataMetadata):
        payload (ApiResponseWorkQueueItemResponseDataPayload):
        priority (int):  Example: 5.
        queue (int):
        queue_ref (str):  Example: core.inbox.
        status (WorkQueueItemStatus):
        updated (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
        item_key (None | str | Unset):  Example: order-123.
        lease_expires_at (datetime.datetime | None | Unset):  Example: 2024-01-13T10:30:00Z.
        lease_token (None | Unset | UUID):
        leased_execution (int | None | Unset):
        requested_by_enforcement (int | None | Unset):
        requested_by_execution (int | None | Unset):
        requested_by_identity (int | None | Unset):
    """

    ack_summary: ApiResponseWorkQueueItemResponseDataAckSummaryType0 | None
    attempt_count: int
    created: datetime.datetime
    enqueue_source: str
    id: int
    last_error: ApiResponseWorkQueueItemResponseDataLastErrorType0 | None
    metadata: ApiResponseWorkQueueItemResponseDataMetadata
    payload: ApiResponseWorkQueueItemResponseDataPayload
    priority: int
    queue: int
    queue_ref: str
    status: WorkQueueItemStatus
    updated: datetime.datetime
    item_key: None | str | Unset = UNSET
    lease_expires_at: datetime.datetime | None | Unset = UNSET
    lease_token: None | Unset | UUID = UNSET
    leased_execution: int | None | Unset = UNSET
    requested_by_enforcement: int | None | Unset = UNSET
    requested_by_execution: int | None | Unset = UNSET
    requested_by_identity: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.api_response_work_queue_item_response_data_ack_summary_type_0 import (
            ApiResponseWorkQueueItemResponseDataAckSummaryType0,
        )
        from ..models.api_response_work_queue_item_response_data_last_error_type_0 import (
            ApiResponseWorkQueueItemResponseDataLastErrorType0,
        )

        ack_summary: dict[str, Any] | None
        if isinstance(
            self.ack_summary, ApiResponseWorkQueueItemResponseDataAckSummaryType0
        ):
            ack_summary = self.ack_summary.to_dict()
        else:
            ack_summary = self.ack_summary

        attempt_count = self.attempt_count

        created = self.created.isoformat()

        enqueue_source = self.enqueue_source

        id = self.id

        last_error: dict[str, Any] | None
        if isinstance(
            self.last_error, ApiResponseWorkQueueItemResponseDataLastErrorType0
        ):
            last_error = self.last_error.to_dict()
        else:
            last_error = self.last_error

        metadata = self.metadata.to_dict()

        payload = self.payload.to_dict()

        priority = self.priority

        queue = self.queue

        queue_ref = self.queue_ref

        status = self.status.value

        updated = self.updated.isoformat()

        item_key: None | str | Unset
        if isinstance(self.item_key, Unset):
            item_key = UNSET
        else:
            item_key = self.item_key

        lease_expires_at: None | str | Unset
        if isinstance(self.lease_expires_at, Unset):
            lease_expires_at = UNSET
        elif isinstance(self.lease_expires_at, datetime.datetime):
            lease_expires_at = self.lease_expires_at.isoformat()
        else:
            lease_expires_at = self.lease_expires_at

        lease_token: None | str | Unset
        if isinstance(self.lease_token, Unset):
            lease_token = UNSET
        elif isinstance(self.lease_token, UUID):
            lease_token = str(self.lease_token)
        else:
            lease_token = self.lease_token

        leased_execution: int | None | Unset
        if isinstance(self.leased_execution, Unset):
            leased_execution = UNSET
        else:
            leased_execution = self.leased_execution

        requested_by_enforcement: int | None | Unset
        if isinstance(self.requested_by_enforcement, Unset):
            requested_by_enforcement = UNSET
        else:
            requested_by_enforcement = self.requested_by_enforcement

        requested_by_execution: int | None | Unset
        if isinstance(self.requested_by_execution, Unset):
            requested_by_execution = UNSET
        else:
            requested_by_execution = self.requested_by_execution

        requested_by_identity: int | None | Unset
        if isinstance(self.requested_by_identity, Unset):
            requested_by_identity = UNSET
        else:
            requested_by_identity = self.requested_by_identity

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "ack_summary": ack_summary,
                "attempt_count": attempt_count,
                "created": created,
                "enqueue_source": enqueue_source,
                "id": id,
                "last_error": last_error,
                "metadata": metadata,
                "payload": payload,
                "priority": priority,
                "queue": queue,
                "queue_ref": queue_ref,
                "status": status,
                "updated": updated,
            }
        )
        if item_key is not UNSET:
            field_dict["item_key"] = item_key
        if lease_expires_at is not UNSET:
            field_dict["lease_expires_at"] = lease_expires_at
        if lease_token is not UNSET:
            field_dict["lease_token"] = lease_token
        if leased_execution is not UNSET:
            field_dict["leased_execution"] = leased_execution
        if requested_by_enforcement is not UNSET:
            field_dict["requested_by_enforcement"] = requested_by_enforcement
        if requested_by_execution is not UNSET:
            field_dict["requested_by_execution"] = requested_by_execution
        if requested_by_identity is not UNSET:
            field_dict["requested_by_identity"] = requested_by_identity

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.api_response_work_queue_item_response_data_ack_summary_type_0 import (
            ApiResponseWorkQueueItemResponseDataAckSummaryType0,
        )
        from ..models.api_response_work_queue_item_response_data_last_error_type_0 import (
            ApiResponseWorkQueueItemResponseDataLastErrorType0,
        )
        from ..models.api_response_work_queue_item_response_data_metadata import (
            ApiResponseWorkQueueItemResponseDataMetadata,
        )
        from ..models.api_response_work_queue_item_response_data_payload import (
            ApiResponseWorkQueueItemResponseDataPayload,
        )

        d = dict(src_dict)

        def _parse_ack_summary(
            data: object,
        ) -> ApiResponseWorkQueueItemResponseDataAckSummaryType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                ack_summary_type_0 = (
                    ApiResponseWorkQueueItemResponseDataAckSummaryType0.from_dict(data)
                )

                return ack_summary_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                ApiResponseWorkQueueItemResponseDataAckSummaryType0 | None, data
            )

        ack_summary = _parse_ack_summary(d.pop("ack_summary"))

        attempt_count = d.pop("attempt_count")

        created = datetime.datetime.fromisoformat(d.pop("created"))

        enqueue_source = d.pop("enqueue_source")

        id = d.pop("id")

        def _parse_last_error(
            data: object,
        ) -> ApiResponseWorkQueueItemResponseDataLastErrorType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                last_error_type_0 = (
                    ApiResponseWorkQueueItemResponseDataLastErrorType0.from_dict(data)
                )

                return last_error_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ApiResponseWorkQueueItemResponseDataLastErrorType0 | None, data)

        last_error = _parse_last_error(d.pop("last_error"))

        metadata = ApiResponseWorkQueueItemResponseDataMetadata.from_dict(
            d.pop("metadata")
        )

        payload = ApiResponseWorkQueueItemResponseDataPayload.from_dict(
            d.pop("payload")
        )

        priority = d.pop("priority")

        queue = d.pop("queue")

        queue_ref = d.pop("queue_ref")

        status = WorkQueueItemStatus(d.pop("status"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_item_key(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        item_key = _parse_item_key(d.pop("item_key", UNSET))

        def _parse_lease_expires_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                lease_expires_at_type_0 = datetime.datetime.fromisoformat(data)

                return lease_expires_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        lease_expires_at = _parse_lease_expires_at(d.pop("lease_expires_at", UNSET))

        def _parse_lease_token(data: object) -> None | Unset | UUID:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                lease_token_type_0 = UUID(data)

                return lease_token_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UUID, data)

        lease_token = _parse_lease_token(d.pop("lease_token", UNSET))

        def _parse_leased_execution(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        leased_execution = _parse_leased_execution(d.pop("leased_execution", UNSET))

        def _parse_requested_by_enforcement(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        requested_by_enforcement = _parse_requested_by_enforcement(
            d.pop("requested_by_enforcement", UNSET)
        )

        def _parse_requested_by_execution(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        requested_by_execution = _parse_requested_by_execution(
            d.pop("requested_by_execution", UNSET)
        )

        def _parse_requested_by_identity(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        requested_by_identity = _parse_requested_by_identity(
            d.pop("requested_by_identity", UNSET)
        )

        api_response_work_queue_item_response_data = cls(
            ack_summary=ack_summary,
            attempt_count=attempt_count,
            created=created,
            enqueue_source=enqueue_source,
            id=id,
            last_error=last_error,
            metadata=metadata,
            payload=payload,
            priority=priority,
            queue=queue,
            queue_ref=queue_ref,
            status=status,
            updated=updated,
            item_key=item_key,
            lease_expires_at=lease_expires_at,
            lease_token=lease_token,
            leased_execution=leased_execution,
            requested_by_enforcement=requested_by_enforcement,
            requested_by_execution=requested_by_execution,
            requested_by_identity=requested_by_identity,
        )

        api_response_work_queue_item_response_data.additional_properties = d
        return api_response_work_queue_item_response_data

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
