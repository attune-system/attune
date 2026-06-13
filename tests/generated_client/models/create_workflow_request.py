from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_workflow_request_definition import (
        CreateWorkflowRequestDefinition,
    )
    from ..models.create_workflow_request_out_schema import (
        CreateWorkflowRequestOutSchema,
    )
    from ..models.create_workflow_request_param_schema import (
        CreateWorkflowRequestParamSchema,
    )


T = TypeVar("T", bound="CreateWorkflowRequest")


@_attrs_define
class CreateWorkflowRequest:
    """Request DTO for creating a new workflow

    Attributes:
        definition (CreateWorkflowRequestDefinition): Workflow definition (complete workflow YAML structure as JSON)
        label (str): Human-readable label Example: Incident Response Workflow.
        out_schema (CreateWorkflowRequestOutSchema): Output schema (flat format) defining expected outputs with inline
            required/secret
        pack_ref (str): Pack reference this workflow belongs to Example: slack.
        param_schema (CreateWorkflowRequestParamSchema): Parameter schema (StackStorm-style) defining expected inputs
            with inline required/secret
        ref (str): Unique reference identifier (e.g., "core.notify_on_failure", "slack.incident_workflow") Example:
            slack.incident_workflow.
        version (str): Workflow version (semantic versioning recommended) Example: 1.0.0.
        description (None | str | Unset): Workflow description Example: Automated incident response workflow with
            notifications and approvals.
        tags (list[str] | None | Unset): Tags for categorization and search Example: ['incident', 'slack', 'approval'].
    """

    definition: CreateWorkflowRequestDefinition
    label: str
    out_schema: CreateWorkflowRequestOutSchema
    pack_ref: str
    param_schema: CreateWorkflowRequestParamSchema
    ref: str
    version: str
    description: None | str | Unset = UNSET
    tags: list[str] | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        definition = self.definition.to_dict()

        label = self.label

        out_schema = self.out_schema.to_dict()

        pack_ref = self.pack_ref

        param_schema = self.param_schema.to_dict()

        ref = self.ref

        version = self.version

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        tags: list[str] | None | Unset
        if isinstance(self.tags, Unset):
            tags = UNSET
        elif isinstance(self.tags, list):
            tags = self.tags

        else:
            tags = self.tags

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "definition": definition,
                "label": label,
                "out_schema": out_schema,
                "pack_ref": pack_ref,
                "param_schema": param_schema,
                "ref": ref,
                "version": version,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if tags is not UNSET:
            field_dict["tags"] = tags

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.create_workflow_request_definition import (
            CreateWorkflowRequestDefinition,
        )
        from ..models.create_workflow_request_out_schema import (
            CreateWorkflowRequestOutSchema,
        )
        from ..models.create_workflow_request_param_schema import (
            CreateWorkflowRequestParamSchema,
        )

        d = dict(src_dict)
        definition = CreateWorkflowRequestDefinition.from_dict(d.pop("definition"))

        label = d.pop("label")

        out_schema = CreateWorkflowRequestOutSchema.from_dict(d.pop("out_schema"))

        pack_ref = d.pop("pack_ref")

        param_schema = CreateWorkflowRequestParamSchema.from_dict(d.pop("param_schema"))

        ref = d.pop("ref")

        version = d.pop("version")

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_tags(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                tags_type_0 = cast(list[str], data)

                return tags_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        tags = _parse_tags(d.pop("tags", UNSET))

        create_workflow_request = cls(
            definition=definition,
            label=label,
            out_schema=out_schema,
            pack_ref=pack_ref,
            param_schema=param_schema,
            ref=ref,
            version=version,
            description=description,
            tags=tags,
        )

        create_workflow_request.additional_properties = d
        return create_workflow_request

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
