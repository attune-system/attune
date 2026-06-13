from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.action_reference_visibility import ActionReferenceVisibility
from ..types import UNSET, Unset

T = TypeVar("T", bound="PaginatedResponseActionSearchHitItemsItem")


@_attrs_define
class PaginatedResponseActionSearchHitItemsItem:
    """Lean search hit for action discovery — designed to minimize context bloat
    for AI agents and humans browsing large action catalogs. Excludes ID,
    timestamps, schemas, and runtime internals.

        Attributes:
            accesses_mcp (bool): Hint that this action may invoke the Attune MCP server and spawn child executions.
            is_workflow (bool): True when this action is a workflow (orchestrates child executions)
            label (str): Human-readable label Example: Post Message to Slack.
            pack_ref (str): Pack reference Example: slack.
            ref (str): Action reference (globally unique identifier, e.g., "slack.post_message") Example:
                slack.post_message.
            reference_visibility (ActionReferenceVisibility):
            description (None | str | Unset): Action description Example: Posts a message to a Slack channel.
            runtime_ref (None | str | Unset): Runtime reference (e.g., "core.python"). None for workflow actions. Example:
                core.python.
    """

    accesses_mcp: bool
    is_workflow: bool
    label: str
    pack_ref: str
    ref: str
    reference_visibility: ActionReferenceVisibility
    description: None | str | Unset = UNSET
    runtime_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        accesses_mcp = self.accesses_mcp

        is_workflow = self.is_workflow

        label = self.label

        pack_ref = self.pack_ref

        ref = self.ref

        reference_visibility = self.reference_visibility.value

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        runtime_ref: None | str | Unset
        if isinstance(self.runtime_ref, Unset):
            runtime_ref = UNSET
        else:
            runtime_ref = self.runtime_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "accesses_mcp": accesses_mcp,
                "is_workflow": is_workflow,
                "label": label,
                "pack_ref": pack_ref,
                "ref": ref,
                "reference_visibility": reference_visibility,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if runtime_ref is not UNSET:
            field_dict["runtime_ref"] = runtime_ref

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        accesses_mcp = d.pop("accesses_mcp")

        is_workflow = d.pop("is_workflow")

        label = d.pop("label")

        pack_ref = d.pop("pack_ref")

        ref = d.pop("ref")

        reference_visibility = ActionReferenceVisibility(d.pop("reference_visibility"))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_runtime_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        runtime_ref = _parse_runtime_ref(d.pop("runtime_ref", UNSET))

        paginated_response_action_search_hit_items_item = cls(
            accesses_mcp=accesses_mcp,
            is_workflow=is_workflow,
            label=label,
            pack_ref=pack_ref,
            ref=ref,
            reference_visibility=reference_visibility,
            description=description,
            runtime_ref=runtime_ref,
        )

        paginated_response_action_search_hit_items_item.additional_properties = d
        return paginated_response_action_search_hit_items_item

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
