from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="PaginatedResponseWorkflowSummaryItemsItem")


@_attrs_define
class PaginatedResponseWorkflowSummaryItemsItem:
    """Simplified workflow response (for list endpoints)

    Attributes:
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        id (int): Workflow ID Example: 1.
        label (str): Human-readable label Example: Incident Response Workflow.
        pack_ref (str): Pack reference Example: slack.
        ref (str): Unique reference identifier Example: slack.incident_workflow.
        tags (list[str]): Tags Example: ['incident', 'slack', 'approval'].
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:30:00Z.
        version (str): Workflow version Example: 1.0.0.
        description (None | str | Unset): Workflow description Example: Automated incident response workflow with
            notifications and approvals.
    """

    created: datetime.datetime
    id: int
    label: str
    pack_ref: str
    ref: str
    tags: list[str]
    updated: datetime.datetime
    version: str
    description: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        id = self.id

        label = self.label

        pack_ref = self.pack_ref

        ref = self.ref

        tags = self.tags

        updated = self.updated.isoformat()

        version = self.version

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "id": id,
                "label": label,
                "pack_ref": pack_ref,
                "ref": ref,
                "tags": tags,
                "updated": updated,
                "version": version,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        label = d.pop("label")

        pack_ref = d.pop("pack_ref")

        ref = d.pop("ref")

        tags = cast(list[str], d.pop("tags"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        version = d.pop("version")

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        paginated_response_workflow_summary_items_item = cls(
            created=created,
            id=id,
            label=label,
            pack_ref=pack_ref,
            ref=ref,
            tags=tags,
            updated=updated,
            version=version,
            description=description,
        )

        paginated_response_workflow_summary_items_item.additional_properties = d
        return paginated_response_workflow_summary_items_item

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
