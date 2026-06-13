from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.rule_response_action_params import RuleResponseActionParams
    from ..models.rule_response_conditions import RuleResponseConditions
    from ..models.rule_response_trigger_params import RuleResponseTriggerParams


T = TypeVar("T", bound="RuleResponse")


@_attrs_define
class RuleResponse:
    """Response DTO for rule information

    Attributes:
        action_params (RuleResponseActionParams): Parameters to pass to the action when rule is triggered
        action_ref (str): Action reference Example: slack.post_message.
        conditions (RuleResponseConditions): Conditions for rule evaluation
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        enabled (bool): Whether the rule is enabled Example: True.
        id (int): Rule ID Example: 1.
        is_adhoc (bool): Whether this is an ad-hoc rule (not from pack installation)
        label (str): Human-readable label Example: Notify on Error.
        pack (int): Pack ID Example: 1.
        pack_ref (str): Pack reference Example: slack.
        ref (str): Unique reference identifier Example: slack.notify_on_error.
        trigger_params (RuleResponseTriggerParams): Parameters for trigger configuration and event filtering
        trigger_ref (str): Trigger reference Example: system.error_event.
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:30:00Z.
        action (int | None | Unset): Action ID (null if the referenced action has been deleted) Example: 1.
        description (None | str | Unset): Rule description Example: Send Slack notification when an error occurs.
        owner_identity (int | None | Unset): Identity that registered the rule. NULL for system-loaded rules. Example:
            1.
        permission_set_refs (list[str] | None | Unset): Optional execution permission override. Null means inherit
            action default;
            empty array means force no execution API token. Example: ['core.agent_reader'].
        trigger (int | None | Unset): Trigger ID (null if the referenced trigger has been deleted) Example: 1.
    """

    action_params: RuleResponseActionParams
    action_ref: str
    conditions: RuleResponseConditions
    created: datetime.datetime
    enabled: bool
    id: int
    is_adhoc: bool
    label: str
    pack: int
    pack_ref: str
    ref: str
    trigger_params: RuleResponseTriggerParams
    trigger_ref: str
    updated: datetime.datetime
    action: int | None | Unset = UNSET
    description: None | str | Unset = UNSET
    owner_identity: int | None | Unset = UNSET
    permission_set_refs: list[str] | None | Unset = UNSET
    trigger: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        action_params = self.action_params.to_dict()

        action_ref = self.action_ref

        conditions = self.conditions.to_dict()

        created = self.created.isoformat()

        enabled = self.enabled

        id = self.id

        is_adhoc = self.is_adhoc

        label = self.label

        pack = self.pack

        pack_ref = self.pack_ref

        ref = self.ref

        trigger_params = self.trigger_params.to_dict()

        trigger_ref = self.trigger_ref

        updated = self.updated.isoformat()

        action: int | None | Unset
        if isinstance(self.action, Unset):
            action = UNSET
        else:
            action = self.action

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        owner_identity: int | None | Unset
        if isinstance(self.owner_identity, Unset):
            owner_identity = UNSET
        else:
            owner_identity = self.owner_identity

        permission_set_refs: list[str] | None | Unset
        if isinstance(self.permission_set_refs, Unset):
            permission_set_refs = UNSET
        elif isinstance(self.permission_set_refs, list):
            permission_set_refs = self.permission_set_refs

        else:
            permission_set_refs = self.permission_set_refs

        trigger: int | None | Unset
        if isinstance(self.trigger, Unset):
            trigger = UNSET
        else:
            trigger = self.trigger

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "action_params": action_params,
                "action_ref": action_ref,
                "conditions": conditions,
                "created": created,
                "enabled": enabled,
                "id": id,
                "is_adhoc": is_adhoc,
                "label": label,
                "pack": pack,
                "pack_ref": pack_ref,
                "ref": ref,
                "trigger_params": trigger_params,
                "trigger_ref": trigger_ref,
                "updated": updated,
            }
        )
        if action is not UNSET:
            field_dict["action"] = action
        if description is not UNSET:
            field_dict["description"] = description
        if owner_identity is not UNSET:
            field_dict["owner_identity"] = owner_identity
        if permission_set_refs is not UNSET:
            field_dict["permission_set_refs"] = permission_set_refs
        if trigger is not UNSET:
            field_dict["trigger"] = trigger

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.rule_response_action_params import RuleResponseActionParams
        from ..models.rule_response_conditions import RuleResponseConditions
        from ..models.rule_response_trigger_params import RuleResponseTriggerParams

        d = dict(src_dict)
        action_params = RuleResponseActionParams.from_dict(d.pop("action_params"))

        action_ref = d.pop("action_ref")

        conditions = RuleResponseConditions.from_dict(d.pop("conditions"))

        created = datetime.datetime.fromisoformat(d.pop("created"))

        enabled = d.pop("enabled")

        id = d.pop("id")

        is_adhoc = d.pop("is_adhoc")

        label = d.pop("label")

        pack = d.pop("pack")

        pack_ref = d.pop("pack_ref")

        ref = d.pop("ref")

        trigger_params = RuleResponseTriggerParams.from_dict(d.pop("trigger_params"))

        trigger_ref = d.pop("trigger_ref")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_action(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        action = _parse_action(d.pop("action", UNSET))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_owner_identity(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        owner_identity = _parse_owner_identity(d.pop("owner_identity", UNSET))

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

        def _parse_trigger(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        trigger = _parse_trigger(d.pop("trigger", UNSET))

        rule_response = cls(
            action_params=action_params,
            action_ref=action_ref,
            conditions=conditions,
            created=created,
            enabled=enabled,
            id=id,
            is_adhoc=is_adhoc,
            label=label,
            pack=pack,
            pack_ref=pack_ref,
            ref=ref,
            trigger_params=trigger_params,
            trigger_ref=trigger_ref,
            updated=updated,
            action=action,
            description=description,
            owner_identity=owner_identity,
            permission_set_refs=permission_set_refs,
            trigger=trigger,
        )

        rule_response.additional_properties = d
        return rule_response

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
