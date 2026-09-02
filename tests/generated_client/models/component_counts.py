from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="ComponentCounts")


@_attrs_define
class ComponentCounts:
    """Component counts

    Attributes:
        actions (int): Number of actions
        policies (int): Number of policies
        rules (int): Number of rules
        sensors (int): Number of sensors
        triggers (int): Number of triggers
        workflows (int): Number of workflows
    """

    actions: int
    policies: int
    rules: int
    sensors: int
    triggers: int
    workflows: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        actions = self.actions

        policies = self.policies

        rules = self.rules

        sensors = self.sensors

        triggers = self.triggers

        workflows = self.workflows

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "actions": actions,
                "policies": policies,
                "rules": rules,
                "sensors": sensors,
                "triggers": triggers,
                "workflows": workflows,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        actions = d.pop("actions")

        policies = d.pop("policies")

        rules = d.pop("rules")

        sensors = d.pop("sensors")

        triggers = d.pop("triggers")

        workflows = d.pop("workflows")

        component_counts = cls(
            actions=actions,
            policies=policies,
            rules=rules,
            sensors=sensors,
            triggers=triggers,
            workflows=workflows,
        )

        component_counts.additional_properties = d
        return component_counts

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
