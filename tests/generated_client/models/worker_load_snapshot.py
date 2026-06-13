from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="WorkerLoadSnapshot")


@_attrs_define
class WorkerLoadSnapshot:
    """
    Attributes:
        canceling (int):
        requested (int):
        running (int):  Example: 2.
        scheduled (int):  Example: 1.
        scheduling (int):
        total_active (int):  Example: 3.
        active_rules (int | None | Unset):  Example: 4.
        max_concurrent_executions (int | None | Unset):  Example: 10.
        max_concurrent_sensors (int | None | Unset):  Example: 10.
        queue_depth (int | None | Unset):  Example: 1.
        sensor_processes_monitored (int | None | Unset):  Example: 3.
        sensor_processes_running (int | None | Unset):  Example: 2.
        utilization_percent (int | None | Unset):  Example: 30.
    """

    canceling: int
    requested: int
    running: int
    scheduled: int
    scheduling: int
    total_active: int
    active_rules: int | None | Unset = UNSET
    max_concurrent_executions: int | None | Unset = UNSET
    max_concurrent_sensors: int | None | Unset = UNSET
    queue_depth: int | None | Unset = UNSET
    sensor_processes_monitored: int | None | Unset = UNSET
    sensor_processes_running: int | None | Unset = UNSET
    utilization_percent: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        canceling = self.canceling

        requested = self.requested

        running = self.running

        scheduled = self.scheduled

        scheduling = self.scheduling

        total_active = self.total_active

        active_rules: int | None | Unset
        if isinstance(self.active_rules, Unset):
            active_rules = UNSET
        else:
            active_rules = self.active_rules

        max_concurrent_executions: int | None | Unset
        if isinstance(self.max_concurrent_executions, Unset):
            max_concurrent_executions = UNSET
        else:
            max_concurrent_executions = self.max_concurrent_executions

        max_concurrent_sensors: int | None | Unset
        if isinstance(self.max_concurrent_sensors, Unset):
            max_concurrent_sensors = UNSET
        else:
            max_concurrent_sensors = self.max_concurrent_sensors

        queue_depth: int | None | Unset
        if isinstance(self.queue_depth, Unset):
            queue_depth = UNSET
        else:
            queue_depth = self.queue_depth

        sensor_processes_monitored: int | None | Unset
        if isinstance(self.sensor_processes_monitored, Unset):
            sensor_processes_monitored = UNSET
        else:
            sensor_processes_monitored = self.sensor_processes_monitored

        sensor_processes_running: int | None | Unset
        if isinstance(self.sensor_processes_running, Unset):
            sensor_processes_running = UNSET
        else:
            sensor_processes_running = self.sensor_processes_running

        utilization_percent: int | None | Unset
        if isinstance(self.utilization_percent, Unset):
            utilization_percent = UNSET
        else:
            utilization_percent = self.utilization_percent

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "canceling": canceling,
                "requested": requested,
                "running": running,
                "scheduled": scheduled,
                "scheduling": scheduling,
                "total_active": total_active,
            }
        )
        if active_rules is not UNSET:
            field_dict["active_rules"] = active_rules
        if max_concurrent_executions is not UNSET:
            field_dict["max_concurrent_executions"] = max_concurrent_executions
        if max_concurrent_sensors is not UNSET:
            field_dict["max_concurrent_sensors"] = max_concurrent_sensors
        if queue_depth is not UNSET:
            field_dict["queue_depth"] = queue_depth
        if sensor_processes_monitored is not UNSET:
            field_dict["sensor_processes_monitored"] = sensor_processes_monitored
        if sensor_processes_running is not UNSET:
            field_dict["sensor_processes_running"] = sensor_processes_running
        if utilization_percent is not UNSET:
            field_dict["utilization_percent"] = utilization_percent

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        canceling = d.pop("canceling")

        requested = d.pop("requested")

        running = d.pop("running")

        scheduled = d.pop("scheduled")

        scheduling = d.pop("scheduling")

        total_active = d.pop("total_active")

        def _parse_active_rules(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        active_rules = _parse_active_rules(d.pop("active_rules", UNSET))

        def _parse_max_concurrent_executions(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        max_concurrent_executions = _parse_max_concurrent_executions(
            d.pop("max_concurrent_executions", UNSET)
        )

        def _parse_max_concurrent_sensors(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        max_concurrent_sensors = _parse_max_concurrent_sensors(
            d.pop("max_concurrent_sensors", UNSET)
        )

        def _parse_queue_depth(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        queue_depth = _parse_queue_depth(d.pop("queue_depth", UNSET))

        def _parse_sensor_processes_monitored(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        sensor_processes_monitored = _parse_sensor_processes_monitored(
            d.pop("sensor_processes_monitored", UNSET)
        )

        def _parse_sensor_processes_running(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        sensor_processes_running = _parse_sensor_processes_running(
            d.pop("sensor_processes_running", UNSET)
        )

        def _parse_utilization_percent(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        utilization_percent = _parse_utilization_percent(
            d.pop("utilization_percent", UNSET)
        )

        worker_load_snapshot = cls(
            canceling=canceling,
            requested=requested,
            running=running,
            scheduled=scheduled,
            scheduling=scheduling,
            total_active=total_active,
            active_rules=active_rules,
            max_concurrent_executions=max_concurrent_executions,
            max_concurrent_sensors=max_concurrent_sensors,
            queue_depth=queue_depth,
            sensor_processes_monitored=sensor_processes_monitored,
            sensor_processes_running=sensor_processes_running,
            utilization_percent=utilization_percent,
        )

        worker_load_snapshot.additional_properties = d
        return worker_load_snapshot

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
