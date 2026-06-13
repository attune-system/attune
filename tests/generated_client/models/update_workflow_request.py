from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.update_workflow_request_definition_type_0 import (
        UpdateWorkflowRequestDefinitionType0,
    )
    from ..models.update_workflow_request_out_schema_type_0 import (
        UpdateWorkflowRequestOutSchemaType0,
    )
    from ..models.update_workflow_request_param_schema_type_0 import (
        UpdateWorkflowRequestParamSchemaType0,
    )


T = TypeVar("T", bound="UpdateWorkflowRequest")


@_attrs_define
class UpdateWorkflowRequest:
    """Request DTO for updating a workflow

    Attributes:
        definition (None | UpdateWorkflowRequestDefinitionType0): Workflow definition
        out_schema (None | UpdateWorkflowRequestOutSchemaType0): Output schema
        param_schema (None | UpdateWorkflowRequestParamSchemaType0): Parameter schema (StackStorm-style with inline
            required/secret)
        description (None | str | Unset): Workflow description Example: Enhanced incident response workflow with
            additional automation.
        label (None | str | Unset): Human-readable label Example: Incident Response Workflow (Updated).
        tags (list[str] | None | Unset): Tags Example: ['incident', 'slack', 'approval', 'automation'].
        version (None | str | Unset): Workflow version Example: 1.1.0.
    """

    definition: None | UpdateWorkflowRequestDefinitionType0
    out_schema: None | UpdateWorkflowRequestOutSchemaType0
    param_schema: None | UpdateWorkflowRequestParamSchemaType0
    description: None | str | Unset = UNSET
    label: None | str | Unset = UNSET
    tags: list[str] | None | Unset = UNSET
    version: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.update_workflow_request_definition_type_0 import (
            UpdateWorkflowRequestDefinitionType0,
        )
        from ..models.update_workflow_request_out_schema_type_0 import (
            UpdateWorkflowRequestOutSchemaType0,
        )
        from ..models.update_workflow_request_param_schema_type_0 import (
            UpdateWorkflowRequestParamSchemaType0,
        )

        definition: dict[str, Any] | None
        if isinstance(self.definition, UpdateWorkflowRequestDefinitionType0):
            definition = self.definition.to_dict()
        else:
            definition = self.definition

        out_schema: dict[str, Any] | None
        if isinstance(self.out_schema, UpdateWorkflowRequestOutSchemaType0):
            out_schema = self.out_schema.to_dict()
        else:
            out_schema = self.out_schema

        param_schema: dict[str, Any] | None
        if isinstance(self.param_schema, UpdateWorkflowRequestParamSchemaType0):
            param_schema = self.param_schema.to_dict()
        else:
            param_schema = self.param_schema

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        label: None | str | Unset
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        tags: list[str] | None | Unset
        if isinstance(self.tags, Unset):
            tags = UNSET
        elif isinstance(self.tags, list):
            tags = self.tags

        else:
            tags = self.tags

        version: None | str | Unset
        if isinstance(self.version, Unset):
            version = UNSET
        else:
            version = self.version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "definition": definition,
                "out_schema": out_schema,
                "param_schema": param_schema,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if label is not UNSET:
            field_dict["label"] = label
        if tags is not UNSET:
            field_dict["tags"] = tags
        if version is not UNSET:
            field_dict["version"] = version

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.update_workflow_request_definition_type_0 import (
            UpdateWorkflowRequestDefinitionType0,
        )
        from ..models.update_workflow_request_out_schema_type_0 import (
            UpdateWorkflowRequestOutSchemaType0,
        )
        from ..models.update_workflow_request_param_schema_type_0 import (
            UpdateWorkflowRequestParamSchemaType0,
        )

        d = dict(src_dict)

        def _parse_definition(
            data: object,
        ) -> None | UpdateWorkflowRequestDefinitionType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                definition_type_0 = UpdateWorkflowRequestDefinitionType0.from_dict(data)

                return definition_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateWorkflowRequestDefinitionType0, data)

        definition = _parse_definition(d.pop("definition"))

        def _parse_out_schema(
            data: object,
        ) -> None | UpdateWorkflowRequestOutSchemaType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                out_schema_type_0 = UpdateWorkflowRequestOutSchemaType0.from_dict(data)

                return out_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateWorkflowRequestOutSchemaType0, data)

        out_schema = _parse_out_schema(d.pop("out_schema"))

        def _parse_param_schema(
            data: object,
        ) -> None | UpdateWorkflowRequestParamSchemaType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                param_schema_type_0 = UpdateWorkflowRequestParamSchemaType0.from_dict(
                    data
                )

                return param_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateWorkflowRequestParamSchemaType0, data)

        param_schema = _parse_param_schema(d.pop("param_schema"))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        label = _parse_label(d.pop("label", UNSET))

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

        def _parse_version(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        version = _parse_version(d.pop("version", UNSET))

        update_workflow_request = cls(
            definition=definition,
            out_schema=out_schema,
            param_schema=param_schema,
            description=description,
            label=label,
            tags=tags,
            version=version,
        )

        update_workflow_request.additional_properties = d
        return update_workflow_request

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
