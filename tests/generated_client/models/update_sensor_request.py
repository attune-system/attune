from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.log_retention_limit_patch_type_0 import LogRetentionLimitPatchType0
    from ..models.log_retention_limit_patch_type_1 import LogRetentionLimitPatchType1
    from ..models.log_retention_policy_patch_type_0 import LogRetentionPolicyPatchType0
    from ..models.log_retention_policy_patch_type_1 import LogRetentionPolicyPatchType1
    from ..models.update_sensor_request_param_schema_type_0 import (
        UpdateSensorRequestParamSchemaType0,
    )
    from ..models.update_sensor_request_worker_selector_type_0 import (
        UpdateSensorRequestWorkerSelectorType0,
    )
    from ..models.worker_affinity import WorkerAffinity
    from ..models.worker_toleration import WorkerToleration


T = TypeVar("T", bound="UpdateSensorRequest")


@_attrs_define
class UpdateSensorRequest:
    """Request DTO for updating a sensor

    Attributes:
        artifact_retention_limit (LogRetentionLimitPatchType0 | LogRetentionLimitPatchType1 | None | Unset):
        artifact_retention_policy (LogRetentionPolicyPatchType0 | LogRetentionPolicyPatchType1 | None | Unset):
        description (None | str | Unset): Sensor description Example: Enhanced CPU monitoring with alerts.
        enabled (bool | None | Unset): Whether the sensor is enabled
        entrypoint (None | str | Unset): Entry point for sensor execution Example:
            /sensors/monitoring/cpu_monitor_v2.py.
        label (None | str | Unset): Human-readable label Example: CPU Monitoring Sensor (Updated).
        log_retention_limit (LogRetentionLimitPatchType0 | LogRetentionLimitPatchType1 | None | Unset):
        log_retention_policy (LogRetentionPolicyPatchType0 | LogRetentionPolicyPatchType1 | None | Unset):
        param_schema (None | Unset | UpdateSensorRequestParamSchemaType0): Parameter schema (StackStorm-style with
            inline required/secret)
        worker_affinity (None | Unset | WorkerAffinity):
        worker_selector (None | Unset | UpdateSensorRequestWorkerSelectorType0): Worker labels required for this sensor
            process.
        worker_tolerations (list[WorkerToleration] | None | Unset): Worker taints tolerated by this sensor process.
    """

    artifact_retention_limit: (
        LogRetentionLimitPatchType0 | LogRetentionLimitPatchType1 | None | Unset
    ) = UNSET
    artifact_retention_policy: (
        LogRetentionPolicyPatchType0 | LogRetentionPolicyPatchType1 | None | Unset
    ) = UNSET
    description: None | str | Unset = UNSET
    enabled: bool | None | Unset = UNSET
    entrypoint: None | str | Unset = UNSET
    label: None | str | Unset = UNSET
    log_retention_limit: (
        LogRetentionLimitPatchType0 | LogRetentionLimitPatchType1 | None | Unset
    ) = UNSET
    log_retention_policy: (
        LogRetentionPolicyPatchType0 | LogRetentionPolicyPatchType1 | None | Unset
    ) = UNSET
    param_schema: None | Unset | UpdateSensorRequestParamSchemaType0 = UNSET
    worker_affinity: None | Unset | WorkerAffinity = UNSET
    worker_selector: None | Unset | UpdateSensorRequestWorkerSelectorType0 = UNSET
    worker_tolerations: list[WorkerToleration] | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.log_retention_limit_patch_type_0 import (
            LogRetentionLimitPatchType0,
        )
        from ..models.log_retention_limit_patch_type_1 import (
            LogRetentionLimitPatchType1,
        )
        from ..models.log_retention_policy_patch_type_0 import (
            LogRetentionPolicyPatchType0,
        )
        from ..models.log_retention_policy_patch_type_1 import (
            LogRetentionPolicyPatchType1,
        )
        from ..models.update_sensor_request_param_schema_type_0 import (
            UpdateSensorRequestParamSchemaType0,
        )
        from ..models.update_sensor_request_worker_selector_type_0 import (
            UpdateSensorRequestWorkerSelectorType0,
        )
        from ..models.worker_affinity import WorkerAffinity

        artifact_retention_limit: dict[str, Any] | None | Unset
        if isinstance(self.artifact_retention_limit, Unset):
            artifact_retention_limit = UNSET
        elif isinstance(self.artifact_retention_limit, LogRetentionLimitPatchType0):
            artifact_retention_limit = self.artifact_retention_limit.to_dict()
        elif isinstance(self.artifact_retention_limit, LogRetentionLimitPatchType1):
            artifact_retention_limit = self.artifact_retention_limit.to_dict()
        else:
            artifact_retention_limit = self.artifact_retention_limit

        artifact_retention_policy: dict[str, Any] | None | Unset
        if isinstance(self.artifact_retention_policy, Unset):
            artifact_retention_policy = UNSET
        elif isinstance(self.artifact_retention_policy, LogRetentionPolicyPatchType0):
            artifact_retention_policy = self.artifact_retention_policy.to_dict()
        elif isinstance(self.artifact_retention_policy, LogRetentionPolicyPatchType1):
            artifact_retention_policy = self.artifact_retention_policy.to_dict()
        else:
            artifact_retention_policy = self.artifact_retention_policy

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        enabled: bool | None | Unset
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

        entrypoint: None | str | Unset
        if isinstance(self.entrypoint, Unset):
            entrypoint = UNSET
        else:
            entrypoint = self.entrypoint

        label: None | str | Unset
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        log_retention_limit: dict[str, Any] | None | Unset
        if isinstance(self.log_retention_limit, Unset):
            log_retention_limit = UNSET
        elif isinstance(self.log_retention_limit, LogRetentionLimitPatchType0):
            log_retention_limit = self.log_retention_limit.to_dict()
        elif isinstance(self.log_retention_limit, LogRetentionLimitPatchType1):
            log_retention_limit = self.log_retention_limit.to_dict()
        else:
            log_retention_limit = self.log_retention_limit

        log_retention_policy: dict[str, Any] | None | Unset
        if isinstance(self.log_retention_policy, Unset):
            log_retention_policy = UNSET
        elif isinstance(self.log_retention_policy, LogRetentionPolicyPatchType0):
            log_retention_policy = self.log_retention_policy.to_dict()
        elif isinstance(self.log_retention_policy, LogRetentionPolicyPatchType1):
            log_retention_policy = self.log_retention_policy.to_dict()
        else:
            log_retention_policy = self.log_retention_policy

        param_schema: dict[str, Any] | None | Unset
        if isinstance(self.param_schema, Unset):
            param_schema = UNSET
        elif isinstance(self.param_schema, UpdateSensorRequestParamSchemaType0):
            param_schema = self.param_schema.to_dict()
        else:
            param_schema = self.param_schema

        worker_affinity: dict[str, Any] | None | Unset
        if isinstance(self.worker_affinity, Unset):
            worker_affinity = UNSET
        elif isinstance(self.worker_affinity, WorkerAffinity):
            worker_affinity = self.worker_affinity.to_dict()
        else:
            worker_affinity = self.worker_affinity

        worker_selector: dict[str, Any] | None | Unset
        if isinstance(self.worker_selector, Unset):
            worker_selector = UNSET
        elif isinstance(self.worker_selector, UpdateSensorRequestWorkerSelectorType0):
            worker_selector = self.worker_selector.to_dict()
        else:
            worker_selector = self.worker_selector

        worker_tolerations: list[dict[str, Any]] | None | Unset
        if isinstance(self.worker_tolerations, Unset):
            worker_tolerations = UNSET
        elif isinstance(self.worker_tolerations, list):
            worker_tolerations = []
            for worker_tolerations_type_0_item_data in self.worker_tolerations:
                worker_tolerations_type_0_item = (
                    worker_tolerations_type_0_item_data.to_dict()
                )
                worker_tolerations.append(worker_tolerations_type_0_item)

        else:
            worker_tolerations = self.worker_tolerations

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if artifact_retention_limit is not UNSET:
            field_dict["artifact_retention_limit"] = artifact_retention_limit
        if artifact_retention_policy is not UNSET:
            field_dict["artifact_retention_policy"] = artifact_retention_policy
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if entrypoint is not UNSET:
            field_dict["entrypoint"] = entrypoint
        if label is not UNSET:
            field_dict["label"] = label
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
        from ..models.log_retention_limit_patch_type_0 import (
            LogRetentionLimitPatchType0,
        )
        from ..models.log_retention_limit_patch_type_1 import (
            LogRetentionLimitPatchType1,
        )
        from ..models.log_retention_policy_patch_type_0 import (
            LogRetentionPolicyPatchType0,
        )
        from ..models.log_retention_policy_patch_type_1 import (
            LogRetentionPolicyPatchType1,
        )
        from ..models.update_sensor_request_param_schema_type_0 import (
            UpdateSensorRequestParamSchemaType0,
        )
        from ..models.update_sensor_request_worker_selector_type_0 import (
            UpdateSensorRequestWorkerSelectorType0,
        )
        from ..models.worker_affinity import WorkerAffinity
        from ..models.worker_toleration import WorkerToleration

        d = dict(src_dict)

        def _parse_artifact_retention_limit(
            data: object,
        ) -> LogRetentionLimitPatchType0 | LogRetentionLimitPatchType1 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_limit_patch_type_0 = (
                    LogRetentionLimitPatchType0.from_dict(data)
                )

                return componentsschemas_log_retention_limit_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_limit_patch_type_1 = (
                    LogRetentionLimitPatchType1.from_dict(data)
                )

                return componentsschemas_log_retention_limit_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                LogRetentionLimitPatchType0
                | LogRetentionLimitPatchType1
                | None
                | Unset,
                data,
            )

        artifact_retention_limit = _parse_artifact_retention_limit(
            d.pop("artifact_retention_limit", UNSET)
        )

        def _parse_artifact_retention_policy(
            data: object,
        ) -> LogRetentionPolicyPatchType0 | LogRetentionPolicyPatchType1 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_policy_patch_type_0 = (
                    LogRetentionPolicyPatchType0.from_dict(data)
                )

                return componentsschemas_log_retention_policy_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_policy_patch_type_1 = (
                    LogRetentionPolicyPatchType1.from_dict(data)
                )

                return componentsschemas_log_retention_policy_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                LogRetentionPolicyPatchType0
                | LogRetentionPolicyPatchType1
                | None
                | Unset,
                data,
            )

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

        def _parse_enabled(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        enabled = _parse_enabled(d.pop("enabled", UNSET))

        def _parse_entrypoint(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        entrypoint = _parse_entrypoint(d.pop("entrypoint", UNSET))

        def _parse_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_log_retention_limit(
            data: object,
        ) -> LogRetentionLimitPatchType0 | LogRetentionLimitPatchType1 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_limit_patch_type_0 = (
                    LogRetentionLimitPatchType0.from_dict(data)
                )

                return componentsschemas_log_retention_limit_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_limit_patch_type_1 = (
                    LogRetentionLimitPatchType1.from_dict(data)
                )

                return componentsschemas_log_retention_limit_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                LogRetentionLimitPatchType0
                | LogRetentionLimitPatchType1
                | None
                | Unset,
                data,
            )

        log_retention_limit = _parse_log_retention_limit(
            d.pop("log_retention_limit", UNSET)
        )

        def _parse_log_retention_policy(
            data: object,
        ) -> LogRetentionPolicyPatchType0 | LogRetentionPolicyPatchType1 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_policy_patch_type_0 = (
                    LogRetentionPolicyPatchType0.from_dict(data)
                )

                return componentsschemas_log_retention_policy_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_policy_patch_type_1 = (
                    LogRetentionPolicyPatchType1.from_dict(data)
                )

                return componentsschemas_log_retention_policy_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                LogRetentionPolicyPatchType0
                | LogRetentionPolicyPatchType1
                | None
                | Unset,
                data,
            )

        log_retention_policy = _parse_log_retention_policy(
            d.pop("log_retention_policy", UNSET)
        )

        def _parse_param_schema(
            data: object,
        ) -> None | Unset | UpdateSensorRequestParamSchemaType0:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                param_schema_type_0 = UpdateSensorRequestParamSchemaType0.from_dict(
                    data
                )

                return param_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UpdateSensorRequestParamSchemaType0, data)

        param_schema = _parse_param_schema(d.pop("param_schema", UNSET))

        def _parse_worker_affinity(data: object) -> None | Unset | WorkerAffinity:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                worker_affinity_type_1 = WorkerAffinity.from_dict(data)

                return worker_affinity_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | WorkerAffinity, data)

        worker_affinity = _parse_worker_affinity(d.pop("worker_affinity", UNSET))

        def _parse_worker_selector(
            data: object,
        ) -> None | Unset | UpdateSensorRequestWorkerSelectorType0:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                worker_selector_type_0 = (
                    UpdateSensorRequestWorkerSelectorType0.from_dict(data)
                )

                return worker_selector_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UpdateSensorRequestWorkerSelectorType0, data)

        worker_selector = _parse_worker_selector(d.pop("worker_selector", UNSET))

        def _parse_worker_tolerations(
            data: object,
        ) -> list[WorkerToleration] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                worker_tolerations_type_0 = []
                _worker_tolerations_type_0 = data
                for worker_tolerations_type_0_item_data in _worker_tolerations_type_0:
                    worker_tolerations_type_0_item = WorkerToleration.from_dict(
                        worker_tolerations_type_0_item_data
                    )

                    worker_tolerations_type_0.append(worker_tolerations_type_0_item)

                return worker_tolerations_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[WorkerToleration] | None | Unset, data)

        worker_tolerations = _parse_worker_tolerations(
            d.pop("worker_tolerations", UNSET)
        )

        update_sensor_request = cls(
            artifact_retention_limit=artifact_retention_limit,
            artifact_retention_policy=artifact_retention_policy,
            description=description,
            enabled=enabled,
            entrypoint=entrypoint,
            label=label,
            log_retention_limit=log_retention_limit,
            log_retention_policy=log_retention_policy,
            param_schema=param_schema,
            worker_affinity=worker_affinity,
            worker_selector=worker_selector,
            worker_tolerations=worker_tolerations,
        )

        update_sensor_request.additional_properties = d
        return update_sensor_request

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
