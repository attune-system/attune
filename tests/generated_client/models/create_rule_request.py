from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_rule_request_action_params import CreateRuleRequestActionParams
    from ..models.create_rule_request_conditions import CreateRuleRequestConditions
    from ..models.create_rule_request_trigger_params import (
        CreateRuleRequestTriggerParams,
    )


T = TypeVar("T", bound="CreateRuleRequest")


@_attrs_define
class CreateRuleRequest:
    """Request DTO for creating a new rule

    Attributes:
        action_ref (str): Action reference to execute when rule matches Example: slack.post_message.
        label (str): Human-readable label Example: Notify on Error.
        pack_ref (str): Pack reference this rule belongs to Example: slack.
        ref (str): Unique reference identifier (e.g., "mypack.notify_on_error") Example: slack.notify_on_error.
        trigger_ref (str): Trigger reference that activates this rule Example: system.error_event.
        action_params (CreateRuleRequestActionParams | Unset): Parameters to pass to the action when rule is triggered
        conditions (CreateRuleRequestConditions | Unset): Conditions for rule evaluation (JSON Logic or custom format)
        description (None | str | Unset): Rule description Example: Send Slack notification when an error occurs.
        enabled (bool | Unset): Whether the rule is enabled Example: True.
        permission_set_refs (list[str] | None | Unset): Permission set refs to apply to executions created by this rule.
            Omit to
            inherit the action default. Provide an empty array to force no API token. Example: ['core.agent_reader'].
        trigger_params (CreateRuleRequestTriggerParams | Unset): Parameters for trigger configuration and event
            filtering
    """

    action_ref: str
    label: str
    pack_ref: str
    ref: str
    trigger_ref: str
    action_params: CreateRuleRequestActionParams | Unset = UNSET
    conditions: CreateRuleRequestConditions | Unset = UNSET
    description: None | str | Unset = UNSET
    enabled: bool | Unset = UNSET
    permission_set_refs: list[str] | None | Unset = UNSET
    trigger_params: CreateRuleRequestTriggerParams | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        action_ref = self.action_ref

        label = self.label

        pack_ref = self.pack_ref

        ref = self.ref

        trigger_ref = self.trigger_ref

        action_params: dict[str, Any] | Unset = UNSET
        if not isinstance(self.action_params, Unset):
            action_params = self.action_params.to_dict()

        conditions: dict[str, Any] | Unset = UNSET
        if not isinstance(self.conditions, Unset):
            conditions = self.conditions.to_dict()

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        enabled = self.enabled

        permission_set_refs: list[str] | None | Unset
        if isinstance(self.permission_set_refs, Unset):
            permission_set_refs = UNSET
        elif isinstance(self.permission_set_refs, list):
            permission_set_refs = self.permission_set_refs

        else:
            permission_set_refs = self.permission_set_refs

        trigger_params: dict[str, Any] | Unset = UNSET
        if not isinstance(self.trigger_params, Unset):
            trigger_params = self.trigger_params.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "action_ref": action_ref,
                "label": label,
                "pack_ref": pack_ref,
                "ref": ref,
                "trigger_ref": trigger_ref,
            }
        )
        if action_params is not UNSET:
            field_dict["action_params"] = action_params
        if conditions is not UNSET:
            field_dict["conditions"] = conditions
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if permission_set_refs is not UNSET:
            field_dict["permission_set_refs"] = permission_set_refs
        if trigger_params is not UNSET:
            field_dict["trigger_params"] = trigger_params

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.create_rule_request_action_params import (
            CreateRuleRequestActionParams,
        )
        from ..models.create_rule_request_conditions import CreateRuleRequestConditions
        from ..models.create_rule_request_trigger_params import (
            CreateRuleRequestTriggerParams,
        )

        d = dict(src_dict)
        action_ref = d.pop("action_ref")

        label = d.pop("label")

        pack_ref = d.pop("pack_ref")

        ref = d.pop("ref")

        trigger_ref = d.pop("trigger_ref")

        _action_params = d.pop("action_params", UNSET)
        action_params: CreateRuleRequestActionParams | Unset
        if isinstance(_action_params, Unset):
            action_params = UNSET
        else:
            action_params = CreateRuleRequestActionParams.from_dict(_action_params)

        _conditions = d.pop("conditions", UNSET)
        conditions: CreateRuleRequestConditions | Unset
        if isinstance(_conditions, Unset):
            conditions = UNSET
        else:
            conditions = CreateRuleRequestConditions.from_dict(_conditions)

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        enabled = d.pop("enabled", UNSET)

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

        _trigger_params = d.pop("trigger_params", UNSET)
        trigger_params: CreateRuleRequestTriggerParams | Unset
        if isinstance(_trigger_params, Unset):
            trigger_params = UNSET
        else:
            trigger_params = CreateRuleRequestTriggerParams.from_dict(_trigger_params)

        create_rule_request = cls(
            action_ref=action_ref,
            label=label,
            pack_ref=pack_ref,
            ref=ref,
            trigger_ref=trigger_ref,
            action_params=action_params,
            conditions=conditions,
            description=description,
            enabled=enabled,
            permission_set_refs=permission_set_refs,
            trigger_params=trigger_params,
        )

        create_rule_request.additional_properties = d
        return create_rule_request

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
