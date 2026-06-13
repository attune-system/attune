from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="ApiResponseWebhookReceiverResponseData")


@_attrs_define
class ApiResponseWebhookReceiverResponseData:
    """Response from webhook receiver endpoint

    Attributes:
        event_id (int): ID of the event created from this webhook
        message (str): Success message
        received_at (datetime.datetime): Timestamp when the webhook was received
        trigger_ref (str): Reference of the trigger that received this webhook
    """

    event_id: int
    message: str
    received_at: datetime.datetime
    trigger_ref: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        event_id = self.event_id

        message = self.message

        received_at = self.received_at.isoformat()

        trigger_ref = self.trigger_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "event_id": event_id,
                "message": message,
                "received_at": received_at,
                "trigger_ref": trigger_ref,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        event_id = d.pop("event_id")

        message = d.pop("message")

        received_at = datetime.datetime.fromisoformat(d.pop("received_at"))

        trigger_ref = d.pop("trigger_ref")

        api_response_webhook_receiver_response_data = cls(
            event_id=event_id,
            message=message,
            received_at=received_at,
            trigger_ref=trigger_ref,
        )

        api_response_webhook_receiver_response_data.additional_properties = d
        return api_response_webhook_receiver_response_data

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
