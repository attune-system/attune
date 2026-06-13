from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.retention_policy_type import RetentionPolicyType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.sensor_response_param_schema_type_0 import (
        SensorResponseParamSchemaType0,
    )
    from ..models.sensor_response_worker_selector import SensorResponseWorkerSelector
    from ..models.worker_affinity import WorkerAffinity
    from ..models.worker_toleration import WorkerToleration


T = TypeVar("T", bound="SensorResponse")


@_attrs_define
class SensorResponse:
    """Response DTO for sensor information

    Attributes:
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        enabled (bool): Whether the sensor is enabled Example: True.
        entrypoint (str): Entry point Example: /sensors/monitoring/cpu_monitor.py.
        id (int): Sensor ID Example: 1.
        label (str): Human-readable label Example: CPU Monitoring Sensor.
        param_schema (None | SensorResponseParamSchemaType0): Parameter schema (StackStorm-style with inline
            required/secret)
        ref (str): Unique reference identifier Example: monitoring.cpu_sensor.
        runtime (int): Runtime ID Example: 1.
        runtime_ref (str): Runtime reference Example: python3.
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:30:00Z.
        worker_affinity (WorkerAffinity):
        worker_selector (SensorResponseWorkerSelector): Worker labels required for this sensor process.
        worker_tolerations (list[WorkerToleration]): Worker taints tolerated by this sensor process.
        artifact_retention_limit (int | None | Unset): Per-sensor retention limit override for non-log artifacts created
            by sensor-owned executions. Example: 10.
        artifact_retention_policy (None | RetentionPolicyType | Unset):
        description (None | str | Unset): Sensor description Example: Monitors CPU usage and generates events.
        log_retention_limit (int | None | Unset): Per-sensor retention limit override for registered stdout/stderr log
            artifacts. Example: 4.
        log_retention_policy (None | RetentionPolicyType | Unset):
        pack (int | None | Unset): Pack ID (optional) Example: 1.
        pack_ref (None | str | Unset): Pack reference (optional) Example: monitoring.
    """

    created: datetime.datetime
    enabled: bool
    entrypoint: str
    id: int
    label: str
    param_schema: None | SensorResponseParamSchemaType0
    ref: str
    runtime: int
    runtime_ref: str
    updated: datetime.datetime
    worker_affinity: WorkerAffinity
    worker_selector: SensorResponseWorkerSelector
    worker_tolerations: list[WorkerToleration]
    artifact_retention_limit: int | None | Unset = UNSET
    artifact_retention_policy: None | RetentionPolicyType | Unset = UNSET
    description: None | str | Unset = UNSET
    log_retention_limit: int | None | Unset = UNSET
    log_retention_policy: None | RetentionPolicyType | Unset = UNSET
    pack: int | None | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.sensor_response_param_schema_type_0 import (
            SensorResponseParamSchemaType0,
        )

        created = self.created.isoformat()

        enabled = self.enabled

        entrypoint = self.entrypoint

        id = self.id

        label = self.label

        param_schema: dict[str, Any] | None
        if isinstance(self.param_schema, SensorResponseParamSchemaType0):
            param_schema = self.param_schema.to_dict()
        else:
            param_schema = self.param_schema

        ref = self.ref

        runtime = self.runtime

        runtime_ref = self.runtime_ref

        updated = self.updated.isoformat()

        worker_affinity = self.worker_affinity.to_dict()

        worker_selector = self.worker_selector.to_dict()

        worker_tolerations = []
        for worker_tolerations_item_data in self.worker_tolerations:
            worker_tolerations_item = worker_tolerations_item_data.to_dict()
            worker_tolerations.append(worker_tolerations_item)

        artifact_retention_limit: int | None | Unset
        if isinstance(self.artifact_retention_limit, Unset):
            artifact_retention_limit = UNSET
        else:
            artifact_retention_limit = self.artifact_retention_limit

        artifact_retention_policy: None | str | Unset
        if isinstance(self.artifact_retention_policy, Unset):
            artifact_retention_policy = UNSET
        elif isinstance(self.artifact_retention_policy, RetentionPolicyType):
            artifact_retention_policy = self.artifact_retention_policy.value
        else:
            artifact_retention_policy = self.artifact_retention_policy

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        log_retention_limit: int | None | Unset
        if isinstance(self.log_retention_limit, Unset):
            log_retention_limit = UNSET
        else:
            log_retention_limit = self.log_retention_limit

        log_retention_policy: None | str | Unset
        if isinstance(self.log_retention_policy, Unset):
            log_retention_policy = UNSET
        elif isinstance(self.log_retention_policy, RetentionPolicyType):
            log_retention_policy = self.log_retention_policy.value
        else:
            log_retention_policy = self.log_retention_policy

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

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "enabled": enabled,
                "entrypoint": entrypoint,
                "id": id,
                "label": label,
                "param_schema": param_schema,
                "ref": ref,
                "runtime": runtime,
                "runtime_ref": runtime_ref,
                "updated": updated,
                "worker_affinity": worker_affinity,
                "worker_selector": worker_selector,
                "worker_tolerations": worker_tolerations,
            }
        )
        if artifact_retention_limit is not UNSET:
            field_dict["artifact_retention_limit"] = artifact_retention_limit
        if artifact_retention_policy is not UNSET:
            field_dict["artifact_retention_policy"] = artifact_retention_policy
        if description is not UNSET:
            field_dict["description"] = description
        if log_retention_limit is not UNSET:
            field_dict["log_retention_limit"] = log_retention_limit
        if log_retention_policy is not UNSET:
            field_dict["log_retention_policy"] = log_retention_policy
        if pack is not UNSET:
            field_dict["pack"] = pack
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.sensor_response_param_schema_type_0 import (
            SensorResponseParamSchemaType0,
        )
        from ..models.sensor_response_worker_selector import (
            SensorResponseWorkerSelector,
        )
        from ..models.worker_affinity import WorkerAffinity
        from ..models.worker_toleration import WorkerToleration

        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        enabled = d.pop("enabled")

        entrypoint = d.pop("entrypoint")

        id = d.pop("id")

        label = d.pop("label")

        def _parse_param_schema(data: object) -> None | SensorResponseParamSchemaType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                param_schema_type_0 = SensorResponseParamSchemaType0.from_dict(data)

                return param_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | SensorResponseParamSchemaType0, data)

        param_schema = _parse_param_schema(d.pop("param_schema"))

        ref = d.pop("ref")

        runtime = d.pop("runtime")

        runtime_ref = d.pop("runtime_ref")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        worker_affinity = WorkerAffinity.from_dict(d.pop("worker_affinity"))

        worker_selector = SensorResponseWorkerSelector.from_dict(
            d.pop("worker_selector")
        )

        worker_tolerations = []
        _worker_tolerations = d.pop("worker_tolerations")
        for worker_tolerations_item_data in _worker_tolerations:
            worker_tolerations_item = WorkerToleration.from_dict(
                worker_tolerations_item_data
            )

            worker_tolerations.append(worker_tolerations_item)

        def _parse_artifact_retention_limit(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        artifact_retention_limit = _parse_artifact_retention_limit(
            d.pop("artifact_retention_limit", UNSET)
        )

        def _parse_artifact_retention_policy(
            data: object,
        ) -> None | RetentionPolicyType | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                artifact_retention_policy_type_1 = RetentionPolicyType(data)

                return artifact_retention_policy_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | RetentionPolicyType | Unset, data)

        artifact_retention_policy = _parse_artifact_retention_policy(
            d.pop("artifact_retention_policy", UNSET)
        )

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_log_retention_limit(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        log_retention_limit = _parse_log_retention_limit(
            d.pop("log_retention_limit", UNSET)
        )

        def _parse_log_retention_policy(
            data: object,
        ) -> None | RetentionPolicyType | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                log_retention_policy_type_1 = RetentionPolicyType(data)

                return log_retention_policy_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | RetentionPolicyType | Unset, data)

        log_retention_policy = _parse_log_retention_policy(
            d.pop("log_retention_policy", UNSET)
        )

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

        sensor_response = cls(
            created=created,
            enabled=enabled,
            entrypoint=entrypoint,
            id=id,
            label=label,
            param_schema=param_schema,
            ref=ref,
            runtime=runtime,
            runtime_ref=runtime_ref,
            updated=updated,
            worker_affinity=worker_affinity,
            worker_selector=worker_selector,
            worker_tolerations=worker_tolerations,
            artifact_retention_limit=artifact_retention_limit,
            artifact_retention_policy=artifact_retention_policy,
            description=description,
            log_retention_limit=log_retention_limit,
            log_retention_policy=log_retention_policy,
            pack=pack,
            pack_ref=pack_ref,
        )

        sensor_response.additional_properties = d
        return sensor_response

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
