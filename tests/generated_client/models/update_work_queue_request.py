from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.action_reference_visibility import ActionReferenceVisibility
from ..models.work_queue_batch_mode import WorkQueueBatchMode
from ..models.work_queue_update_strategy import WorkQueueUpdateStrategy
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.nullable_string_patch_type_1 import NullableStringPatchType1
    from ..models.set_string import SetString
    from ..models.update_work_queue_request_action_params_type_0 import (
        UpdateWorkQueueRequestActionParamsType0,
    )
    from ..models.update_work_queue_request_config_type_0 import (
        UpdateWorkQueueRequestConfigType0,
    )
    from ..models.update_work_queue_request_item_schema_type_0 import (
        UpdateWorkQueueRequestItemSchemaType0,
    )


T = TypeVar("T", bound="UpdateWorkQueueRequest")


@_attrs_define
class UpdateWorkQueueRequest:
    """
    Attributes:
        action_params (None | UpdateWorkQueueRequestActionParamsType0):
        config (None | UpdateWorkQueueRequestConfigType0):
        item_schema (None | UpdateWorkQueueRequestItemSchemaType0):
        accepting_new_items (bool | None | Unset):  Example: True.
        allow_pending_update (bool | None | Unset):  Example: True.
        batch_mode (None | Unset | WorkQueueBatchMode):
        default_priority (int | None | Unset):  Example: 10.
        description (None | NullableStringPatchType1 | SetString | Unset):
        dispatch_action_ref (None | str | Unset):  Example: core.process_item.
        enabled (bool | None | Unset):
        label (None | str | Unset):  Example: Core Inbox (Updated).
        pack_ref (None | NullableStringPatchType1 | SetString | Unset):
        permission_set_refs (list[str] | None | Unset): Permission set refs to apply to executions dispatched by this
            queue. Omit
            to keep the current value. Provide null to inherit the dispatch action
            default, or an empty array to force no API token. Example: ['core.agent_reader'].
        reference_allowed_pack_refs (list[str] | None | Unset): Replace the restricted visibility allow-list. Example:
            ['incident_response', 'deployments'].
        reference_visibility (ActionReferenceVisibility | None | Unset):
        update_strategy (None | Unset | WorkQueueUpdateStrategy):
    """

    action_params: None | UpdateWorkQueueRequestActionParamsType0
    config: None | UpdateWorkQueueRequestConfigType0
    item_schema: None | UpdateWorkQueueRequestItemSchemaType0
    accepting_new_items: bool | None | Unset = UNSET
    allow_pending_update: bool | None | Unset = UNSET
    batch_mode: None | Unset | WorkQueueBatchMode = UNSET
    default_priority: int | None | Unset = UNSET
    description: None | NullableStringPatchType1 | SetString | Unset = UNSET
    dispatch_action_ref: None | str | Unset = UNSET
    enabled: bool | None | Unset = UNSET
    label: None | str | Unset = UNSET
    pack_ref: None | NullableStringPatchType1 | SetString | Unset = UNSET
    permission_set_refs: list[str] | None | Unset = UNSET
    reference_allowed_pack_refs: list[str] | None | Unset = UNSET
    reference_visibility: ActionReferenceVisibility | None | Unset = UNSET
    update_strategy: None | Unset | WorkQueueUpdateStrategy = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.nullable_string_patch_type_1 import NullableStringPatchType1
        from ..models.set_string import SetString
        from ..models.update_work_queue_request_action_params_type_0 import (
            UpdateWorkQueueRequestActionParamsType0,
        )
        from ..models.update_work_queue_request_config_type_0 import (
            UpdateWorkQueueRequestConfigType0,
        )
        from ..models.update_work_queue_request_item_schema_type_0 import (
            UpdateWorkQueueRequestItemSchemaType0,
        )

        action_params: dict[str, Any] | None
        if isinstance(self.action_params, UpdateWorkQueueRequestActionParamsType0):
            action_params = self.action_params.to_dict()
        else:
            action_params = self.action_params

        config: dict[str, Any] | None
        if isinstance(self.config, UpdateWorkQueueRequestConfigType0):
            config = self.config.to_dict()
        else:
            config = self.config

        item_schema: dict[str, Any] | None
        if isinstance(self.item_schema, UpdateWorkQueueRequestItemSchemaType0):
            item_schema = self.item_schema.to_dict()
        else:
            item_schema = self.item_schema

        accepting_new_items: bool | None | Unset
        if isinstance(self.accepting_new_items, Unset):
            accepting_new_items = UNSET
        else:
            accepting_new_items = self.accepting_new_items

        allow_pending_update: bool | None | Unset
        if isinstance(self.allow_pending_update, Unset):
            allow_pending_update = UNSET
        else:
            allow_pending_update = self.allow_pending_update

        batch_mode: None | str | Unset
        if isinstance(self.batch_mode, Unset):
            batch_mode = UNSET
        elif isinstance(self.batch_mode, WorkQueueBatchMode):
            batch_mode = self.batch_mode.value
        else:
            batch_mode = self.batch_mode

        default_priority: int | None | Unset
        if isinstance(self.default_priority, Unset):
            default_priority = UNSET
        else:
            default_priority = self.default_priority

        description: dict[str, Any] | None | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        elif isinstance(self.description, SetString):
            description = self.description.to_dict()
        elif isinstance(self.description, NullableStringPatchType1):
            description = self.description.to_dict()
        else:
            description = self.description

        dispatch_action_ref: None | str | Unset
        if isinstance(self.dispatch_action_ref, Unset):
            dispatch_action_ref = UNSET
        else:
            dispatch_action_ref = self.dispatch_action_ref

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

        pack_ref: dict[str, Any] | None | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        elif isinstance(self.pack_ref, SetString):
            pack_ref = self.pack_ref.to_dict()
        elif isinstance(self.pack_ref, NullableStringPatchType1):
            pack_ref = self.pack_ref.to_dict()
        else:
            pack_ref = self.pack_ref

        permission_set_refs: list[str] | None | Unset
        if isinstance(self.permission_set_refs, Unset):
            permission_set_refs = UNSET
        elif isinstance(self.permission_set_refs, list):
            permission_set_refs = self.permission_set_refs

        else:
            permission_set_refs = self.permission_set_refs

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

        update_strategy: None | str | Unset
        if isinstance(self.update_strategy, Unset):
            update_strategy = UNSET
        elif isinstance(self.update_strategy, WorkQueueUpdateStrategy):
            update_strategy = self.update_strategy.value
        else:
            update_strategy = self.update_strategy

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "action_params": action_params,
                "config": config,
                "item_schema": item_schema,
            }
        )
        if accepting_new_items is not UNSET:
            field_dict["accepting_new_items"] = accepting_new_items
        if allow_pending_update is not UNSET:
            field_dict["allow_pending_update"] = allow_pending_update
        if batch_mode is not UNSET:
            field_dict["batch_mode"] = batch_mode
        if default_priority is not UNSET:
            field_dict["default_priority"] = default_priority
        if description is not UNSET:
            field_dict["description"] = description
        if dispatch_action_ref is not UNSET:
            field_dict["dispatch_action_ref"] = dispatch_action_ref
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if label is not UNSET:
            field_dict["label"] = label
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref
        if permission_set_refs is not UNSET:
            field_dict["permission_set_refs"] = permission_set_refs
        if reference_allowed_pack_refs is not UNSET:
            field_dict["reference_allowed_pack_refs"] = reference_allowed_pack_refs
        if reference_visibility is not UNSET:
            field_dict["reference_visibility"] = reference_visibility
        if update_strategy is not UNSET:
            field_dict["update_strategy"] = update_strategy

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.nullable_string_patch_type_1 import NullableStringPatchType1
        from ..models.set_string import SetString
        from ..models.update_work_queue_request_action_params_type_0 import (
            UpdateWorkQueueRequestActionParamsType0,
        )
        from ..models.update_work_queue_request_config_type_0 import (
            UpdateWorkQueueRequestConfigType0,
        )
        from ..models.update_work_queue_request_item_schema_type_0 import (
            UpdateWorkQueueRequestItemSchemaType0,
        )

        d = dict(src_dict)

        def _parse_action_params(
            data: object,
        ) -> None | UpdateWorkQueueRequestActionParamsType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                action_params_type_0 = (
                    UpdateWorkQueueRequestActionParamsType0.from_dict(data)
                )

                return action_params_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateWorkQueueRequestActionParamsType0, data)

        action_params = _parse_action_params(d.pop("action_params"))

        def _parse_config(data: object) -> None | UpdateWorkQueueRequestConfigType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                config_type_0 = UpdateWorkQueueRequestConfigType0.from_dict(data)

                return config_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateWorkQueueRequestConfigType0, data)

        config = _parse_config(d.pop("config"))

        def _parse_item_schema(
            data: object,
        ) -> None | UpdateWorkQueueRequestItemSchemaType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                item_schema_type_0 = UpdateWorkQueueRequestItemSchemaType0.from_dict(
                    data
                )

                return item_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateWorkQueueRequestItemSchemaType0, data)

        item_schema = _parse_item_schema(d.pop("item_schema"))

        def _parse_accepting_new_items(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        accepting_new_items = _parse_accepting_new_items(
            d.pop("accepting_new_items", UNSET)
        )

        def _parse_allow_pending_update(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        allow_pending_update = _parse_allow_pending_update(
            d.pop("allow_pending_update", UNSET)
        )

        def _parse_batch_mode(data: object) -> None | Unset | WorkQueueBatchMode:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                batch_mode_type_1 = WorkQueueBatchMode(data)

                return batch_mode_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | WorkQueueBatchMode, data)

        batch_mode = _parse_batch_mode(d.pop("batch_mode", UNSET))

        def _parse_default_priority(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        default_priority = _parse_default_priority(d.pop("default_priority", UNSET))

        def _parse_description(
            data: object,
        ) -> None | NullableStringPatchType1 | SetString | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_nullable_string_patch_set_string = (
                    SetString.from_dict(data)
                )

                return componentsschemas_nullable_string_patch_set_string
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_nullable_string_patch_type_1 = (
                    NullableStringPatchType1.from_dict(data)
                )

                return componentsschemas_nullable_string_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | NullableStringPatchType1 | SetString | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_dispatch_action_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        dispatch_action_ref = _parse_dispatch_action_ref(
            d.pop("dispatch_action_ref", UNSET)
        )

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

        def _parse_pack_ref(
            data: object,
        ) -> None | NullableStringPatchType1 | SetString | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_nullable_string_patch_set_string = (
                    SetString.from_dict(data)
                )

                return componentsschemas_nullable_string_patch_set_string
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_nullable_string_patch_type_1 = (
                    NullableStringPatchType1.from_dict(data)
                )

                return componentsschemas_nullable_string_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | NullableStringPatchType1 | SetString | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))

        def _parse_permission_set_refs(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                permission_set_refs_type_0 = cast(list[str], data)

                return permission_set_refs_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        permission_set_refs = _parse_permission_set_refs(
            d.pop("permission_set_refs", UNSET)
        )

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

        def _parse_update_strategy(
            data: object,
        ) -> None | Unset | WorkQueueUpdateStrategy:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                update_strategy_type_1 = WorkQueueUpdateStrategy(data)

                return update_strategy_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | WorkQueueUpdateStrategy, data)

        update_strategy = _parse_update_strategy(d.pop("update_strategy", UNSET))

        update_work_queue_request = cls(
            action_params=action_params,
            config=config,
            item_schema=item_schema,
            accepting_new_items=accepting_new_items,
            allow_pending_update=allow_pending_update,
            batch_mode=batch_mode,
            default_priority=default_priority,
            description=description,
            dispatch_action_ref=dispatch_action_ref,
            enabled=enabled,
            label=label,
            pack_ref=pack_ref,
            permission_set_refs=permission_set_refs,
            reference_allowed_pack_refs=reference_allowed_pack_refs,
            reference_visibility=reference_visibility,
            update_strategy=update_strategy,
        )

        update_work_queue_request.additional_properties = d
        return update_work_queue_request

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
