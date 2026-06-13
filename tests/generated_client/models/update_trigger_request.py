from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.action_reference_visibility import ActionReferenceVisibility
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.trigger_string_patch_type_0 import TriggerStringPatchType0
    from ..models.trigger_string_patch_type_1 import TriggerStringPatchType1
    from ..models.update_trigger_request_out_schema_type_0 import (
        UpdateTriggerRequestOutSchemaType0,
    )
    from ..models.update_trigger_request_param_schema_type_0 import (
        UpdateTriggerRequestParamSchemaType0,
    )


T = TypeVar("T", bound="UpdateTriggerRequest")


@_attrs_define
class UpdateTriggerRequest:
    """Request DTO for updating a trigger

    Attributes:
        out_schema (None | UpdateTriggerRequestOutSchemaType0): Output schema
        param_schema (None | UpdateTriggerRequestParamSchemaType0): Parameter schema (StackStorm-style with inline
            required/secret)
        description (None | TriggerStringPatchType0 | TriggerStringPatchType1 | Unset):
        enabled (bool | None | Unset): Whether the trigger is enabled Example: True.
        label (None | str | Unset): Human-readable label Example: Webhook Trigger (Updated).
        reference_allowed_pack_refs (list[str] | None | Unset): Replace the restricted visibility allow-list. Example:
            ['incident_response', 'deployments'].
        reference_visibility (ActionReferenceVisibility | None | Unset):
    """

    out_schema: None | UpdateTriggerRequestOutSchemaType0
    param_schema: None | UpdateTriggerRequestParamSchemaType0
    description: None | TriggerStringPatchType0 | TriggerStringPatchType1 | Unset = (
        UNSET
    )
    enabled: bool | None | Unset = UNSET
    label: None | str | Unset = UNSET
    reference_allowed_pack_refs: list[str] | None | Unset = UNSET
    reference_visibility: ActionReferenceVisibility | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.trigger_string_patch_type_0 import TriggerStringPatchType0
        from ..models.trigger_string_patch_type_1 import TriggerStringPatchType1
        from ..models.update_trigger_request_out_schema_type_0 import (
            UpdateTriggerRequestOutSchemaType0,
        )
        from ..models.update_trigger_request_param_schema_type_0 import (
            UpdateTriggerRequestParamSchemaType0,
        )

        out_schema: dict[str, Any] | None
        if isinstance(self.out_schema, UpdateTriggerRequestOutSchemaType0):
            out_schema = self.out_schema.to_dict()
        else:
            out_schema = self.out_schema

        param_schema: dict[str, Any] | None
        if isinstance(self.param_schema, UpdateTriggerRequestParamSchemaType0):
            param_schema = self.param_schema.to_dict()
        else:
            param_schema = self.param_schema

        description: dict[str, Any] | None | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        elif isinstance(self.description, TriggerStringPatchType0):
            description = self.description.to_dict()
        elif isinstance(self.description, TriggerStringPatchType1):
            description = self.description.to_dict()
        else:
            description = self.description

        enabled: bool | None | Unset
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

        label: None | str | Unset
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        reference_allowed_pack_refs: list[str] | None | Unset
        if isinstance(self.reference_allowed_pack_refs, Unset):
            reference_allowed_pack_refs = UNSET
        elif isinstance(self.reference_allowed_pack_refs, list):
            reference_allowed_pack_refs = self.reference_allowed_pack_refs

        else:
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
                "out_schema": out_schema,
                "param_schema": param_schema,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if label is not UNSET:
            field_dict["label"] = label
        if reference_allowed_pack_refs is not UNSET:
            field_dict["reference_allowed_pack_refs"] = reference_allowed_pack_refs
        if reference_visibility is not UNSET:
            field_dict["reference_visibility"] = reference_visibility

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.trigger_string_patch_type_0 import TriggerStringPatchType0
        from ..models.trigger_string_patch_type_1 import TriggerStringPatchType1
        from ..models.update_trigger_request_out_schema_type_0 import (
            UpdateTriggerRequestOutSchemaType0,
        )
        from ..models.update_trigger_request_param_schema_type_0 import (
            UpdateTriggerRequestParamSchemaType0,
        )

        d = dict(src_dict)

        def _parse_out_schema(
            data: object,
        ) -> None | UpdateTriggerRequestOutSchemaType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                out_schema_type_0 = UpdateTriggerRequestOutSchemaType0.from_dict(data)

                return out_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateTriggerRequestOutSchemaType0, data)

        out_schema = _parse_out_schema(d.pop("out_schema"))

        def _parse_param_schema(
            data: object,
        ) -> None | UpdateTriggerRequestParamSchemaType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                param_schema_type_0 = UpdateTriggerRequestParamSchemaType0.from_dict(
                    data
                )

                return param_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateTriggerRequestParamSchemaType0, data)

        param_schema = _parse_param_schema(d.pop("param_schema"))

        def _parse_description(
            data: object,
        ) -> None | TriggerStringPatchType0 | TriggerStringPatchType1 | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_trigger_string_patch_type_0 = (
                    TriggerStringPatchType0.from_dict(data)
                )

                return componentsschemas_trigger_string_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_trigger_string_patch_type_1 = (
                    TriggerStringPatchType1.from_dict(data)
                )

                return componentsschemas_trigger_string_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                None | TriggerStringPatchType0 | TriggerStringPatchType1 | Unset, data
            )

        description = _parse_description(d.pop("description", UNSET))

        def _parse_enabled(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        enabled = _parse_enabled(d.pop("enabled", UNSET))

        def _parse_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_reference_allowed_pack_refs(
            data: object,
        ) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                reference_allowed_pack_refs_type_0 = cast(list[str], data)

                return reference_allowed_pack_refs_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        reference_allowed_pack_refs = _parse_reference_allowed_pack_refs(
            d.pop("reference_allowed_pack_refs", UNSET)
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

        update_trigger_request = cls(
            out_schema=out_schema,
            param_schema=param_schema,
            description=description,
            enabled=enabled,
            label=label,
            reference_allowed_pack_refs=reference_allowed_pack_refs,
            reference_visibility=reference_visibility,
        )

        update_trigger_request.additional_properties = d
        return update_trigger_request

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
