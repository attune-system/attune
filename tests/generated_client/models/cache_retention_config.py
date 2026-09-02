from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="CacheRetentionConfig")


@_attrs_define
class CacheRetentionConfig:
    """Supervisor-owned cache generation/entry retention configuration.

    Persisted as the `cache_retention` JSON object on
    `runtime_retention_config`, exposed through the retention API, and reloaded
    at the start of every supervisor cycle. Cache cleanup runs as a distinct
    step inside the existing retention cycle and reuses its advisory lock and
    cadence rather than electing a second leader.

        Attributes:
            alert_cooldown_seconds (int | Unset): Suppress duplicate cache alerts sharing a correlation id for this long.
            alert_limit_per_cycle (int | Unset): Maximum cache alerts emitted per supervisor cycle.
            batch_size (int | Unset): Maximum `cache_entry` rows deleted per bounded batch call.
            dry_run (bool | Unset): Report cleanup candidates and metrics without deleting rows.
            enabled (bool | Unset): Enable cache generation/entry cleanup as part of the retention cycle.
            freshness_alert_grace_seconds (int | Unset): Extra grace beyond a namespace's own `freshness_target_seconds`
                before
                a stale active generation is treated as alert-worthy.
            freshness_alerts_enabled (bool | Unset): Emit a `core.alert` when a namespace's active generation exceeds its
                freshness target, or a namespace repeatedly fails to publish a
                staging generation.
            max_batches_per_generation (int | Unset): Maximum entry-deletion batches performed for a single cleanup-
                candidate
                generation within one supervisor cycle. Bounds how long a single
                high-cardinality generation can dominate a cycle; entries are always
                deleted in indexed bounded batches before the generation row itself.
            max_generations_per_cycle (int | Unset): Maximum cleanup-candidate generations (failed, or retired past
                `readable_until`) processed in a single supervisor cycle.
            max_namespaces_per_cycle (int | Unset): Maximum namespaces inspected for staging expiry/freshness per cycle,
                and maximum tombstoned-and-emptied namespaces deleted per cycle.
            min_traversal_window_seconds (int | Unset): Minimum time a retired generation remains readable after retirement.
                Enforced defensively by the supervisor in addition to the generation's
                own stored `readable_until`, so cleanup never races a traversal that
                began while the generation was still active.
            staging_expiry_seconds (int | Unset): Unpublished staging or ready generations older than this many seconds
                are treated as abandoned; the supervisor marks them `failed` so the
                normal cleanup path reclaims them.
            staging_failure_alert_threshold (int | Unset): Consecutive staging failures observed for the same namespace
                within
                the freshness lookback before a repeated-failure alert is emitted.
    """

    alert_cooldown_seconds: int | Unset = UNSET
    alert_limit_per_cycle: int | Unset = UNSET
    batch_size: int | Unset = UNSET
    dry_run: bool | Unset = UNSET
    enabled: bool | Unset = UNSET
    freshness_alert_grace_seconds: int | Unset = UNSET
    freshness_alerts_enabled: bool | Unset = UNSET
    max_batches_per_generation: int | Unset = UNSET
    max_generations_per_cycle: int | Unset = UNSET
    max_namespaces_per_cycle: int | Unset = UNSET
    min_traversal_window_seconds: int | Unset = UNSET
    staging_expiry_seconds: int | Unset = UNSET
    staging_failure_alert_threshold: int | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        alert_cooldown_seconds = self.alert_cooldown_seconds

        alert_limit_per_cycle = self.alert_limit_per_cycle

        batch_size = self.batch_size

        dry_run = self.dry_run

        enabled = self.enabled

        freshness_alert_grace_seconds = self.freshness_alert_grace_seconds

        freshness_alerts_enabled = self.freshness_alerts_enabled

        max_batches_per_generation = self.max_batches_per_generation

        max_generations_per_cycle = self.max_generations_per_cycle

        max_namespaces_per_cycle = self.max_namespaces_per_cycle

        min_traversal_window_seconds = self.min_traversal_window_seconds

        staging_expiry_seconds = self.staging_expiry_seconds

        staging_failure_alert_threshold = self.staging_failure_alert_threshold

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if alert_cooldown_seconds is not UNSET:
            field_dict["alert_cooldown_seconds"] = alert_cooldown_seconds
        if alert_limit_per_cycle is not UNSET:
            field_dict["alert_limit_per_cycle"] = alert_limit_per_cycle
        if batch_size is not UNSET:
            field_dict["batch_size"] = batch_size
        if dry_run is not UNSET:
            field_dict["dry_run"] = dry_run
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if freshness_alert_grace_seconds is not UNSET:
            field_dict["freshness_alert_grace_seconds"] = freshness_alert_grace_seconds
        if freshness_alerts_enabled is not UNSET:
            field_dict["freshness_alerts_enabled"] = freshness_alerts_enabled
        if max_batches_per_generation is not UNSET:
            field_dict["max_batches_per_generation"] = max_batches_per_generation
        if max_generations_per_cycle is not UNSET:
            field_dict["max_generations_per_cycle"] = max_generations_per_cycle
        if max_namespaces_per_cycle is not UNSET:
            field_dict["max_namespaces_per_cycle"] = max_namespaces_per_cycle
        if min_traversal_window_seconds is not UNSET:
            field_dict["min_traversal_window_seconds"] = min_traversal_window_seconds
        if staging_expiry_seconds is not UNSET:
            field_dict["staging_expiry_seconds"] = staging_expiry_seconds
        if staging_failure_alert_threshold is not UNSET:
            field_dict["staging_failure_alert_threshold"] = (
                staging_failure_alert_threshold
            )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        alert_cooldown_seconds = d.pop("alert_cooldown_seconds", UNSET)

        alert_limit_per_cycle = d.pop("alert_limit_per_cycle", UNSET)

        batch_size = d.pop("batch_size", UNSET)

        dry_run = d.pop("dry_run", UNSET)

        enabled = d.pop("enabled", UNSET)

        freshness_alert_grace_seconds = d.pop("freshness_alert_grace_seconds", UNSET)

        freshness_alerts_enabled = d.pop("freshness_alerts_enabled", UNSET)

        max_batches_per_generation = d.pop("max_batches_per_generation", UNSET)

        max_generations_per_cycle = d.pop("max_generations_per_cycle", UNSET)

        max_namespaces_per_cycle = d.pop("max_namespaces_per_cycle", UNSET)

        min_traversal_window_seconds = d.pop("min_traversal_window_seconds", UNSET)

        staging_expiry_seconds = d.pop("staging_expiry_seconds", UNSET)

        staging_failure_alert_threshold = d.pop(
            "staging_failure_alert_threshold", UNSET
        )

        cache_retention_config = cls(
            alert_cooldown_seconds=alert_cooldown_seconds,
            alert_limit_per_cycle=alert_limit_per_cycle,
            batch_size=batch_size,
            dry_run=dry_run,
            enabled=enabled,
            freshness_alert_grace_seconds=freshness_alert_grace_seconds,
            freshness_alerts_enabled=freshness_alerts_enabled,
            max_batches_per_generation=max_batches_per_generation,
            max_generations_per_cycle=max_generations_per_cycle,
            max_namespaces_per_cycle=max_namespaces_per_cycle,
            min_traversal_window_seconds=min_traversal_window_seconds,
            staging_expiry_seconds=staging_expiry_seconds,
            staging_failure_alert_threshold=staging_failure_alert_threshold,
        )

        cache_retention_config.additional_properties = d
        return cache_retention_config

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
