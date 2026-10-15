from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from typing_extensions import Self

from ..models.action_reference_visibility import ActionReferenceVisibility
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.trigger_json_patch_type_0 import TriggerJsonPatchType0
    from ..models.trigger_json_patch_type_1 import TriggerJsonPatchType1
    from ..models.trigger_string_patch_type_0 import TriggerStringPatchType0
    from ..models.trigger_string_patch_type_1 import TriggerStringPatchType1


T = TypeVar("T", bound="UpdateTriggerRequest")


@_attrs_define
class UpdateTriggerRequest:
    """Request DTO for updating a trigger

    Attributes:
        description (None | TriggerStringPatchType0 | TriggerStringPatchType1 | Unset):
        enabled (bool | None | Unset): Whether the trigger is enabled Example: True.
        event_config_schema (None | TriggerJsonPatchType0 | TriggerJsonPatchType1 | Unset):
        label (None | str | Unset): Human-readable label Example: Webhook Trigger (Updated).
        out_schema (None | TriggerJsonPatchType0 | TriggerJsonPatchType1 | Unset):
        param_schema (None | TriggerJsonPatchType0 | TriggerJsonPatchType1 | Unset):
        reference_allowed_pack_refs (list[str] | None | Unset): Replace the restricted visibility allow-list. Example:
            ['incident_response', 'deployments'].
        reference_visibility (ActionReferenceVisibility | None | Unset):
    """

    description: None | TriggerStringPatchType0 | TriggerStringPatchType1 | Unset = (
        UNSET
    )
    enabled: bool | None | Unset = UNSET
    event_config_schema: (
        None | TriggerJsonPatchType0 | TriggerJsonPatchType1 | Unset
    ) = UNSET
    label: None | str | Unset = UNSET
    out_schema: None | TriggerJsonPatchType0 | TriggerJsonPatchType1 | Unset = UNSET
    param_schema: None | TriggerJsonPatchType0 | TriggerJsonPatchType1 | Unset = UNSET
    reference_allowed_pack_refs: list[str] | None | Unset = UNSET
    reference_visibility: ActionReferenceVisibility | None | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.trigger_json_patch_type_0 import (
            TriggerJsonPatchType0,
        )
        from ..models.trigger_json_patch_type_1 import (
            TriggerJsonPatchType1,
        )
        from ..models.trigger_string_patch_type_0 import (
            TriggerStringPatchType0,
        )
        from ..models.trigger_string_patch_type_1 import (
            TriggerStringPatchType1,
        )

        description: dict[str, Any] | None | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        elif isinstance(self.description, TriggerStringPatchType0) or isinstance(
            self.description, TriggerStringPatchType1
        ):
            description = self.description.to_dict()
        else:
            description = self.description

        enabled: bool | None | Unset
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

        event_config_schema: dict[str, Any] | None | Unset
        if isinstance(self.event_config_schema, Unset):
            event_config_schema = UNSET
        elif isinstance(self.event_config_schema, TriggerJsonPatchType0) or isinstance(
            self.event_config_schema, TriggerJsonPatchType1
        ):
            event_config_schema = self.event_config_schema.to_dict()
        else:
            event_config_schema = self.event_config_schema

        label: None | str | Unset
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        out_schema: dict[str, Any] | None | Unset
        if isinstance(self.out_schema, Unset):
            out_schema = UNSET
        elif isinstance(self.out_schema, TriggerJsonPatchType0) or isinstance(
            self.out_schema, TriggerJsonPatchType1
        ):
            out_schema = self.out_schema.to_dict()
        else:
            out_schema = self.out_schema

        param_schema: dict[str, Any] | None | Unset
        if isinstance(self.param_schema, Unset):
            param_schema = UNSET
        elif isinstance(self.param_schema, TriggerJsonPatchType0) or isinstance(
            self.param_schema, TriggerJsonPatchType1
        ):
            param_schema = self.param_schema.to_dict()
        else:
            param_schema = self.param_schema

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

        field_dict.update({})
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if event_config_schema is not UNSET:
            field_dict["event_config_schema"] = event_config_schema
        if label is not UNSET:
            field_dict["label"] = label
        if out_schema is not UNSET:
            field_dict["out_schema"] = out_schema
        if param_schema is not UNSET:
            field_dict["param_schema"] = param_schema
        if reference_allowed_pack_refs is not UNSET:
            field_dict["reference_allowed_pack_refs"] = reference_allowed_pack_refs
        if reference_visibility is not UNSET:
            field_dict["reference_visibility"] = reference_visibility

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.trigger_json_patch_type_0 import (
            TriggerJsonPatchType0,
        )
        from ..models.trigger_json_patch_type_1 import (
            TriggerJsonPatchType1,
        )
        from ..models.trigger_string_patch_type_0 import (
            TriggerStringPatchType0,
        )
        from ..models.trigger_string_patch_type_1 import (
            TriggerStringPatchType1,
        )

        d = dict(src_dict)

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

        def _parse_event_config_schema(
            data: object,
        ) -> None | TriggerJsonPatchType0 | TriggerJsonPatchType1 | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_trigger_json_patch_type_0 = (
                    TriggerJsonPatchType0.from_dict(data)
                )

                return componentsschemas_trigger_json_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_trigger_json_patch_type_1 = (
                    TriggerJsonPatchType1.from_dict(data)
                )

                return componentsschemas_trigger_json_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                None | TriggerJsonPatchType0 | TriggerJsonPatchType1 | Unset, data
            )

        event_config_schema = _parse_event_config_schema(
            d.pop("event_config_schema", UNSET)
        )

        def _parse_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_out_schema(
            data: object,
        ) -> None | TriggerJsonPatchType0 | TriggerJsonPatchType1 | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_trigger_json_patch_type_0 = (
                    TriggerJsonPatchType0.from_dict(data)
                )

                return componentsschemas_trigger_json_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_trigger_json_patch_type_1 = (
                    TriggerJsonPatchType1.from_dict(data)
                )

                return componentsschemas_trigger_json_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                None | TriggerJsonPatchType0 | TriggerJsonPatchType1 | Unset, data
            )

        out_schema = _parse_out_schema(d.pop("out_schema", UNSET))

        def _parse_param_schema(
            data: object,
        ) -> None | TriggerJsonPatchType0 | TriggerJsonPatchType1 | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_trigger_json_patch_type_0 = (
                    TriggerJsonPatchType0.from_dict(data)
                )

                return componentsschemas_trigger_json_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_trigger_json_patch_type_1 = (
                    TriggerJsonPatchType1.from_dict(data)
                )

                return componentsschemas_trigger_json_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                None | TriggerJsonPatchType0 | TriggerJsonPatchType1 | Unset, data
            )

        param_schema = _parse_param_schema(d.pop("param_schema", UNSET))

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
            description=description,
            enabled=enabled,
            event_config_schema=event_config_schema,
            label=label,
            out_schema=out_schema,
            param_schema=param_schema,
            reference_allowed_pack_refs=reference_allowed_pack_refs,
            reference_visibility=reference_visibility,
        )

        return update_trigger_request
