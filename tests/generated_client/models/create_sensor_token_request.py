from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="CreateSensorTokenRequest")


@_attrs_define
class CreateSensorTokenRequest:
    """Request body for creating sensor tokens

    Attributes:
        sensor_ref (str): Sensor reference (e.g., "core.timer")
        trigger_types (list[str]): List of trigger types this sensor can create events for
        pack_ref (None | str | Unset): Registered pack reference. Internal worker callers must provide it;
            public callers may omit it and let the API resolve it.
        permission_set_refs (list[str] | Unset): Explicit sensor cache permission-set refs. `standard` grants read-only
            access to the registered sensor and pack cache scopes.
        ttl_seconds (int | None | Unset): Optional TTL in seconds (default: 86400 = 24 hours, max: 259200 = 72 hours)
    """

    sensor_ref: str
    trigger_types: list[str]
    pack_ref: None | str | Unset = UNSET
    permission_set_refs: list[str] | Unset = UNSET
    ttl_seconds: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        sensor_ref = self.sensor_ref

        trigger_types = self.trigger_types

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref

        permission_set_refs: list[str] | Unset = UNSET
        if not isinstance(self.permission_set_refs, Unset):
            permission_set_refs = self.permission_set_refs

        ttl_seconds: int | None | Unset
        if isinstance(self.ttl_seconds, Unset):
            ttl_seconds = UNSET
        else:
            ttl_seconds = self.ttl_seconds

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "sensor_ref": sensor_ref,
                "trigger_types": trigger_types,
            }
        )
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref
        if permission_set_refs is not UNSET:
            field_dict["permission_set_refs"] = permission_set_refs
        if ttl_seconds is not UNSET:
            field_dict["ttl_seconds"] = ttl_seconds

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        sensor_ref = d.pop("sensor_ref")

        trigger_types = cast(list[str], d.pop("trigger_types"))

        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))

        permission_set_refs = cast(list[str], d.pop("permission_set_refs", UNSET))

        def _parse_ttl_seconds(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        ttl_seconds = _parse_ttl_seconds(d.pop("ttl_seconds", UNSET))

        create_sensor_token_request = cls(
            sensor_ref=sensor_ref,
            trigger_types=trigger_types,
            pack_ref=pack_ref,
            permission_set_refs=permission_set_refs,
            ttl_seconds=ttl_seconds,
        )

        create_sensor_token_request.additional_properties = d
        return create_sensor_token_request

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
