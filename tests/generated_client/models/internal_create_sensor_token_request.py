from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="InternalCreateSensorTokenRequest")


@_attrs_define
class InternalCreateSensorTokenRequest:
    """Request body for internal sensor token creation/reissue.

    Worker/service tokens must provide `sensor_ref` and `trigger_types`.
    Sensor-token refresh calls may omit those fields; the server will derive them
    from authenticated identity state.

        Attributes:
            pack_ref (None | str | Unset): Registered pack reference (required for worker/service callers).
            permission_set_refs (list[str] | None | Unset): Explicit cache permission-set refs (required, though it may be
                empty,
                for worker/service callers).
            sensor_ref (None | str | Unset): Sensor reference (required for worker/service callers)
            trigger_types (list[str] | None | Unset): List of trigger types this sensor can create events for (required for
                worker/service callers)
            ttl_seconds (int | None | Unset): Optional TTL in seconds (default: 86400 = 24 hours, max: 259200 = 72 hours)
    """

    pack_ref: None | str | Unset = UNSET
    permission_set_refs: list[str] | None | Unset = UNSET
    sensor_ref: None | str | Unset = UNSET
    trigger_types: list[str] | None | Unset = UNSET
    ttl_seconds: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
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

        sensor_ref: None | str | Unset
        if isinstance(self.sensor_ref, Unset):
            sensor_ref = UNSET
        else:
            sensor_ref = self.sensor_ref

        trigger_types: list[str] | None | Unset
        if isinstance(self.trigger_types, Unset):
            trigger_types = UNSET
        elif isinstance(self.trigger_types, list):
            trigger_types = self.trigger_types

        else:
            trigger_types = self.trigger_types

        ttl_seconds: int | None | Unset
        if isinstance(self.ttl_seconds, Unset):
            ttl_seconds = UNSET
        else:
            ttl_seconds = self.ttl_seconds

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref
        if permission_set_refs is not UNSET:
            field_dict["permission_set_refs"] = permission_set_refs
        if sensor_ref is not UNSET:
            field_dict["sensor_ref"] = sensor_ref
        if trigger_types is not UNSET:
            field_dict["trigger_types"] = trigger_types
        if ttl_seconds is not UNSET:
            field_dict["ttl_seconds"] = ttl_seconds

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)

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

        def _parse_sensor_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        sensor_ref = _parse_sensor_ref(d.pop("sensor_ref", UNSET))

        def _parse_trigger_types(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                trigger_types_type_0 = cast(list[str], data)

                return trigger_types_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        trigger_types = _parse_trigger_types(d.pop("trigger_types", UNSET))

        def _parse_ttl_seconds(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        ttl_seconds = _parse_ttl_seconds(d.pop("ttl_seconds", UNSET))

        internal_create_sensor_token_request = cls(
            pack_ref=pack_ref,
            permission_set_refs=permission_set_refs,
            sensor_ref=sensor_ref,
            trigger_types=trigger_types,
            ttl_seconds=ttl_seconds,
        )

        internal_create_sensor_token_request.additional_properties = d
        return internal_create_sensor_token_request

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
