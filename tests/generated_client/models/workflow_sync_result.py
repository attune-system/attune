from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="WorkflowSyncResult")


@_attrs_define
class WorkflowSyncResult:
    """Individual workflow sync result

    Attributes:
        created (bool): Whether the workflow was created (false = updated)
        ref_name (str): Workflow reference name
        warnings (list[str]): Any warnings during registration
        workflow_def_id (int): Workflow definition ID
    """

    created: bool
    ref_name: str
    warnings: list[str]
    workflow_def_id: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created

        ref_name = self.ref_name

        warnings = self.warnings

        workflow_def_id = self.workflow_def_id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "ref_name": ref_name,
                "warnings": warnings,
                "workflow_def_id": workflow_def_id,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        created = d.pop("created")

        ref_name = d.pop("ref_name")

        warnings = cast(list[str], d.pop("warnings"))

        workflow_def_id = d.pop("workflow_def_id")

        workflow_sync_result = cls(
            created=created,
            ref_name=ref_name,
            warnings=warnings,
            workflow_def_id=workflow_def_id,
        )

        workflow_sync_result.additional_properties = d
        return workflow_sync_result

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
