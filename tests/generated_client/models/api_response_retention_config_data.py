from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.retention_targets_config import RetentionTargetsConfig


T = TypeVar("T", bound="ApiResponseRetentionConfigData")


@_attrs_define
class ApiResponseRetentionConfigData:
    """Supervisor-owned runtime retention configuration.

    Attributes:
        advisory_lock_key (int | Unset): Advisory lock key used to make accidental multi-supervisor deployments safe.
        batch_size (int | Unset): Maximum rows to delete per target per cycle for regular tables.
        check_interval_seconds (int | Unset): How often the supervisor runs retention, in seconds.
        dry_run (bool | Unset): Report candidates without deleting rows/chunks.
        enabled (bool | Unset): Enable runtime row retention globally.
        targets (RetentionTargetsConfig | Unset): Per-table runtime retention targets.
    """

    advisory_lock_key: int | Unset = UNSET
    batch_size: int | Unset = UNSET
    check_interval_seconds: int | Unset = UNSET
    dry_run: bool | Unset = UNSET
    enabled: bool | Unset = UNSET
    targets: RetentionTargetsConfig | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        advisory_lock_key = self.advisory_lock_key

        batch_size = self.batch_size

        check_interval_seconds = self.check_interval_seconds

        dry_run = self.dry_run

        enabled = self.enabled

        targets: dict[str, Any] | Unset = UNSET
        if not isinstance(self.targets, Unset):
            targets = self.targets.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if advisory_lock_key is not UNSET:
            field_dict["advisory_lock_key"] = advisory_lock_key
        if batch_size is not UNSET:
            field_dict["batch_size"] = batch_size
        if check_interval_seconds is not UNSET:
            field_dict["check_interval_seconds"] = check_interval_seconds
        if dry_run is not UNSET:
            field_dict["dry_run"] = dry_run
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if targets is not UNSET:
            field_dict["targets"] = targets

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.retention_targets_config import RetentionTargetsConfig

        d = dict(src_dict)
        advisory_lock_key = d.pop("advisory_lock_key", UNSET)

        batch_size = d.pop("batch_size", UNSET)

        check_interval_seconds = d.pop("check_interval_seconds", UNSET)

        dry_run = d.pop("dry_run", UNSET)

        enabled = d.pop("enabled", UNSET)

        _targets = d.pop("targets", UNSET)
        targets: RetentionTargetsConfig | Unset
        if isinstance(_targets, Unset):
            targets = UNSET
        else:
            targets = RetentionTargetsConfig.from_dict(_targets)

        api_response_retention_config_data = cls(
            advisory_lock_key=advisory_lock_key,
            batch_size=batch_size,
            check_interval_seconds=check_interval_seconds,
            dry_run=dry_run,
            enabled=enabled,
            targets=targets,
        )

        api_response_retention_config_data.additional_properties = d
        return api_response_retention_config_data

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
