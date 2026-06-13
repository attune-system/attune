from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.action_reference_visibility import ActionReferenceVisibility
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.trigger_response_out_schema_type_0 import (
        TriggerResponseOutSchemaType0,
    )
    from ..models.trigger_response_param_schema_type_0 import (
        TriggerResponseParamSchemaType0,
    )


T = TypeVar("T", bound="TriggerResponse")


@_attrs_define
class TriggerResponse:
    """Response DTO for trigger information

    Attributes:
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        enabled (bool): Whether the trigger is enabled Example: True.
        id (int): Trigger ID Example: 1.
        is_adhoc (bool): Whether this is an ad-hoc trigger (not from pack installation)
        label (str): Human-readable label Example: Webhook Trigger.
        out_schema (None | TriggerResponseOutSchemaType0): Output schema
        param_schema (None | TriggerResponseParamSchemaType0): Parameter schema (StackStorm-style with inline
            required/secret)
        ref (str): Unique reference identifier Example: core.webhook.
        reference_visibility (ActionReferenceVisibility):
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:30:00Z.
        webhook_enabled (bool): Whether webhooks are enabled for this trigger
        description (None | str | Unset): Trigger description Example: Triggers when a webhook is received.
        pack (int | None | Unset): Pack ID (optional) Example: 1.
        pack_ref (None | str | Unset): Pack reference (optional) Example: core.
        reference_allowed_pack_refs (list[str] | Unset): Pack refs allowed to subscribe to this trigger when visibility
            is restricted. Example: ['incident_response', 'deployments'].
        sensor (int | None | Unset): Sensor ID (optional — webhook triggers have no sensor) Example: 1.
        sensor_ref (None | str | Unset): Sensor reference (optional) Example: core.timer_sensor.
        webhook_key (None | str | Unset): Webhook key (only present if webhooks are enabled) Example:
            wh_k7j2n9p4m8q1r5w3x6z0a2b5c8d1e4f7g9h2.
    """

    created: datetime.datetime
    enabled: bool
    id: int
    is_adhoc: bool
    label: str
    out_schema: None | TriggerResponseOutSchemaType0
    param_schema: None | TriggerResponseParamSchemaType0
    ref: str
    reference_visibility: ActionReferenceVisibility
    updated: datetime.datetime
    webhook_enabled: bool
    description: None | str | Unset = UNSET
    pack: int | None | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    reference_allowed_pack_refs: list[str] | Unset = UNSET
    sensor: int | None | Unset = UNSET
    sensor_ref: None | str | Unset = UNSET
    webhook_key: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.trigger_response_out_schema_type_0 import (
            TriggerResponseOutSchemaType0,
        )
        from ..models.trigger_response_param_schema_type_0 import (
            TriggerResponseParamSchemaType0,
        )

        created = self.created.isoformat()

        enabled = self.enabled

        id = self.id

        is_adhoc = self.is_adhoc

        label = self.label

        out_schema: dict[str, Any] | None
        if isinstance(self.out_schema, TriggerResponseOutSchemaType0):
            out_schema = self.out_schema.to_dict()
        else:
            out_schema = self.out_schema

        param_schema: dict[str, Any] | None
        if isinstance(self.param_schema, TriggerResponseParamSchemaType0):
            param_schema = self.param_schema.to_dict()
        else:
            param_schema = self.param_schema

        ref = self.ref

        reference_visibility = self.reference_visibility.value

        updated = self.updated.isoformat()

        webhook_enabled = self.webhook_enabled

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        pack: int | None | Unset
        if isinstance(self.pack, Unset):
            pack = UNSET
        else:
            pack = self.pack

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref

        reference_allowed_pack_refs: list[str] | Unset = UNSET
        if not isinstance(self.reference_allowed_pack_refs, Unset):
            reference_allowed_pack_refs = self.reference_allowed_pack_refs

        sensor: int | None | Unset
        if isinstance(self.sensor, Unset):
            sensor = UNSET
        else:
            sensor = self.sensor

        sensor_ref: None | str | Unset
        if isinstance(self.sensor_ref, Unset):
            sensor_ref = UNSET
        else:
            sensor_ref = self.sensor_ref

        webhook_key: None | str | Unset
        if isinstance(self.webhook_key, Unset):
            webhook_key = UNSET
        else:
            webhook_key = self.webhook_key

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "enabled": enabled,
                "id": id,
                "is_adhoc": is_adhoc,
                "label": label,
                "out_schema": out_schema,
                "param_schema": param_schema,
                "ref": ref,
                "reference_visibility": reference_visibility,
                "updated": updated,
                "webhook_enabled": webhook_enabled,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if pack is not UNSET:
            field_dict["pack"] = pack
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref
        if reference_allowed_pack_refs is not UNSET:
            field_dict["reference_allowed_pack_refs"] = reference_allowed_pack_refs
        if sensor is not UNSET:
            field_dict["sensor"] = sensor
        if sensor_ref is not UNSET:
            field_dict["sensor_ref"] = sensor_ref
        if webhook_key is not UNSET:
            field_dict["webhook_key"] = webhook_key

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.trigger_response_out_schema_type_0 import (
            TriggerResponseOutSchemaType0,
        )
        from ..models.trigger_response_param_schema_type_0 import (
            TriggerResponseParamSchemaType0,
        )

        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        enabled = d.pop("enabled")

        id = d.pop("id")

        is_adhoc = d.pop("is_adhoc")

        label = d.pop("label")

        def _parse_out_schema(data: object) -> None | TriggerResponseOutSchemaType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                out_schema_type_0 = TriggerResponseOutSchemaType0.from_dict(data)

                return out_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | TriggerResponseOutSchemaType0, data)

        out_schema = _parse_out_schema(d.pop("out_schema"))

        def _parse_param_schema(data: object) -> None | TriggerResponseParamSchemaType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                param_schema_type_0 = TriggerResponseParamSchemaType0.from_dict(data)

                return param_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | TriggerResponseParamSchemaType0, data)

        param_schema = _parse_param_schema(d.pop("param_schema"))

        ref = d.pop("ref")

        reference_visibility = ActionReferenceVisibility(d.pop("reference_visibility"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        webhook_enabled = d.pop("webhook_enabled")

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_pack(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        pack = _parse_pack(d.pop("pack", UNSET))

        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))

        reference_allowed_pack_refs = cast(
            list[str], d.pop("reference_allowed_pack_refs", UNSET)
        )

        def _parse_sensor(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        sensor = _parse_sensor(d.pop("sensor", UNSET))

        def _parse_sensor_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        sensor_ref = _parse_sensor_ref(d.pop("sensor_ref", UNSET))

        def _parse_webhook_key(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        webhook_key = _parse_webhook_key(d.pop("webhook_key", UNSET))

        trigger_response = cls(
            created=created,
            enabled=enabled,
            id=id,
            is_adhoc=is_adhoc,
            label=label,
            out_schema=out_schema,
            param_schema=param_schema,
            ref=ref,
            reference_visibility=reference_visibility,
            updated=updated,
            webhook_enabled=webhook_enabled,
            description=description,
            pack=pack,
            pack_ref=pack_ref,
            reference_allowed_pack_refs=reference_allowed_pack_refs,
            sensor=sensor,
            sensor_ref=sensor_ref,
            webhook_key=webhook_key,
        )

        trigger_response.additional_properties = d
        return trigger_response

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
