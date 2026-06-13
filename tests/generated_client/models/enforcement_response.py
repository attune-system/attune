from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.enforcement_condition import EnforcementCondition
from ..models.enforcement_status import EnforcementStatus
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.enforcement_response_conditions import EnforcementResponseConditions
    from ..models.enforcement_response_config_type_0 import (
        EnforcementResponseConfigType0,
    )
    from ..models.enforcement_response_payload import EnforcementResponsePayload


T = TypeVar("T", bound="EnforcementResponse")


@_attrs_define
class EnforcementResponse:
    """Full enforcement response with all details

    Attributes:
        condition (EnforcementCondition):
        conditions (EnforcementResponseConditions): Enforcement conditions (rule evaluation criteria)
        config (EnforcementResponseConfigType0 | None): Enforcement configuration
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        id (int):
        payload (EnforcementResponsePayload): Enforcement payload
        rule_ref (str): Rule reference Example: slack.notify_on_error.
        status (EnforcementStatus):
        trigger_ref (str): Trigger reference Example: system.error_event.
        event (int | None | Unset):
        resolved_at (datetime.datetime | None | Unset): Timestamp when the enforcement was resolved (status changed from
            created to processed/disabled) Example: 2024-01-13T10:30:01Z.
        rule (int | None | Unset):
    """

    condition: EnforcementCondition
    conditions: EnforcementResponseConditions
    config: EnforcementResponseConfigType0 | None
    created: datetime.datetime
    id: int
    payload: EnforcementResponsePayload
    rule_ref: str
    status: EnforcementStatus
    trigger_ref: str
    event: int | None | Unset = UNSET
    resolved_at: datetime.datetime | None | Unset = UNSET
    rule: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.enforcement_response_config_type_0 import (
            EnforcementResponseConfigType0,
        )

        condition = self.condition.value

        conditions = self.conditions.to_dict()

        config: dict[str, Any] | None
        if isinstance(self.config, EnforcementResponseConfigType0):
            config = self.config.to_dict()
        else:
            config = self.config

        created = self.created.isoformat()

        id = self.id

        payload = self.payload.to_dict()

        rule_ref = self.rule_ref

        status = self.status.value

        trigger_ref = self.trigger_ref

        event: int | None | Unset
        if isinstance(self.event, Unset):
            event = UNSET
        else:
            event = self.event

        resolved_at: None | str | Unset
        if isinstance(self.resolved_at, Unset):
            resolved_at = UNSET
        elif isinstance(self.resolved_at, datetime.datetime):
            resolved_at = self.resolved_at.isoformat()
        else:
            resolved_at = self.resolved_at

        rule: int | None | Unset
        if isinstance(self.rule, Unset):
            rule = UNSET
        else:
            rule = self.rule

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "condition": condition,
                "conditions": conditions,
                "config": config,
                "created": created,
                "id": id,
                "payload": payload,
                "rule_ref": rule_ref,
                "status": status,
                "trigger_ref": trigger_ref,
            }
        )
        if event is not UNSET:
            field_dict["event"] = event
        if resolved_at is not UNSET:
            field_dict["resolved_at"] = resolved_at
        if rule is not UNSET:
            field_dict["rule"] = rule

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.enforcement_response_conditions import (
            EnforcementResponseConditions,
        )
        from ..models.enforcement_response_config_type_0 import (
            EnforcementResponseConfigType0,
        )
        from ..models.enforcement_response_payload import EnforcementResponsePayload

        d = dict(src_dict)
        condition = EnforcementCondition(d.pop("condition"))

        conditions = EnforcementResponseConditions.from_dict(d.pop("conditions"))

        def _parse_config(data: object) -> EnforcementResponseConfigType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                config_type_0 = EnforcementResponseConfigType0.from_dict(data)

                return config_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(EnforcementResponseConfigType0 | None, data)

        config = _parse_config(d.pop("config"))

        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        payload = EnforcementResponsePayload.from_dict(d.pop("payload"))

        rule_ref = d.pop("rule_ref")

        status = EnforcementStatus(d.pop("status"))

        trigger_ref = d.pop("trigger_ref")

        def _parse_event(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        event = _parse_event(d.pop("event", UNSET))

        def _parse_resolved_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                resolved_at_type_0 = datetime.datetime.fromisoformat(data)

                return resolved_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        resolved_at = _parse_resolved_at(d.pop("resolved_at", UNSET))

        def _parse_rule(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        rule = _parse_rule(d.pop("rule", UNSET))

        enforcement_response = cls(
            condition=condition,
            conditions=conditions,
            config=config,
            created=created,
            id=id,
            payload=payload,
            rule_ref=rule_ref,
            status=status,
            trigger_ref=trigger_ref,
            event=event,
            resolved_at=resolved_at,
            rule=rule,
        )

        enforcement_response.additional_properties = d
        return enforcement_response

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
