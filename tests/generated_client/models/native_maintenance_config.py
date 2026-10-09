from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="NativeMaintenanceConfig")


@_attrs_define
class NativeMaintenanceConfig:
    """Bounded native partition and hourly-summary maintenance.

    Attributes:
        default_repair_row_limit (int | Unset):  Default: 1000.
        enabled (bool | Unset):  Default: True.
        lock_timeout_milliseconds (int | Unset):  Default: 250.
        max_partition_cycle_milliseconds (int | Unset):  Default: 5000.
        max_partition_operations_per_cycle (int | Unset):  Default: 32.
        max_summary_buckets_per_cycle (int | Unset):  Default: 128.
        max_summary_cycle_milliseconds (int | Unset):  Default: 5000.
        max_summary_invalidations_per_bucket (int | Unset):  Default: 10000.
        operation_timeout_milliseconds (int | Unset):  Default: 1000.
        partition_interval_seconds (int | Unset):  Default: 3600.
        partition_lookahead_days (int | Unset):  Default: 7.
        summary_bootstrap_hours (int | Unset):  Default: 24.
        summary_interval_seconds (int | Unset):  Default: 300.
    """

    default_repair_row_limit: int | Unset = 1000
    enabled: bool | Unset = True
    lock_timeout_milliseconds: int | Unset = 250
    max_partition_cycle_milliseconds: int | Unset = 5000
    max_partition_operations_per_cycle: int | Unset = 32
    max_summary_buckets_per_cycle: int | Unset = 128
    max_summary_cycle_milliseconds: int | Unset = 5000
    max_summary_invalidations_per_bucket: int | Unset = 10000
    operation_timeout_milliseconds: int | Unset = 1000
    partition_interval_seconds: int | Unset = 3600
    partition_lookahead_days: int | Unset = 7
    summary_bootstrap_hours: int | Unset = 24
    summary_interval_seconds: int | Unset = 300
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        default_repair_row_limit = self.default_repair_row_limit

        enabled = self.enabled

        lock_timeout_milliseconds = self.lock_timeout_milliseconds

        max_partition_cycle_milliseconds = self.max_partition_cycle_milliseconds

        max_partition_operations_per_cycle = self.max_partition_operations_per_cycle

        max_summary_buckets_per_cycle = self.max_summary_buckets_per_cycle

        max_summary_cycle_milliseconds = self.max_summary_cycle_milliseconds

        max_summary_invalidations_per_bucket = self.max_summary_invalidations_per_bucket

        operation_timeout_milliseconds = self.operation_timeout_milliseconds

        partition_interval_seconds = self.partition_interval_seconds

        partition_lookahead_days = self.partition_lookahead_days

        summary_bootstrap_hours = self.summary_bootstrap_hours

        summary_interval_seconds = self.summary_interval_seconds

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if default_repair_row_limit is not UNSET:
            field_dict["default_repair_row_limit"] = default_repair_row_limit
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if lock_timeout_milliseconds is not UNSET:
            field_dict["lock_timeout_milliseconds"] = lock_timeout_milliseconds
        if max_partition_cycle_milliseconds is not UNSET:
            field_dict["max_partition_cycle_milliseconds"] = (
                max_partition_cycle_milliseconds
            )
        if max_partition_operations_per_cycle is not UNSET:
            field_dict["max_partition_operations_per_cycle"] = (
                max_partition_operations_per_cycle
            )
        if max_summary_buckets_per_cycle is not UNSET:
            field_dict["max_summary_buckets_per_cycle"] = max_summary_buckets_per_cycle
        if max_summary_cycle_milliseconds is not UNSET:
            field_dict["max_summary_cycle_milliseconds"] = (
                max_summary_cycle_milliseconds
            )
        if max_summary_invalidations_per_bucket is not UNSET:
            field_dict["max_summary_invalidations_per_bucket"] = (
                max_summary_invalidations_per_bucket
            )
        if operation_timeout_milliseconds is not UNSET:
            field_dict["operation_timeout_milliseconds"] = (
                operation_timeout_milliseconds
            )
        if partition_interval_seconds is not UNSET:
            field_dict["partition_interval_seconds"] = partition_interval_seconds
        if partition_lookahead_days is not UNSET:
            field_dict["partition_lookahead_days"] = partition_lookahead_days
        if summary_bootstrap_hours is not UNSET:
            field_dict["summary_bootstrap_hours"] = summary_bootstrap_hours
        if summary_interval_seconds is not UNSET:
            field_dict["summary_interval_seconds"] = summary_interval_seconds

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        default_repair_row_limit = d.pop("default_repair_row_limit", UNSET)

        enabled = d.pop("enabled", UNSET)

        lock_timeout_milliseconds = d.pop("lock_timeout_milliseconds", UNSET)

        max_partition_cycle_milliseconds = d.pop(
            "max_partition_cycle_milliseconds", UNSET
        )

        max_partition_operations_per_cycle = d.pop(
            "max_partition_operations_per_cycle", UNSET
        )

        max_summary_buckets_per_cycle = d.pop("max_summary_buckets_per_cycle", UNSET)

        max_summary_cycle_milliseconds = d.pop("max_summary_cycle_milliseconds", UNSET)

        max_summary_invalidations_per_bucket = d.pop(
            "max_summary_invalidations_per_bucket", UNSET
        )

        operation_timeout_milliseconds = d.pop("operation_timeout_milliseconds", UNSET)

        partition_interval_seconds = d.pop("partition_interval_seconds", UNSET)

        partition_lookahead_days = d.pop("partition_lookahead_days", UNSET)

        summary_bootstrap_hours = d.pop("summary_bootstrap_hours", UNSET)

        summary_interval_seconds = d.pop("summary_interval_seconds", UNSET)

        native_maintenance_config = cls(
            default_repair_row_limit=default_repair_row_limit,
            enabled=enabled,
            lock_timeout_milliseconds=lock_timeout_milliseconds,
            max_partition_cycle_milliseconds=max_partition_cycle_milliseconds,
            max_partition_operations_per_cycle=max_partition_operations_per_cycle,
            max_summary_buckets_per_cycle=max_summary_buckets_per_cycle,
            max_summary_cycle_milliseconds=max_summary_cycle_milliseconds,
            max_summary_invalidations_per_bucket=max_summary_invalidations_per_bucket,
            operation_timeout_milliseconds=operation_timeout_milliseconds,
            partition_interval_seconds=partition_interval_seconds,
            partition_lookahead_days=partition_lookahead_days,
            summary_bootstrap_hours=summary_bootstrap_hours,
            summary_interval_seconds=summary_interval_seconds,
        )

        native_maintenance_config.additional_properties = d
        return native_maintenance_config

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
