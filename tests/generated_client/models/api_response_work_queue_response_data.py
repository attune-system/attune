from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.action_reference_visibility import ActionReferenceVisibility
from ..models.work_queue_batch_mode import WorkQueueBatchMode
from ..models.work_queue_update_strategy import WorkQueueUpdateStrategy
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.api_response_work_queue_response_data_action_params import (
        ApiResponseWorkQueueResponseDataActionParams,
    )
    from ..models.api_response_work_queue_response_data_config import (
        ApiResponseWorkQueueResponseDataConfig,
    )
    from ..models.api_response_work_queue_response_data_item_schema import (
        ApiResponseWorkQueueResponseDataItemSchema,
    )
    from ..models.resolved_work_queue_dispatch_tuning_response import (
        ResolvedWorkQueueDispatchTuningResponse,
    )


T = TypeVar("T", bound="ApiResponseWorkQueueResponseData")


@_attrs_define
class ApiResponseWorkQueueResponseData:
    """
    Attributes:
        accepting_new_items (bool):  Example: True.
        action_params (ApiResponseWorkQueueResponseDataActionParams):
        allow_pending_update (bool):
        batch_mode (WorkQueueBatchMode):
        config (ApiResponseWorkQueueResponseDataConfig):
        created (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
        default_priority (int):
        dispatch_action_ref (str):  Example: core.process_item.
        enabled (bool):  Example: True.
        id (int):
        is_adhoc (bool):
        item_schema (ApiResponseWorkQueueResponseDataItemSchema):
        label (str):  Example: Core Inbox.
        ref (str):  Example: core.inbox.
        reference_allowed_pack_refs (list[str]):  Example: ['incident_response', 'deployments'].
        reference_visibility (ActionReferenceVisibility):
        update_strategy (WorkQueueUpdateStrategy):
        updated (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
        description (None | str | Unset):  Example: Dispatches inbound work items to the core processor.
        dispatch_action (int | None | Unset):
        pack (int | None | Unset):
        pack_ref (None | str | Unset):  Example: core.
        permission_set_refs (list[str] | None | Unset):  Example: ['core.agent_reader'].
        resolved_dispatch_tuning (None | ResolvedWorkQueueDispatchTuningResponse | Unset):
    """

    accepting_new_items: bool
    action_params: ApiResponseWorkQueueResponseDataActionParams
    allow_pending_update: bool
    batch_mode: WorkQueueBatchMode
    config: ApiResponseWorkQueueResponseDataConfig
    created: datetime.datetime
    default_priority: int
    dispatch_action_ref: str
    enabled: bool
    id: int
    is_adhoc: bool
    item_schema: ApiResponseWorkQueueResponseDataItemSchema
    label: str
    ref: str
    reference_allowed_pack_refs: list[str]
    reference_visibility: ActionReferenceVisibility
    update_strategy: WorkQueueUpdateStrategy
    updated: datetime.datetime
    description: None | str | Unset = UNSET
    dispatch_action: int | None | Unset = UNSET
    pack: int | None | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    permission_set_refs: list[str] | None | Unset = UNSET
    resolved_dispatch_tuning: None | ResolvedWorkQueueDispatchTuningResponse | Unset = (
        UNSET
    )
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.resolved_work_queue_dispatch_tuning_response import (
            ResolvedWorkQueueDispatchTuningResponse,
        )

        accepting_new_items = self.accepting_new_items

        action_params = self.action_params.to_dict()

        allow_pending_update = self.allow_pending_update

        batch_mode = self.batch_mode.value

        config = self.config.to_dict()

        created = self.created.isoformat()

        default_priority = self.default_priority

        dispatch_action_ref = self.dispatch_action_ref

        enabled = self.enabled

        id = self.id

        is_adhoc = self.is_adhoc

        item_schema = self.item_schema.to_dict()

        label = self.label

        ref = self.ref

        reference_allowed_pack_refs = self.reference_allowed_pack_refs

        reference_visibility = self.reference_visibility.value

        update_strategy = self.update_strategy.value

        updated = self.updated.isoformat()

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        dispatch_action: int | None | Unset
        if isinstance(self.dispatch_action, Unset):
            dispatch_action = UNSET
        else:
            dispatch_action = self.dispatch_action

        pack: int | None | Unset
        if isinstance(self.pack, Unset):
            pack = UNSET
        else:
            pack = self.pack

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref

        permission_set_refs: list[str] | None | Unset
        if isinstance(self.permission_set_refs, Unset):
            permission_set_refs = UNSET
        elif isinstance(self.permission_set_refs, list):
            permission_set_refs = self.permission_set_refs

        else:
            permission_set_refs = self.permission_set_refs

        resolved_dispatch_tuning: dict[str, Any] | None | Unset
        if isinstance(self.resolved_dispatch_tuning, Unset):
            resolved_dispatch_tuning = UNSET
        elif isinstance(
            self.resolved_dispatch_tuning, ResolvedWorkQueueDispatchTuningResponse
        ):
            resolved_dispatch_tuning = self.resolved_dispatch_tuning.to_dict()
        else:
            resolved_dispatch_tuning = self.resolved_dispatch_tuning

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "accepting_new_items": accepting_new_items,
                "action_params": action_params,
                "allow_pending_update": allow_pending_update,
                "batch_mode": batch_mode,
                "config": config,
                "created": created,
                "default_priority": default_priority,
                "dispatch_action_ref": dispatch_action_ref,
                "enabled": enabled,
                "id": id,
                "is_adhoc": is_adhoc,
                "item_schema": item_schema,
                "label": label,
                "ref": ref,
                "reference_allowed_pack_refs": reference_allowed_pack_refs,
                "reference_visibility": reference_visibility,
                "update_strategy": update_strategy,
                "updated": updated,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if dispatch_action is not UNSET:
            field_dict["dispatch_action"] = dispatch_action
        if pack is not UNSET:
            field_dict["pack"] = pack
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref
        if permission_set_refs is not UNSET:
            field_dict["permission_set_refs"] = permission_set_refs
        if resolved_dispatch_tuning is not UNSET:
            field_dict["resolved_dispatch_tuning"] = resolved_dispatch_tuning

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.api_response_work_queue_response_data_action_params import (
            ApiResponseWorkQueueResponseDataActionParams,
        )
        from ..models.api_response_work_queue_response_data_config import (
            ApiResponseWorkQueueResponseDataConfig,
        )
        from ..models.api_response_work_queue_response_data_item_schema import (
            ApiResponseWorkQueueResponseDataItemSchema,
        )
        from ..models.resolved_work_queue_dispatch_tuning_response import (
            ResolvedWorkQueueDispatchTuningResponse,
        )

        d = dict(src_dict)
        accepting_new_items = d.pop("accepting_new_items")

        action_params = ApiResponseWorkQueueResponseDataActionParams.from_dict(
            d.pop("action_params")
        )

        allow_pending_update = d.pop("allow_pending_update")

        batch_mode = WorkQueueBatchMode(d.pop("batch_mode"))

        config = ApiResponseWorkQueueResponseDataConfig.from_dict(d.pop("config"))

        created = datetime.datetime.fromisoformat(d.pop("created"))

        default_priority = d.pop("default_priority")

        dispatch_action_ref = d.pop("dispatch_action_ref")

        enabled = d.pop("enabled")

        id = d.pop("id")

        is_adhoc = d.pop("is_adhoc")

        item_schema = ApiResponseWorkQueueResponseDataItemSchema.from_dict(
            d.pop("item_schema")
        )

        label = d.pop("label")

        ref = d.pop("ref")

        reference_allowed_pack_refs = cast(
            list[str], d.pop("reference_allowed_pack_refs")
        )

        reference_visibility = ActionReferenceVisibility(d.pop("reference_visibility"))

        update_strategy = WorkQueueUpdateStrategy(d.pop("update_strategy"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_dispatch_action(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        dispatch_action = _parse_dispatch_action(d.pop("dispatch_action", UNSET))

        def _parse_pack(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        pack = _parse_pack(d.pop("pack", UNSET))

        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

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

        def _parse_resolved_dispatch_tuning(
            data: object,
        ) -> None | ResolvedWorkQueueDispatchTuningResponse | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                resolved_dispatch_tuning_type_1 = (
                    ResolvedWorkQueueDispatchTuningResponse.from_dict(data)
                )

                return resolved_dispatch_tuning_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | ResolvedWorkQueueDispatchTuningResponse | Unset, data)

        resolved_dispatch_tuning = _parse_resolved_dispatch_tuning(
            d.pop("resolved_dispatch_tuning", UNSET)
        )

        api_response_work_queue_response_data = cls(
            accepting_new_items=accepting_new_items,
            action_params=action_params,
            allow_pending_update=allow_pending_update,
            batch_mode=batch_mode,
            config=config,
            created=created,
            default_priority=default_priority,
            dispatch_action_ref=dispatch_action_ref,
            enabled=enabled,
            id=id,
            is_adhoc=is_adhoc,
            item_schema=item_schema,
            label=label,
            ref=ref,
            reference_allowed_pack_refs=reference_allowed_pack_refs,
            reference_visibility=reference_visibility,
            update_strategy=update_strategy,
            updated=updated,
            description=description,
            dispatch_action=dispatch_action,
            pack=pack,
            pack_ref=pack_ref,
            permission_set_refs=permission_set_refs,
            resolved_dispatch_tuning=resolved_dispatch_tuning,
        )

        api_response_work_queue_response_data.additional_properties = d
        return api_response_work_queue_response_data

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
