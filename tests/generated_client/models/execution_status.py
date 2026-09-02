from enum import StrEnum


class ExecutionStatus(StrEnum):
    ABANDONED = "abandoned"
    CANCELING = "canceling"
    CANCELLED = "cancelled"
    COMPLETED = "completed"
    FAILED = "failed"
    REQUESTED = "requested"
    RUNNING = "running"
    SCHEDULED = "scheduled"
    SCHEDULING = "scheduling"
    TIMEOUT = "timeout"

    def __str__(self) -> str:
        return str(self.value)
