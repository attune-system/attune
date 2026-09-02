from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.component_summary import ComponentSummary


T = TypeVar("T", bound="PackContents")


@_attrs_define
class PackContents:
    """Pack contents summary

    Attributes:
        actions (list[ComponentSummary]): List of actions
        rules (list[ComponentSummary]): List of bundled rules
        sensors (list[ComponentSummary]): List of sensors
        triggers (list[ComponentSummary]): List of triggers
        workflows (list[ComponentSummary]): List of bundled workflows
    """

    actions: list[ComponentSummary]
    rules: list[ComponentSummary]
    sensors: list[ComponentSummary]
    triggers: list[ComponentSummary]
    workflows: list[ComponentSummary]

    def to_dict(self) -> dict[str, Any]:
        actions = []
        for actions_item_data in self.actions:
            actions_item = actions_item_data.to_dict()
            actions.append(actions_item)

        rules = []
        for rules_item_data in self.rules:
            rules_item = rules_item_data.to_dict()
            rules.append(rules_item)

        sensors = []
        for sensors_item_data in self.sensors:
            sensors_item = sensors_item_data.to_dict()
            sensors.append(sensors_item)

        triggers = []
        for triggers_item_data in self.triggers:
            triggers_item = triggers_item_data.to_dict()
            triggers.append(triggers_item)

        workflows = []
        for workflows_item_data in self.workflows:
            workflows_item = workflows_item_data.to_dict()
            workflows.append(workflows_item)

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "actions": actions,
                "rules": rules,
                "sensors": sensors,
                "triggers": triggers,
                "workflows": workflows,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.component_summary import ComponentSummary

        d = dict(src_dict)
        actions = []
        _actions = d.pop("actions")
        for actions_item_data in _actions:
            actions_item = ComponentSummary.from_dict(actions_item_data)

            actions.append(actions_item)

        rules = []
        _rules = d.pop("rules")
        for rules_item_data in _rules:
            rules_item = ComponentSummary.from_dict(rules_item_data)

            rules.append(rules_item)

        sensors = []
        _sensors = d.pop("sensors")
        for sensors_item_data in _sensors:
            sensors_item = ComponentSummary.from_dict(sensors_item_data)

            sensors.append(sensors_item)

        triggers = []
        _triggers = d.pop("triggers")
        for triggers_item_data in _triggers:
            triggers_item = ComponentSummary.from_dict(triggers_item_data)

            triggers.append(triggers_item)

        workflows = []
        _workflows = d.pop("workflows")
        for workflows_item_data in _workflows:
            workflows_item = ComponentSummary.from_dict(workflows_item_data)

            workflows.append(workflows_item)

        pack_contents = cls(
            actions=actions,
            rules=rules,
            sensors=sensors,
            triggers=triggers,
            workflows=workflows,
        )

        return pack_contents
