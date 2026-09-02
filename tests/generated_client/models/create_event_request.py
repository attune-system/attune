from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_event_request_config import CreateEventRequestConfig
    from ..models.create_event_request_payload import CreateEventRequestPayload


T = TypeVar("T", bound="CreateEventRequest")


@_attrs_define
class CreateEventRequest:
    """Request body for creating an event

    Attributes:
        trigger_ref (str): Trigger reference (e.g., "core.timer", "core.webhook")
            Also accepts "trigger_type" for compatibility with the sensor interface spec. Example: core.timer.
        config (CreateEventRequestConfig | Unset): Event configuration
        payload (CreateEventRequestPayload | Unset): Event payload data
        trace_tag (None | str | Unset): Optional source trace tag for this event.
            When omitted for execution-token callers, inherits from the parent execution. Example: core.timer.1234.
        trigger_instance_id (None | str | Unset): Trigger instance ID (for correlation, often rule_id) Example:
            rule_123.
    """

    trigger_ref: str
    config: CreateEventRequestConfig | Unset = UNSET
    payload: CreateEventRequestPayload | Unset = UNSET
    trace_tag: None | str | Unset = UNSET
    trigger_instance_id: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        trigger_ref = self.trigger_ref

        config: dict[str, Any] | Unset = UNSET
        if not isinstance(self.config, Unset):
            config = self.config.to_dict()

        payload: dict[str, Any] | Unset = UNSET
        if not isinstance(self.payload, Unset):
            payload = self.payload.to_dict()

        trace_tag: None | str | Unset
        if isinstance(self.trace_tag, Unset):
            trace_tag = UNSET
        else:
            trace_tag = self.trace_tag

        trigger_instance_id: None | str | Unset
        if isinstance(self.trigger_instance_id, Unset):
            trigger_instance_id = UNSET
        else:
            trigger_instance_id = self.trigger_instance_id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "trigger_ref": trigger_ref,
            }
        )
        if config is not UNSET:
            field_dict["config"] = config
        if payload is not UNSET:
            field_dict["payload"] = payload
        if trace_tag is not UNSET:
            field_dict["trace_tag"] = trace_tag
        if trigger_instance_id is not UNSET:
            field_dict["trigger_instance_id"] = trigger_instance_id

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.create_event_request_config import (
            CreateEventRequestConfig,
        )
        from ..models.create_event_request_payload import (
            CreateEventRequestPayload,
        )

        d = dict(src_dict)
        trigger_ref = d.pop("trigger_ref")

        _config = d.pop("config", UNSET)
        config: CreateEventRequestConfig | Unset
        if isinstance(_config, Unset):
            config = UNSET
        else:
            config = CreateEventRequestConfig.from_dict(_config)

        _payload = d.pop("payload", UNSET)
        payload: CreateEventRequestPayload | Unset
        if isinstance(_payload, Unset):
            payload = UNSET
        else:
            payload = CreateEventRequestPayload.from_dict(_payload)

        def _parse_trace_tag(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        trace_tag = _parse_trace_tag(d.pop("trace_tag", UNSET))

        def _parse_trigger_instance_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        trigger_instance_id = _parse_trigger_instance_id(
            d.pop("trigger_instance_id", UNSET)
        )

        create_event_request = cls(
            trigger_ref=trigger_ref,
            config=config,
            payload=payload,
            trace_tag=trace_tag,
            trigger_instance_id=trigger_instance_id,
        )

        create_event_request.additional_properties = d
        return create_event_request

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
