from enum import Enum

class SourceType(str, Enum):
    ACTION_RESULT_PATH = "action_result_path"
    ENFORCEMENT_COUNT = "enforcement_count"
    ENFORCEMENT_TIMESERIES = "enforcement_timeseries"
    EVENT_COUNT = "event_count"
    EVENT_TIMESERIES = "event_timeseries"
    EXECUTION_COUNT = "execution_count"
    EXECUTION_DURATION_STATS = "execution_duration_stats"
    EXECUTION_STATUS_BREAKDOWN = "execution_status_breakdown"
    EXECUTION_TIMESERIES = "execution_timeseries"
    INQUIRY_BACKLOG = "inquiry_backlog"
    INQUIRY_SLA = "inquiry_sla"
    KEY_VALUE = "key_value"
    LAST_ENFORCEMENT = "last_enforcement"
    LAST_EVENT = "last_event"
    LAST_EXECUTION = "last_execution"
    LATEST_ACTION_RESULT = "latest_action_result"
    QUEUE_BACKLOG = "queue_backlog"
    QUEUE_DISPATCH_STATS = "queue_dispatch_stats"
    QUEUE_THROUGHPUT = "queue_throughput"
    SENSOR_HEALTH = "sensor_health"
    WORKER_HEALTH = "worker_health"
    WORKER_STATUS = "worker_status"

    def __str__(self) -> str:
        return str(self.value)
