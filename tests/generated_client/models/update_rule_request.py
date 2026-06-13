from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.update_rule_request_action_params_type_0 import (
        UpdateRuleRequestActionParamsType0,
    )
    from ..models.update_rule_request_conditions_type_0 import (
        UpdateRuleRequestConditionsType0,
    )
    from ..models.update_rule_request_trigger_params_type_0 import (
        UpdateRuleRequestTriggerParamsType0,
    )


T = TypeVar("T", bound="UpdateRuleRequest")


@_attrs_define
class UpdateRuleRequest:
    """Request DTO for updating a rule

    Attributes:
        action_params (None | UpdateRuleRequestActionParamsType0): Parameters to pass to the action when rule is
            triggered
        conditions (None | UpdateRuleRequestConditionsType0): Conditions for rule evaluation
        trigger_params (None | UpdateRuleRequestTriggerParamsType0): Parameters for trigger configuration and event
            filtering
        action_ref (None | str | Unset): Action reference to execute when rule matches Example: slack.post_message.
        description (None | str | Unset): Rule description Example: Enhanced error notification with filtering.
        enabled (bool | None | Unset): Whether the rule is enabled
        label (None | str | Unset): Human-readable label Example: Notify on Error (Updated).
        permission_set_refs (list[str] | None | Unset): Permission set refs to apply to executions created by this rule.
            Omit to
            keep the current value. Provide null to inherit the action default, or an
            empty array to force no API token. Example: ['core.agent_reader'].
        trigger_ref (None | str | Unset): Trigger reference that activates this rule Example: system.error_event.
    """

    action_params: None | UpdateRuleRequestActionParamsType0
    conditions: None | UpdateRuleRequestConditionsType0
    trigger_params: None | UpdateRuleRequestTriggerParamsType0
    action_ref: None | str | Unset = UNSET
    description: None | str | Unset = UNSET
    enabled: bool | None | Unset = UNSET
    label: None | str | Unset = UNSET
    permission_set_refs: list[str] | None | Unset = UNSET
    trigger_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.update_rule_request_action_params_type_0 import (
            UpdateRuleRequestActionParamsType0,
        )
        from ..models.update_rule_request_conditions_type_0 import (
            UpdateRuleRequestConditionsType0,
        )
        from ..models.update_rule_request_trigger_params_type_0 import (
            UpdateRuleRequestTriggerParamsType0,
        )

        action_params: dict[str, Any] | None
        if isinstance(self.action_params, UpdateRuleRequestActionParamsType0):
            action_params = self.action_params.to_dict()
        else:
            action_params = self.action_params

        conditions: dict[str, Any] | None
        if isinstance(self.conditions, UpdateRuleRequestConditionsType0):
            conditions = self.conditions.to_dict()
        else:
            conditions = self.conditions

        trigger_params: dict[str, Any] | None
        if isinstance(self.trigger_params, UpdateRuleRequestTriggerParamsType0):
            trigger_params = self.trigger_params.to_dict()
        else:
            trigger_params = self.trigger_params

        action_ref: None | str | Unset
        if isinstance(self.action_ref, Unset):
            action_ref = UNSET
        else:
            action_ref = self.action_ref

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        enabled: bool | None | Unset
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

        label: None | str | Unset
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        permission_set_refs: list[str] | None | Unset
        if isinstance(self.permission_set_refs, Unset):
            permission_set_refs = UNSET
        elif isinstance(self.permission_set_refs, list):
            permission_set_refs = self.permission_set_refs

        else:
            permission_set_refs = self.permission_set_refs

        trigger_ref: None | str | Unset
        if isinstance(self.trigger_ref, Unset):
            trigger_ref = UNSET
        else:
            trigger_ref = self.trigger_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "action_params": action_params,
                "conditions": conditions,
                "trigger_params": trigger_params,
            }
        )
        if action_ref is not UNSET:
            field_dict["action_ref"] = action_ref
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if label is not UNSET:
            field_dict["label"] = label
        if permission_set_refs is not UNSET:
            field_dict["permission_set_refs"] = permission_set_refs
        if trigger_ref is not UNSET:
            field_dict["trigger_ref"] = trigger_ref

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.update_rule_request_action_params_type_0 import (
            UpdateRuleRequestActionParamsType0,
        )
        from ..models.update_rule_request_conditions_type_0 import (
            UpdateRuleRequestConditionsType0,
        )
        from ..models.update_rule_request_trigger_params_type_0 import (
            UpdateRuleRequestTriggerParamsType0,
        )

        d = dict(src_dict)

        def _parse_action_params(
            data: object,
        ) -> None | UpdateRuleRequestActionParamsType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                action_params_type_0 = UpdateRuleRequestActionParamsType0.from_dict(
                    data
                )

                return action_params_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateRuleRequestActionParamsType0, data)

        action_params = _parse_action_params(d.pop("action_params"))

        def _parse_conditions(data: object) -> None | UpdateRuleRequestConditionsType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                conditions_type_0 = UpdateRuleRequestConditionsType0.from_dict(data)

                return conditions_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateRuleRequestConditionsType0, data)

        conditions = _parse_conditions(d.pop("conditions"))

        def _parse_trigger_params(
            data: object,
        ) -> None | UpdateRuleRequestTriggerParamsType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                trigger_params_type_0 = UpdateRuleRequestTriggerParamsType0.from_dict(
                    data
                )

                return trigger_params_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateRuleRequestTriggerParamsType0, data)

        trigger_params = _parse_trigger_params(d.pop("trigger_params"))

        def _parse_action_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        action_ref = _parse_action_ref(d.pop("action_ref", UNSET))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_enabled(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        enabled = _parse_enabled(d.pop("enabled", UNSET))

        def _parse_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_permission_set_refs(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                permission_set_refs_type_0 = cast(list[str], data)

                return permission_set_refs_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        permission_set_refs = _parse_permission_set_refs(
            d.pop("permission_set_refs", UNSET)
        )

        def _parse_trigger_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        trigger_ref = _parse_trigger_ref(d.pop("trigger_ref", UNSET))

        update_rule_request = cls(
            action_params=action_params,
            conditions=conditions,
            trigger_params=trigger_params,
            action_ref=action_ref,
            description=description,
            enabled=enabled,
            label=label,
            permission_set_refs=permission_set_refs,
            trigger_ref=trigger_ref,
        )

        update_rule_request.additional_properties = d
        return update_rule_request

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
