from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_sensor_token_internal_response_200_data_workload_fence_type_0 import (
        CreateSensorTokenInternalResponse200DataWorkloadFenceType0,
    )


T = TypeVar("T", bound="CreateSensorTokenInternalResponse200Data")


@_attrs_define
class CreateSensorTokenInternalResponse200Data:
    """Response for sensor token creation

    Attributes:
        expires_at (str):
        identity_id (int):
        permission_set_refs (list[str]):
        sensor_ref (str):
        token (str):
        trigger_types (list[str]):
        pack_ref (None | str | Unset):
        workload_fence (CreateSensorTokenInternalResponse200DataWorkloadFenceType0 | None | Unset):
    """

    expires_at: str
    identity_id: int
    permission_set_refs: list[str]
    sensor_ref: str
    token: str
    trigger_types: list[str]
    pack_ref: None | str | Unset = UNSET
    workload_fence: (
        CreateSensorTokenInternalResponse200DataWorkloadFenceType0 | None | Unset
    ) = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.create_sensor_token_internal_response_200_data_workload_fence_type_0 import (
            CreateSensorTokenInternalResponse200DataWorkloadFenceType0,
        )

        expires_at = self.expires_at

        identity_id = self.identity_id

        permission_set_refs = self.permission_set_refs

        sensor_ref = self.sensor_ref

        token = self.token

        trigger_types = self.trigger_types

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref

        workload_fence: dict[str, Any] | None | Unset
        if isinstance(self.workload_fence, Unset):
            workload_fence = UNSET
        elif isinstance(
            self.workload_fence,
            CreateSensorTokenInternalResponse200DataWorkloadFenceType0,
        ):
            workload_fence = self.workload_fence.to_dict()
        else:
            workload_fence = self.workload_fence

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "expires_at": expires_at,
                "identity_id": identity_id,
                "permission_set_refs": permission_set_refs,
                "sensor_ref": sensor_ref,
                "token": token,
                "trigger_types": trigger_types,
            }
        )
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref
        if workload_fence is not UNSET:
            field_dict["workload_fence"] = workload_fence

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.create_sensor_token_internal_response_200_data_workload_fence_type_0 import (
            CreateSensorTokenInternalResponse200DataWorkloadFenceType0,
        )

        d = dict(src_dict)
        expires_at = d.pop("expires_at")

        identity_id = d.pop("identity_id")

        permission_set_refs = cast(list[str], d.pop("permission_set_refs"))

        sensor_ref = d.pop("sensor_ref")

        token = d.pop("token")

        trigger_types = cast(list[str], d.pop("trigger_types"))

        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))

        def _parse_workload_fence(
            data: object,
        ) -> CreateSensorTokenInternalResponse200DataWorkloadFenceType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                workload_fence_type_0 = CreateSensorTokenInternalResponse200DataWorkloadFenceType0.from_dict(
                    data
                )

                return workload_fence_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                CreateSensorTokenInternalResponse200DataWorkloadFenceType0
                | None
                | Unset,
                data,
            )

        workload_fence = _parse_workload_fence(d.pop("workload_fence", UNSET))

        create_sensor_token_internal_response_200_data = cls(
            expires_at=expires_at,
            identity_id=identity_id,
            permission_set_refs=permission_set_refs,
            sensor_ref=sensor_ref,
            token=token,
            trigger_types=trigger_types,
            pack_ref=pack_ref,
            workload_fence=workload_fence,
        )

        create_sensor_token_internal_response_200_data.additional_properties = d
        return create_sensor_token_internal_response_200_data

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
