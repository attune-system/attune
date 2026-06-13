from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.action_reference_visibility import ActionReferenceVisibility
from ..types import UNSET, Unset

T = TypeVar("T", bound="WorkQueueSummary")


@_attrs_define
class WorkQueueSummary:
    """
    Attributes:
        accepting_new_items (bool):  Example: True.
        created (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
        dispatch_action_ref (str):  Example: core.process_item.
        enabled (bool):  Example: True.
        id (int):
        is_adhoc (bool):
        label (str):  Example: Core Inbox.
        ref (str):  Example: core.inbox.
        reference_allowed_pack_refs (list[str]):  Example: ['incident_response', 'deployments'].
        reference_visibility (ActionReferenceVisibility):
        updated (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
        description (None | str | Unset):  Example: Dispatches inbound work items to the core processor.
        pack_ref (None | str | Unset):  Example: core.
    """

    accepting_new_items: bool
    created: datetime.datetime
    dispatch_action_ref: str
    enabled: bool
    id: int
    is_adhoc: bool
    label: str
    ref: str
    reference_allowed_pack_refs: list[str]
    reference_visibility: ActionReferenceVisibility
    updated: datetime.datetime
    description: None | str | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        accepting_new_items = self.accepting_new_items

        created = self.created.isoformat()

        dispatch_action_ref = self.dispatch_action_ref

        enabled = self.enabled

        id = self.id

        is_adhoc = self.is_adhoc

        label = self.label

        ref = self.ref

        reference_allowed_pack_refs = self.reference_allowed_pack_refs

        reference_visibility = self.reference_visibility.value

        updated = self.updated.isoformat()

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

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "accepting_new_items": accepting_new_items,
                "created": created,
                "dispatch_action_ref": dispatch_action_ref,
                "enabled": enabled,
                "id": id,
                "is_adhoc": is_adhoc,
                "label": label,
                "ref": ref,
                "reference_allowed_pack_refs": reference_allowed_pack_refs,
                "reference_visibility": reference_visibility,
                "updated": updated,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        accepting_new_items = d.pop("accepting_new_items")

        created = datetime.datetime.fromisoformat(d.pop("created"))

        dispatch_action_ref = d.pop("dispatch_action_ref")

        enabled = d.pop("enabled")

        id = d.pop("id")

        is_adhoc = d.pop("is_adhoc")

        label = d.pop("label")

        ref = d.pop("ref")

        reference_allowed_pack_refs = cast(
            list[str], d.pop("reference_allowed_pack_refs")
        )

        reference_visibility = ActionReferenceVisibility(d.pop("reference_visibility"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

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

        work_queue_summary = cls(
            accepting_new_items=accepting_new_items,
            created=created,
            dispatch_action_ref=dispatch_action_ref,
            enabled=enabled,
            id=id,
            is_adhoc=is_adhoc,
            label=label,
            ref=ref,
            reference_allowed_pack_refs=reference_allowed_pack_refs,
            reference_visibility=reference_visibility,
            updated=updated,
            description=description,
            pack_ref=pack_ref,
        )

        work_queue_summary.additional_properties = d
        return work_queue_summary

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
