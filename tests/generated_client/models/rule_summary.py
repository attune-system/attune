from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.rule_summary_action_params import RuleSummaryActionParams
    from ..models.rule_summary_trigger_params import RuleSummaryTriggerParams


T = TypeVar("T", bound="RuleSummary")


@_attrs_define
class RuleSummary:
    """Simplified rule response (for list endpoints)

    Attributes:
        action_params (RuleSummaryActionParams): Parameters to pass to the action when rule is triggered
        action_ref (str): Action reference Example: slack.post_message.
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        enabled (bool): Whether the rule is enabled Example: True.
        id (int): Rule ID Example: 1.
        label (str): Human-readable label Example: Notify on Error.
        pack_ref (str): Pack reference Example: slack.
        ref (str): Unique reference identifier Example: slack.notify_on_error.
        trigger_params (RuleSummaryTriggerParams): Parameters for trigger configuration and event filtering
        trigger_ref (str): Trigger reference Example: system.error_event.
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:30:00Z.
        description (None | str | Unset): Rule description Example: Send Slack notification when an error occurs.
        permission_set_refs (list[str] | None | Unset): Optional execution permission override. Null means inherit
            action default;
            empty array means force no execution API token. Example: ['core.agent_reader'].
    """

    action_params: RuleSummaryActionParams
    action_ref: str
    created: datetime.datetime
    enabled: bool
    id: int
    label: str
    pack_ref: str
    ref: str
    trigger_params: RuleSummaryTriggerParams
    trigger_ref: str
    updated: datetime.datetime
    description: None | str | Unset = UNSET
    permission_set_refs: list[str] | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        action_params = self.action_params.to_dict()

        action_ref = self.action_ref

        created = self.created.isoformat()

        enabled = self.enabled

        id = self.id

        label = self.label

        pack_ref = self.pack_ref

        ref = self.ref

        trigger_params = self.trigger_params.to_dict()

        trigger_ref = self.trigger_ref

        updated = self.updated.isoformat()

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        permission_set_refs: list[str] | None | Unset
        if isinstance(self.permission_set_refs, Unset):
            permission_set_refs = UNSET
        elif isinstance(self.permission_set_refs, list):
            permission_set_refs = self.permission_set_refs

        else:
            permission_set_refs = self.permission_set_refs

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "action_params": action_params,
                "action_ref": action_ref,
                "created": created,
                "enabled": enabled,
                "id": id,
                "label": label,
                "pack_ref": pack_ref,
                "ref": ref,
                "trigger_params": trigger_params,
                "trigger_ref": trigger_ref,
                "updated": updated,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if permission_set_refs is not UNSET:
            field_dict["permission_set_refs"] = permission_set_refs

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.rule_summary_action_params import RuleSummaryActionParams
        from ..models.rule_summary_trigger_params import RuleSummaryTriggerParams

        d = dict(src_dict)
        action_params = RuleSummaryActionParams.from_dict(d.pop("action_params"))

        action_ref = d.pop("action_ref")

        created = datetime.datetime.fromisoformat(d.pop("created"))

        enabled = d.pop("enabled")

        id = d.pop("id")

        label = d.pop("label")

        pack_ref = d.pop("pack_ref")

        ref = d.pop("ref")

        trigger_params = RuleSummaryTriggerParams.from_dict(d.pop("trigger_params"))

        trigger_ref = d.pop("trigger_ref")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

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

        rule_summary = cls(
            action_params=action_params,
            action_ref=action_ref,
            created=created,
            enabled=enabled,
            id=id,
            label=label,
            pack_ref=pack_ref,
            ref=ref,
            trigger_params=trigger_params,
            trigger_ref=trigger_ref,
            updated=updated,
            description=description,
            permission_set_refs=permission_set_refs,
        )

        rule_summary.additional_properties = d
        return rule_summary

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
