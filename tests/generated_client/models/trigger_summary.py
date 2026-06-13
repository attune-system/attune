from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.action_reference_visibility import ActionReferenceVisibility
from ..types import UNSET, Unset

T = TypeVar("T", bound="TriggerSummary")


@_attrs_define
class TriggerSummary:
    """Simplified trigger response (for list endpoints)

    Attributes:
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        enabled (bool): Whether the trigger is enabled Example: True.
        id (int): Trigger ID Example: 1.
        label (str): Human-readable label Example: Webhook Trigger.
        ref (str): Unique reference identifier Example: core.webhook.
        reference_visibility (ActionReferenceVisibility):
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:30:00Z.
        webhook_enabled (bool): Whether webhooks are enabled for this trigger
        description (None | str | Unset): Trigger description Example: Triggers when a webhook is received.
        pack_ref (None | str | Unset): Pack reference (optional) Example: core.
        reference_allowed_pack_refs (list[str] | Unset): Pack refs allowed to subscribe to this trigger when visibility
            is restricted. Example: ['incident_response', 'deployments'].
    """

    created: datetime.datetime
    enabled: bool
    id: int
    label: str
    ref: str
    reference_visibility: ActionReferenceVisibility
    updated: datetime.datetime
    webhook_enabled: bool
    description: None | str | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    reference_allowed_pack_refs: list[str] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        enabled = self.enabled

        id = self.id

        label = self.label

        ref = self.ref

        reference_visibility = self.reference_visibility.value

        updated = self.updated.isoformat()

        webhook_enabled = self.webhook_enabled

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref

        reference_allowed_pack_refs: list[str] | Unset = UNSET
        if not isinstance(self.reference_allowed_pack_refs, Unset):
            reference_allowed_pack_refs = self.reference_allowed_pack_refs

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "enabled": enabled,
                "id": id,
                "label": label,
                "ref": ref,
                "reference_visibility": reference_visibility,
                "updated": updated,
                "webhook_enabled": webhook_enabled,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref
        if reference_allowed_pack_refs is not UNSET:
            field_dict["reference_allowed_pack_refs"] = reference_allowed_pack_refs

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        enabled = d.pop("enabled")

        id = d.pop("id")

        label = d.pop("label")

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

        trigger_summary = cls(
            created=created,
            enabled=enabled,
            id=id,
            label=label,
            ref=ref,
            reference_visibility=reference_visibility,
            updated=updated,
            webhook_enabled=webhook_enabled,
            description=description,
            pack_ref=pack_ref,
            reference_allowed_pack_refs=reference_allowed_pack_refs,
        )

        trigger_summary.additional_properties = d
        return trigger_summary

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
