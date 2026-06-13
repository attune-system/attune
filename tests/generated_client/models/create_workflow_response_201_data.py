from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_workflow_response_201_data_definition import (
        CreateWorkflowResponse201DataDefinition,
    )
    from ..models.create_workflow_response_201_data_out_schema_type_0 import (
        CreateWorkflowResponse201DataOutSchemaType0,
    )
    from ..models.create_workflow_response_201_data_param_schema_type_0 import (
        CreateWorkflowResponse201DataParamSchemaType0,
    )


T = TypeVar("T", bound="CreateWorkflowResponse201Data")


@_attrs_define
class CreateWorkflowResponse201Data:
    """Response DTO for workflow information

    Attributes:
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        definition (CreateWorkflowResponse201DataDefinition): Workflow definition
        id (int): Workflow ID Example: 1.
        label (str): Human-readable label Example: Incident Response Workflow.
        out_schema (CreateWorkflowResponse201DataOutSchemaType0 | None): Output schema
        pack (int): Pack ID Example: 1.
        pack_ref (str): Pack reference Example: slack.
        param_schema (CreateWorkflowResponse201DataParamSchemaType0 | None): Parameter schema (StackStorm-style with
            inline required/secret)
        ref (str): Unique reference identifier Example: slack.incident_workflow.
        tags (list[str]): Tags Example: ['incident', 'slack', 'approval'].
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:30:00Z.
        version (str): Workflow version Example: 1.0.0.
        description (None | str | Unset): Workflow description Example: Automated incident response workflow with
            notifications and approvals.
    """

    created: datetime.datetime
    definition: CreateWorkflowResponse201DataDefinition
    id: int
    label: str
    out_schema: CreateWorkflowResponse201DataOutSchemaType0 | None
    pack: int
    pack_ref: str
    param_schema: CreateWorkflowResponse201DataParamSchemaType0 | None
    ref: str
    tags: list[str]
    updated: datetime.datetime
    version: str
    description: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.create_workflow_response_201_data_out_schema_type_0 import (
            CreateWorkflowResponse201DataOutSchemaType0,
        )
        from ..models.create_workflow_response_201_data_param_schema_type_0 import (
            CreateWorkflowResponse201DataParamSchemaType0,
        )

        created = self.created.isoformat()

        definition = self.definition.to_dict()

        id = self.id

        label = self.label

        out_schema: dict[str, Any] | None
        if isinstance(self.out_schema, CreateWorkflowResponse201DataOutSchemaType0):
            out_schema = self.out_schema.to_dict()
        else:
            out_schema = self.out_schema

        pack = self.pack

        pack_ref = self.pack_ref

        param_schema: dict[str, Any] | None
        if isinstance(self.param_schema, CreateWorkflowResponse201DataParamSchemaType0):
            param_schema = self.param_schema.to_dict()
        else:
            param_schema = self.param_schema

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
                "definition": definition,
                "id": id,
                "label": label,
                "out_schema": out_schema,
                "pack": pack,
                "pack_ref": pack_ref,
                "param_schema": param_schema,
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
        from ..models.create_workflow_response_201_data_definition import (
            CreateWorkflowResponse201DataDefinition,
        )
        from ..models.create_workflow_response_201_data_out_schema_type_0 import (
            CreateWorkflowResponse201DataOutSchemaType0,
        )
        from ..models.create_workflow_response_201_data_param_schema_type_0 import (
            CreateWorkflowResponse201DataParamSchemaType0,
        )

        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        definition = CreateWorkflowResponse201DataDefinition.from_dict(
            d.pop("definition")
        )

        id = d.pop("id")

        label = d.pop("label")

        def _parse_out_schema(
            data: object,
        ) -> CreateWorkflowResponse201DataOutSchemaType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                out_schema_type_0 = (
                    CreateWorkflowResponse201DataOutSchemaType0.from_dict(data)
                )

                return out_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateWorkflowResponse201DataOutSchemaType0 | None, data)

        out_schema = _parse_out_schema(d.pop("out_schema"))

        pack = d.pop("pack")

        pack_ref = d.pop("pack_ref")

        def _parse_param_schema(
            data: object,
        ) -> CreateWorkflowResponse201DataParamSchemaType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                param_schema_type_0 = (
                    CreateWorkflowResponse201DataParamSchemaType0.from_dict(data)
                )

                return param_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateWorkflowResponse201DataParamSchemaType0 | None, data)

        param_schema = _parse_param_schema(d.pop("param_schema"))

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

        create_workflow_response_201_data = cls(
            created=created,
            definition=definition,
            id=id,
            label=label,
            out_schema=out_schema,
            pack=pack,
            pack_ref=pack_ref,
            param_schema=param_schema,
            ref=ref,
            tags=tags,
            updated=updated,
            version=version,
            description=description,
        )

        create_workflow_response_201_data.additional_properties = d
        return create_workflow_response_201_data

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
