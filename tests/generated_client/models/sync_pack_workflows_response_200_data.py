from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.workflow_sync_result import WorkflowSyncResult


T = TypeVar("T", bound="SyncPackWorkflowsResponse200Data")


@_attrs_define
class SyncPackWorkflowsResponse200Data:
    """Response for pack workflow sync operation

    Attributes:
        errors (list[str]): Any errors encountered during sync
        loaded_count (int): Number of workflows loaded from filesystem
        pack_ref (str): Pack reference
        registered_count (int): Number of workflows registered/updated in database
        workflows (list[WorkflowSyncResult]): Individual workflow registration results
    """

    errors: list[str]
    loaded_count: int
    pack_ref: str
    registered_count: int
    workflows: list[WorkflowSyncResult]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        errors = self.errors

        loaded_count = self.loaded_count

        pack_ref = self.pack_ref

        registered_count = self.registered_count

        workflows = []
        for workflows_item_data in self.workflows:
            workflows_item = workflows_item_data.to_dict()
            workflows.append(workflows_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "errors": errors,
                "loaded_count": loaded_count,
                "pack_ref": pack_ref,
                "registered_count": registered_count,
                "workflows": workflows,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.workflow_sync_result import WorkflowSyncResult

        d = dict(src_dict)
        errors = cast(list[str], d.pop("errors"))

        loaded_count = d.pop("loaded_count")

        pack_ref = d.pop("pack_ref")

        registered_count = d.pop("registered_count")

        workflows = []
        _workflows = d.pop("workflows")
        for workflows_item_data in _workflows:
            workflows_item = WorkflowSyncResult.from_dict(workflows_item_data)

            workflows.append(workflows_item)

        sync_pack_workflows_response_200_data = cls(
            errors=errors,
            loaded_count=loaded_count,
            pack_ref=pack_ref,
            registered_count=registered_count,
            workflows=workflows,
        )

        sync_pack_workflows_response_200_data.additional_properties = d
        return sync_pack_workflows_response_200_data

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
