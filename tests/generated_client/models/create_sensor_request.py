from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.retention_policy_type import RetentionPolicyType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_sensor_request_config_type_0 import (
        CreateSensorRequestConfigType0,
    )
    from ..models.create_sensor_request_param_schema_type_0 import (
        CreateSensorRequestParamSchemaType0,
    )
    from ..models.create_sensor_request_worker_selector import (
        CreateSensorRequestWorkerSelector,
    )
    from ..models.worker_affinity import WorkerAffinity
    from ..models.worker_toleration import WorkerToleration


T = TypeVar("T", bound="CreateSensorRequest")


@_attrs_define
class CreateSensorRequest:
    """Request DTO for creating a new sensor

    Attributes:
        entrypoint (str): Entry point for sensor execution (e.g., path to script, function name) Example:
            /sensors/monitoring/cpu_monitor.py.
        label (str): Human-readable label Example: CPU Monitoring Sensor.
        pack_ref (str): Pack reference this sensor belongs to Example: monitoring.
        ref (str): Unique reference identifier (e.g., "mypack.cpu_monitor") Example: monitoring.cpu_sensor.
        runtime_ref (str): Runtime reference for this sensor Example: python3.
        trigger_ref (str): Trigger reference this sensor monitors for Example: monitoring.cpu_threshold.
        artifact_retention_limit (int | None | Unset): Optional per-sensor retention limit override for non-log
            artifacts created by sensor-owned executions. Example: 10.
        artifact_retention_policy (None | RetentionPolicyType | Unset):
        config (CreateSensorRequestConfigType0 | None | Unset): Configuration values for this sensor instance (conforms
            to param_schema)
        description (None | str | Unset): Sensor description Example: Monitors CPU usage and generates events.
        enabled (bool | Unset): Whether the sensor is enabled Example: True.
        log_retention_limit (int | None | Unset): Optional per-sensor retention limit override for registered
            stdout/stderr log artifacts. Example: 4.
        log_retention_policy (None | RetentionPolicyType | Unset):
        param_schema (CreateSensorRequestParamSchemaType0 | None | Unset): Parameter schema (flat format) for sensor
            configuration
        worker_affinity (WorkerAffinity | Unset):
        worker_selector (CreateSensorRequestWorkerSelector | Unset): Worker labels required for this sensor process.
        worker_tolerations (list[WorkerToleration] | Unset): Worker taints tolerated by this sensor process.
    """

    entrypoint: str
    label: str
    pack_ref: str
    ref: str
    runtime_ref: str
    trigger_ref: str
    artifact_retention_limit: int | None | Unset = UNSET
    artifact_retention_policy: None | RetentionPolicyType | Unset = UNSET
    config: CreateSensorRequestConfigType0 | None | Unset = UNSET
    description: None | str | Unset = UNSET
    enabled: bool | Unset = UNSET
    log_retention_limit: int | None | Unset = UNSET
    log_retention_policy: None | RetentionPolicyType | Unset = UNSET
    param_schema: CreateSensorRequestParamSchemaType0 | None | Unset = UNSET
    worker_affinity: WorkerAffinity | Unset = UNSET
    worker_selector: CreateSensorRequestWorkerSelector | Unset = UNSET
    worker_tolerations: list[WorkerToleration] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.create_sensor_request_config_type_0 import (
            CreateSensorRequestConfigType0,
        )
        from ..models.create_sensor_request_param_schema_type_0 import (
            CreateSensorRequestParamSchemaType0,
        )

        entrypoint = self.entrypoint

        label = self.label

        pack_ref = self.pack_ref

        ref = self.ref

        runtime_ref = self.runtime_ref

        trigger_ref = self.trigger_ref

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

        config: dict[str, Any] | None | Unset
        if isinstance(self.config, Unset):
            config = UNSET
        elif isinstance(self.config, CreateSensorRequestConfigType0):
            config = self.config.to_dict()
        else:
            config = self.config

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        enabled = self.enabled

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

        param_schema: dict[str, Any] | None | Unset
        if isinstance(self.param_schema, Unset):
            param_schema = UNSET
        elif isinstance(self.param_schema, CreateSensorRequestParamSchemaType0):
            param_schema = self.param_schema.to_dict()
        else:
            param_schema = self.param_schema

        worker_affinity: dict[str, Any] | Unset = UNSET
        if not isinstance(self.worker_affinity, Unset):
            worker_affinity = self.worker_affinity.to_dict()

        worker_selector: dict[str, Any] | Unset = UNSET
        if not isinstance(self.worker_selector, Unset):
            worker_selector = self.worker_selector.to_dict()

        worker_tolerations: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.worker_tolerations, Unset):
            worker_tolerations = []
            for worker_tolerations_item_data in self.worker_tolerations:
                worker_tolerations_item = worker_tolerations_item_data.to_dict()
                worker_tolerations.append(worker_tolerations_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "entrypoint": entrypoint,
                "label": label,
                "pack_ref": pack_ref,
                "ref": ref,
                "runtime_ref": runtime_ref,
                "trigger_ref": trigger_ref,
            }
        )
        if artifact_retention_limit is not UNSET:
            field_dict["artifact_retention_limit"] = artifact_retention_limit
        if artifact_retention_policy is not UNSET:
            field_dict["artifact_retention_policy"] = artifact_retention_policy
        if config is not UNSET:
            field_dict["config"] = config
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if log_retention_limit is not UNSET:
            field_dict["log_retention_limit"] = log_retention_limit
        if log_retention_policy is not UNSET:
            field_dict["log_retention_policy"] = log_retention_policy
        if param_schema is not UNSET:
            field_dict["param_schema"] = param_schema
        if worker_affinity is not UNSET:
            field_dict["worker_affinity"] = worker_affinity
        if worker_selector is not UNSET:
            field_dict["worker_selector"] = worker_selector
        if worker_tolerations is not UNSET:
            field_dict["worker_tolerations"] = worker_tolerations

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.create_sensor_request_config_type_0 import (
            CreateSensorRequestConfigType0,
        )
        from ..models.create_sensor_request_param_schema_type_0 import (
            CreateSensorRequestParamSchemaType0,
        )
        from ..models.create_sensor_request_worker_selector import (
            CreateSensorRequestWorkerSelector,
        )
        from ..models.worker_affinity import WorkerAffinity
        from ..models.worker_toleration import WorkerToleration

        d = dict(src_dict)
        entrypoint = d.pop("entrypoint")

        label = d.pop("label")

        pack_ref = d.pop("pack_ref")

        ref = d.pop("ref")

        runtime_ref = d.pop("runtime_ref")

        trigger_ref = d.pop("trigger_ref")

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

        def _parse_config(
            data: object,
        ) -> CreateSensorRequestConfigType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                config_type_0 = CreateSensorRequestConfigType0.from_dict(data)

                return config_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateSensorRequestConfigType0 | None | Unset, data)

        config = _parse_config(d.pop("config", UNSET))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        enabled = d.pop("enabled", UNSET)

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

        def _parse_param_schema(
            data: object,
        ) -> CreateSensorRequestParamSchemaType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                param_schema_type_0 = CreateSensorRequestParamSchemaType0.from_dict(
                    data
                )

                return param_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateSensorRequestParamSchemaType0 | None | Unset, data)

        param_schema = _parse_param_schema(d.pop("param_schema", UNSET))

        _worker_affinity = d.pop("worker_affinity", UNSET)
        worker_affinity: WorkerAffinity | Unset
        if isinstance(_worker_affinity, Unset):
            worker_affinity = UNSET
        else:
            worker_affinity = WorkerAffinity.from_dict(_worker_affinity)

        _worker_selector = d.pop("worker_selector", UNSET)
        worker_selector: CreateSensorRequestWorkerSelector | Unset
        if isinstance(_worker_selector, Unset):
            worker_selector = UNSET
        else:
            worker_selector = CreateSensorRequestWorkerSelector.from_dict(
                _worker_selector
            )

        _worker_tolerations = d.pop("worker_tolerations", UNSET)
        worker_tolerations: list[WorkerToleration] | Unset = UNSET
        if _worker_tolerations is not UNSET:
            worker_tolerations = []
            for worker_tolerations_item_data in _worker_tolerations:
                worker_tolerations_item = WorkerToleration.from_dict(
                    worker_tolerations_item_data
                )

                worker_tolerations.append(worker_tolerations_item)

        create_sensor_request = cls(
            entrypoint=entrypoint,
            label=label,
            pack_ref=pack_ref,
            ref=ref,
            runtime_ref=runtime_ref,
            trigger_ref=trigger_ref,
            artifact_retention_limit=artifact_retention_limit,
            artifact_retention_policy=artifact_retention_policy,
            config=config,
            description=description,
            enabled=enabled,
            log_retention_limit=log_retention_limit,
            log_retention_policy=log_retention_policy,
            param_schema=param_schema,
            worker_affinity=worker_affinity,
            worker_selector=worker_selector,
            worker_tolerations=worker_tolerations,
        )

        create_sensor_request.additional_properties = d
        return create_sensor_request

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
