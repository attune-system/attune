from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.retention_policy_type import RetentionPolicyType
from ..types import UNSET, Unset

T = TypeVar("T", bound="SensorSummary")


@_attrs_define
class SensorSummary:
    """Simplified sensor response (for list endpoints)

    Attributes:
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        enabled (bool): Whether the sensor is enabled Example: True.
        id (int): Sensor ID Example: 1.
        label (str): Human-readable label Example: CPU Monitoring Sensor.
        ref (str): Unique reference identifier Example: monitoring.cpu_sensor.
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:30:00Z.
        artifact_retention_limit (int | None | Unset): Per-sensor retention limit override for non-log artifacts created
            by sensor-owned executions. Example: 10.
        artifact_retention_policy (None | RetentionPolicyType | Unset):
        description (None | str | Unset): Sensor description Example: Monitors CPU usage and generates events.
        log_retention_limit (int | None | Unset): Per-sensor retention limit override for registered stdout/stderr log
            artifacts. Example: 4.
        log_retention_policy (None | RetentionPolicyType | Unset):
        pack_ref (None | str | Unset): Pack reference (optional) Example: monitoring.
    """

    created: datetime.datetime
    enabled: bool
    id: int
    label: str
    ref: str
    updated: datetime.datetime
    artifact_retention_limit: int | None | Unset = UNSET
    artifact_retention_policy: None | RetentionPolicyType | Unset = UNSET
    description: None | str | Unset = UNSET
    log_retention_limit: int | None | Unset = UNSET
    log_retention_policy: None | RetentionPolicyType | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        enabled = self.enabled

        id = self.id

        label = self.label

        ref = self.ref

        updated = self.updated.isoformat()

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
                "id": id,
                "label": label,
                "ref": ref,
                "updated": updated,
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
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        enabled = d.pop("enabled")

        id = d.pop("id")

        label = d.pop("label")

        ref = d.pop("ref")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

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

        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))

        sensor_summary = cls(
            created=created,
            enabled=enabled,
            id=id,
            label=label,
            ref=ref,
            updated=updated,
            artifact_retention_limit=artifact_retention_limit,
            artifact_retention_policy=artifact_retention_policy,
            description=description,
            log_retention_limit=log_retention_limit,
            log_retention_policy=log_retention_policy,
            pack_ref=pack_ref,
        )

        sensor_summary.additional_properties = d
        return sensor_summary

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
