from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.action_reference_visibility import ActionReferenceVisibility
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.save_workflow_file_request_definition import (
        SaveWorkflowFileRequestDefinition,
    )
    from ..models.save_workflow_file_request_out_schema_type_0 import (
        SaveWorkflowFileRequestOutSchemaType0,
    )
    from ..models.save_workflow_file_request_param_schema_type_0 import (
        SaveWorkflowFileRequestParamSchemaType0,
    )


T = TypeVar("T", bound="SaveWorkflowFileRequest")


@_attrs_define
class SaveWorkflowFileRequest:
    """Request DTO for saving a workflow file to disk and syncing to DB

    Attributes:
        definition (SaveWorkflowFileRequestDefinition): The full workflow definition as JSON (will be serialized to YAML
            on disk)
        label (str): Human-readable label Example: Deploy Application.
        name (str): Workflow name (becomes filename: {name}.workflow.yaml) Example: deploy_app.
        out_schema (None | SaveWorkflowFileRequestOutSchemaType0): Output schema (flat format)
        pack_ref (str): Pack reference this workflow belongs to Example: core.
        param_schema (None | SaveWorkflowFileRequestParamSchemaType0): Parameter schema (flat format with inline
            required/secret)
        version (str): Workflow version (semantic versioning recommended) Example: 1.0.0.
        description (None | str | Unset): Workflow description Example: Deploys an application to the target
            environment.
        enabled (bool | None | Unset): Whether the companion workflow action is enabled. Omitted defaults to true.
            Default: True. Example: True.
        reference_allowed_pack_refs (list[str] | Unset): Pack refs allowed to reference the companion workflow action
            when visibility is restricted. Example: ['incident_response', 'deployments'].
        reference_visibility (ActionReferenceVisibility | None | Unset):  Default: ActionReferenceVisibility.PUBLIC.
        tags (list[str] | None | Unset): Tags for categorization Example: ['deployment', 'automation'].
    """

    definition: SaveWorkflowFileRequestDefinition
    label: str
    name: str
    out_schema: None | SaveWorkflowFileRequestOutSchemaType0
    pack_ref: str
    param_schema: None | SaveWorkflowFileRequestParamSchemaType0
    version: str
    description: None | str | Unset = UNSET
    enabled: bool | None | Unset = True
    reference_allowed_pack_refs: list[str] | Unset = UNSET
    reference_visibility: ActionReferenceVisibility | None | Unset = (
        ActionReferenceVisibility.PUBLIC
    )
    tags: list[str] | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.save_workflow_file_request_out_schema_type_0 import (
            SaveWorkflowFileRequestOutSchemaType0,
        )
        from ..models.save_workflow_file_request_param_schema_type_0 import (
            SaveWorkflowFileRequestParamSchemaType0,
        )

        definition = self.definition.to_dict()

        label = self.label

        name = self.name

        out_schema: dict[str, Any] | None
        if isinstance(self.out_schema, SaveWorkflowFileRequestOutSchemaType0):
            out_schema = self.out_schema.to_dict()
        else:
            out_schema = self.out_schema

        pack_ref = self.pack_ref

        param_schema: dict[str, Any] | None
        if isinstance(self.param_schema, SaveWorkflowFileRequestParamSchemaType0):
            param_schema = self.param_schema.to_dict()
        else:
            param_schema = self.param_schema

        version = self.version

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        enabled: bool | None | Unset
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

        reference_allowed_pack_refs: list[str] | Unset = UNSET
        if not isinstance(self.reference_allowed_pack_refs, Unset):
            reference_allowed_pack_refs = self.reference_allowed_pack_refs

        reference_visibility: None | str | Unset
        if isinstance(self.reference_visibility, Unset):
            reference_visibility = UNSET
        elif isinstance(self.reference_visibility, ActionReferenceVisibility):
            reference_visibility = self.reference_visibility.value
        else:
            reference_visibility = self.reference_visibility

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
                "name": name,
                "out_schema": out_schema,
                "pack_ref": pack_ref,
                "param_schema": param_schema,
                "version": version,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if reference_allowed_pack_refs is not UNSET:
            field_dict["reference_allowed_pack_refs"] = reference_allowed_pack_refs
        if reference_visibility is not UNSET:
            field_dict["reference_visibility"] = reference_visibility
        if tags is not UNSET:
            field_dict["tags"] = tags

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.save_workflow_file_request_definition import (
            SaveWorkflowFileRequestDefinition,
        )
        from ..models.save_workflow_file_request_out_schema_type_0 import (
            SaveWorkflowFileRequestOutSchemaType0,
        )
        from ..models.save_workflow_file_request_param_schema_type_0 import (
            SaveWorkflowFileRequestParamSchemaType0,
        )

        d = dict(src_dict)
        definition = SaveWorkflowFileRequestDefinition.from_dict(d.pop("definition"))

        label = d.pop("label")

        name = d.pop("name")

        def _parse_out_schema(
            data: object,
        ) -> None | SaveWorkflowFileRequestOutSchemaType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                out_schema_type_0 = SaveWorkflowFileRequestOutSchemaType0.from_dict(
                    data
                )

                return out_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | SaveWorkflowFileRequestOutSchemaType0, data)

        out_schema = _parse_out_schema(d.pop("out_schema"))

        pack_ref = d.pop("pack_ref")

        def _parse_param_schema(
            data: object,
        ) -> None | SaveWorkflowFileRequestParamSchemaType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                param_schema_type_0 = SaveWorkflowFileRequestParamSchemaType0.from_dict(
                    data
                )

                return param_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | SaveWorkflowFileRequestParamSchemaType0, data)

        param_schema = _parse_param_schema(d.pop("param_schema"))

        version = d.pop("version")

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_enabled(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        enabled = _parse_enabled(d.pop("enabled", UNSET))

        reference_allowed_pack_refs = cast(
            list[str], d.pop("reference_allowed_pack_refs", UNSET)
        )

        def _parse_reference_visibility(
            data: object,
        ) -> ActionReferenceVisibility | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                reference_visibility_type_1 = ActionReferenceVisibility(data)

                return reference_visibility_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ActionReferenceVisibility | None | Unset, data)

        reference_visibility = _parse_reference_visibility(
            d.pop("reference_visibility", UNSET)
        )

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

        save_workflow_file_request = cls(
            definition=definition,
            label=label,
            name=name,
            out_schema=out_schema,
            pack_ref=pack_ref,
            param_schema=param_schema,
            version=version,
            description=description,
            enabled=enabled,
            reference_allowed_pack_refs=reference_allowed_pack_refs,
            reference_visibility=reference_visibility,
            tags=tags,
        )

        save_workflow_file_request.additional_properties = d
        return save_workflow_file_request

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
