from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.retention_target_config import RetentionTargetConfig


T = TypeVar("T", bound="RetentionTargetsConfig")


@_attrs_define
class RetentionTargetsConfig:
    """Per-table runtime retention targets.

    Attributes:
        audit_events (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        continuous_aggregates (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        enforcements (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        events (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        execution_admission (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        execution_history (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        executions (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        inquiries (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        notifications (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        pack_test_executions (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        sensor_process_history (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        sensor_processes (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        webhook_event_logs (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        work_queue_dispatches (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        work_queue_items (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        worker_history (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
        workers (RetentionTargetConfig | Unset): Runtime database row retention settings.

            A target with `max_age_seconds: None` keeps rows forever (purging disabled).
            A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
    """

    audit_events: RetentionTargetConfig | Unset = UNSET
    continuous_aggregates: RetentionTargetConfig | Unset = UNSET
    enforcements: RetentionTargetConfig | Unset = UNSET
    events: RetentionTargetConfig | Unset = UNSET
    execution_admission: RetentionTargetConfig | Unset = UNSET
    execution_history: RetentionTargetConfig | Unset = UNSET
    executions: RetentionTargetConfig | Unset = UNSET
    inquiries: RetentionTargetConfig | Unset = UNSET
    notifications: RetentionTargetConfig | Unset = UNSET
    pack_test_executions: RetentionTargetConfig | Unset = UNSET
    sensor_process_history: RetentionTargetConfig | Unset = UNSET
    sensor_processes: RetentionTargetConfig | Unset = UNSET
    webhook_event_logs: RetentionTargetConfig | Unset = UNSET
    work_queue_dispatches: RetentionTargetConfig | Unset = UNSET
    work_queue_items: RetentionTargetConfig | Unset = UNSET
    worker_history: RetentionTargetConfig | Unset = UNSET
    workers: RetentionTargetConfig | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        audit_events: dict[str, Any] | Unset = UNSET
        if not isinstance(self.audit_events, Unset):
            audit_events = self.audit_events.to_dict()

        continuous_aggregates: dict[str, Any] | Unset = UNSET
        if not isinstance(self.continuous_aggregates, Unset):
            continuous_aggregates = self.continuous_aggregates.to_dict()

        enforcements: dict[str, Any] | Unset = UNSET
        if not isinstance(self.enforcements, Unset):
            enforcements = self.enforcements.to_dict()

        events: dict[str, Any] | Unset = UNSET
        if not isinstance(self.events, Unset):
            events = self.events.to_dict()

        execution_admission: dict[str, Any] | Unset = UNSET
        if not isinstance(self.execution_admission, Unset):
            execution_admission = self.execution_admission.to_dict()

        execution_history: dict[str, Any] | Unset = UNSET
        if not isinstance(self.execution_history, Unset):
            execution_history = self.execution_history.to_dict()

        executions: dict[str, Any] | Unset = UNSET
        if not isinstance(self.executions, Unset):
            executions = self.executions.to_dict()

        inquiries: dict[str, Any] | Unset = UNSET
        if not isinstance(self.inquiries, Unset):
            inquiries = self.inquiries.to_dict()

        notifications: dict[str, Any] | Unset = UNSET
        if not isinstance(self.notifications, Unset):
            notifications = self.notifications.to_dict()

        pack_test_executions: dict[str, Any] | Unset = UNSET
        if not isinstance(self.pack_test_executions, Unset):
            pack_test_executions = self.pack_test_executions.to_dict()

        sensor_process_history: dict[str, Any] | Unset = UNSET
        if not isinstance(self.sensor_process_history, Unset):
            sensor_process_history = self.sensor_process_history.to_dict()

        sensor_processes: dict[str, Any] | Unset = UNSET
        if not isinstance(self.sensor_processes, Unset):
            sensor_processes = self.sensor_processes.to_dict()

        webhook_event_logs: dict[str, Any] | Unset = UNSET
        if not isinstance(self.webhook_event_logs, Unset):
            webhook_event_logs = self.webhook_event_logs.to_dict()

        work_queue_dispatches: dict[str, Any] | Unset = UNSET
        if not isinstance(self.work_queue_dispatches, Unset):
            work_queue_dispatches = self.work_queue_dispatches.to_dict()

        work_queue_items: dict[str, Any] | Unset = UNSET
        if not isinstance(self.work_queue_items, Unset):
            work_queue_items = self.work_queue_items.to_dict()

        worker_history: dict[str, Any] | Unset = UNSET
        if not isinstance(self.worker_history, Unset):
            worker_history = self.worker_history.to_dict()

        workers: dict[str, Any] | Unset = UNSET
        if not isinstance(self.workers, Unset):
            workers = self.workers.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if audit_events is not UNSET:
            field_dict["audit_events"] = audit_events
        if continuous_aggregates is not UNSET:
            field_dict["continuous_aggregates"] = continuous_aggregates
        if enforcements is not UNSET:
            field_dict["enforcements"] = enforcements
        if events is not UNSET:
            field_dict["events"] = events
        if execution_admission is not UNSET:
            field_dict["execution_admission"] = execution_admission
        if execution_history is not UNSET:
            field_dict["execution_history"] = execution_history
        if executions is not UNSET:
            field_dict["executions"] = executions
        if inquiries is not UNSET:
            field_dict["inquiries"] = inquiries
        if notifications is not UNSET:
            field_dict["notifications"] = notifications
        if pack_test_executions is not UNSET:
            field_dict["pack_test_executions"] = pack_test_executions
        if sensor_process_history is not UNSET:
            field_dict["sensor_process_history"] = sensor_process_history
        if sensor_processes is not UNSET:
            field_dict["sensor_processes"] = sensor_processes
        if webhook_event_logs is not UNSET:
            field_dict["webhook_event_logs"] = webhook_event_logs
        if work_queue_dispatches is not UNSET:
            field_dict["work_queue_dispatches"] = work_queue_dispatches
        if work_queue_items is not UNSET:
            field_dict["work_queue_items"] = work_queue_items
        if worker_history is not UNSET:
            field_dict["worker_history"] = worker_history
        if workers is not UNSET:
            field_dict["workers"] = workers

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.retention_target_config import RetentionTargetConfig

        d = dict(src_dict)
        _audit_events = d.pop("audit_events", UNSET)
        audit_events: RetentionTargetConfig | Unset
        if isinstance(_audit_events, Unset):
            audit_events = UNSET
        else:
            audit_events = RetentionTargetConfig.from_dict(_audit_events)

        _continuous_aggregates = d.pop("continuous_aggregates", UNSET)
        continuous_aggregates: RetentionTargetConfig | Unset
        if isinstance(_continuous_aggregates, Unset):
            continuous_aggregates = UNSET
        else:
            continuous_aggregates = RetentionTargetConfig.from_dict(
                _continuous_aggregates
            )

        _enforcements = d.pop("enforcements", UNSET)
        enforcements: RetentionTargetConfig | Unset
        if isinstance(_enforcements, Unset):
            enforcements = UNSET
        else:
            enforcements = RetentionTargetConfig.from_dict(_enforcements)

        _events = d.pop("events", UNSET)
        events: RetentionTargetConfig | Unset
        if isinstance(_events, Unset):
            events = UNSET
        else:
            events = RetentionTargetConfig.from_dict(_events)

        _execution_admission = d.pop("execution_admission", UNSET)
        execution_admission: RetentionTargetConfig | Unset
        if isinstance(_execution_admission, Unset):
            execution_admission = UNSET
        else:
            execution_admission = RetentionTargetConfig.from_dict(_execution_admission)

        _execution_history = d.pop("execution_history", UNSET)
        execution_history: RetentionTargetConfig | Unset
        if isinstance(_execution_history, Unset):
            execution_history = UNSET
        else:
            execution_history = RetentionTargetConfig.from_dict(_execution_history)

        _executions = d.pop("executions", UNSET)
        executions: RetentionTargetConfig | Unset
        if isinstance(_executions, Unset):
            executions = UNSET
        else:
            executions = RetentionTargetConfig.from_dict(_executions)

        _inquiries = d.pop("inquiries", UNSET)
        inquiries: RetentionTargetConfig | Unset
        if isinstance(_inquiries, Unset):
            inquiries = UNSET
        else:
            inquiries = RetentionTargetConfig.from_dict(_inquiries)

        _notifications = d.pop("notifications", UNSET)
        notifications: RetentionTargetConfig | Unset
        if isinstance(_notifications, Unset):
            notifications = UNSET
        else:
            notifications = RetentionTargetConfig.from_dict(_notifications)

        _pack_test_executions = d.pop("pack_test_executions", UNSET)
        pack_test_executions: RetentionTargetConfig | Unset
        if isinstance(_pack_test_executions, Unset):
            pack_test_executions = UNSET
        else:
            pack_test_executions = RetentionTargetConfig.from_dict(
                _pack_test_executions
            )

        _sensor_process_history = d.pop("sensor_process_history", UNSET)
        sensor_process_history: RetentionTargetConfig | Unset
        if isinstance(_sensor_process_history, Unset):
            sensor_process_history = UNSET
        else:
            sensor_process_history = RetentionTargetConfig.from_dict(
                _sensor_process_history
            )

        _sensor_processes = d.pop("sensor_processes", UNSET)
        sensor_processes: RetentionTargetConfig | Unset
        if isinstance(_sensor_processes, Unset):
            sensor_processes = UNSET
        else:
            sensor_processes = RetentionTargetConfig.from_dict(_sensor_processes)

        _webhook_event_logs = d.pop("webhook_event_logs", UNSET)
        webhook_event_logs: RetentionTargetConfig | Unset
        if isinstance(_webhook_event_logs, Unset):
            webhook_event_logs = UNSET
        else:
            webhook_event_logs = RetentionTargetConfig.from_dict(_webhook_event_logs)

        _work_queue_dispatches = d.pop("work_queue_dispatches", UNSET)
        work_queue_dispatches: RetentionTargetConfig | Unset
        if isinstance(_work_queue_dispatches, Unset):
            work_queue_dispatches = UNSET
        else:
            work_queue_dispatches = RetentionTargetConfig.from_dict(
                _work_queue_dispatches
            )

        _work_queue_items = d.pop("work_queue_items", UNSET)
        work_queue_items: RetentionTargetConfig | Unset
        if isinstance(_work_queue_items, Unset):
            work_queue_items = UNSET
        else:
            work_queue_items = RetentionTargetConfig.from_dict(_work_queue_items)

        _worker_history = d.pop("worker_history", UNSET)
        worker_history: RetentionTargetConfig | Unset
        if isinstance(_worker_history, Unset):
            worker_history = UNSET
        else:
            worker_history = RetentionTargetConfig.from_dict(_worker_history)

        _workers = d.pop("workers", UNSET)
        workers: RetentionTargetConfig | Unset
        if isinstance(_workers, Unset):
            workers = UNSET
        else:
            workers = RetentionTargetConfig.from_dict(_workers)

        retention_targets_config = cls(
            audit_events=audit_events,
            continuous_aggregates=continuous_aggregates,
            enforcements=enforcements,
            events=events,
            execution_admission=execution_admission,
            execution_history=execution_history,
            executions=executions,
            inquiries=inquiries,
            notifications=notifications,
            pack_test_executions=pack_test_executions,
            sensor_process_history=sensor_process_history,
            sensor_processes=sensor_processes,
            webhook_event_logs=webhook_event_logs,
            work_queue_dispatches=work_queue_dispatches,
            work_queue_items=work_queue_items,
            worker_history=worker_history,
            workers=workers,
        )

        retention_targets_config.additional_properties = d
        return retention_targets_config

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
