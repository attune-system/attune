from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.action_reference_visibility import ActionReferenceVisibility
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_trigger_request_out_schema_type_0 import (
        CreateTriggerRequestOutSchemaType0,
    )
    from ..models.create_trigger_request_param_schema_type_0 import (
        CreateTriggerRequestParamSchemaType0,
    )


T = TypeVar("T", bound="CreateTriggerRequest")


@_attrs_define
class CreateTriggerRequest:
    """Request DTO for creating a new trigger

    Attributes:
        label (str): Human-readable label Example: Webhook Trigger.
        ref (str): Unique reference identifier (e.g., "core.webhook", "system.timer") Example: core.webhook.
        description (None | str | Unset): Trigger description Example: Triggers when a webhook is received.
        enabled (bool | Unset): Whether the trigger is enabled Example: True.
        out_schema (CreateTriggerRequestOutSchemaType0 | None | Unset): Output schema (flat format) defining event data
            structure with inline required/secret
        pack_ref (None | str | Unset): Optional pack reference this trigger belongs to Example: core.
        param_schema (CreateTriggerRequestParamSchemaType0 | None | Unset): Parameter schema (StackStorm-style) defining
            trigger configuration with inline required/secret
        reference_allowed_pack_refs (list[str] | Unset): Pack refs allowed to subscribe to this trigger when visibility
            is restricted. Example: ['incident_response', 'deployments'].
        reference_visibility (ActionReferenceVisibility | None | Unset):  Default: ActionReferenceVisibility.PUBLIC.
    """

    label: str
    ref: str
    description: None | str | Unset = UNSET
    enabled: bool | Unset = UNSET
    out_schema: CreateTriggerRequestOutSchemaType0 | None | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    param_schema: CreateTriggerRequestParamSchemaType0 | None | Unset = UNSET
    reference_allowed_pack_refs: list[str] | Unset = UNSET
    reference_visibility: ActionReferenceVisibility | None | Unset = (
        ActionReferenceVisibility.PUBLIC
    )
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.create_trigger_request_out_schema_type_0 import (
            CreateTriggerRequestOutSchemaType0,
        )
        from ..models.create_trigger_request_param_schema_type_0 import (
            CreateTriggerRequestParamSchemaType0,
        )

        label = self.label

        ref = self.ref

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        enabled = self.enabled

        out_schema: dict[str, Any] | None | Unset
        if isinstance(self.out_schema, Unset):
            out_schema = UNSET
        elif isinstance(self.out_schema, CreateTriggerRequestOutSchemaType0):
            out_schema = self.out_schema.to_dict()
        else:
            out_schema = self.out_schema

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref

        param_schema: dict[str, Any] | None | Unset
        if isinstance(self.param_schema, Unset):
            param_schema = UNSET
        elif isinstance(self.param_schema, CreateTriggerRequestParamSchemaType0):
            param_schema = self.param_schema.to_dict()
        else:
            param_schema = self.param_schema

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

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "label": label,
                "ref": ref,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if out_schema is not UNSET:
            field_dict["out_schema"] = out_schema
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref
        if param_schema is not UNSET:
            field_dict["param_schema"] = param_schema
        if reference_allowed_pack_refs is not UNSET:
            field_dict["reference_allowed_pack_refs"] = reference_allowed_pack_refs
        if reference_visibility is not UNSET:
            field_dict["reference_visibility"] = reference_visibility

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.create_trigger_request_out_schema_type_0 import (
            CreateTriggerRequestOutSchemaType0,
        )
        from ..models.create_trigger_request_param_schema_type_0 import (
            CreateTriggerRequestParamSchemaType0,
        )

        d = dict(src_dict)
        label = d.pop("label")

        ref = d.pop("ref")

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        enabled = d.pop("enabled", UNSET)

        def _parse_out_schema(
            data: object,
        ) -> CreateTriggerRequestOutSchemaType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                out_schema_type_0 = CreateTriggerRequestOutSchemaType0.from_dict(data)

                return out_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateTriggerRequestOutSchemaType0 | None | Unset, data)

        out_schema = _parse_out_schema(d.pop("out_schema", UNSET))

        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))

        def _parse_param_schema(
            data: object,
        ) -> CreateTriggerRequestParamSchemaType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                param_schema_type_0 = CreateTriggerRequestParamSchemaType0.from_dict(
                    data
                )

                return param_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateTriggerRequestParamSchemaType0 | None | Unset, data)

        param_schema = _parse_param_schema(d.pop("param_schema", UNSET))

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

        create_trigger_request = cls(
            label=label,
            ref=ref,
            description=description,
            enabled=enabled,
            out_schema=out_schema,
            pack_ref=pack_ref,
            param_schema=param_schema,
            reference_allowed_pack_refs=reference_allowed_pack_refs,
            reference_visibility=reference_visibility,
        )

        create_trigger_request.additional_properties = d
        return create_trigger_request

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
