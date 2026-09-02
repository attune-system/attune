from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.enforcement_condition import EnforcementCondition
from ..models.enforcement_status import EnforcementStatus
from ..types import UNSET, Unset

T = TypeVar("T", bound="TraceEnforcementSummary")


@_attrs_define
class TraceEnforcementSummary:
    """
    Attributes:
        condition (EnforcementCondition):
        created (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
        id (int):
        rule_ref (str):  Example: core.on_timer.
        status (EnforcementStatus):
        trigger_ref (str):  Example: core.timer.
        event (int | None | Unset):
        resolved_at (datetime.datetime | None | Unset):  Example: 2024-01-13T10:31:00Z.
        rule (int | None | Unset):
    """

    condition: EnforcementCondition
    created: datetime.datetime
    id: int
    rule_ref: str
    status: EnforcementStatus
    trigger_ref: str
    event: int | None | Unset = UNSET
    resolved_at: datetime.datetime | None | Unset = UNSET
    rule: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        condition = self.condition.value

        created = self.created.isoformat()

        id = self.id

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
                "created": created,
                "id": id,
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
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        condition = EnforcementCondition(d.pop("condition"))

        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

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

        trace_enforcement_summary = cls(
            condition=condition,
            created=created,
            id=id,
            rule_ref=rule_ref,
            status=status,
            trigger_ref=trigger_ref,
            event=event,
            resolved_at=resolved_at,
            rule=rule,
        )

        trace_enforcement_summary.additional_properties = d
        return trace_enforcement_summary

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
