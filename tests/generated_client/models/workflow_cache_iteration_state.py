from enum import StrEnum


class WorkflowCacheIterationState(StrEnum):
    CANCELLED = "cancelled"
    COMPLETED = "completed"
    FAILED = "failed"
    SCANNING = "scanning"

    def __str__(self) -> str:
        return str(self.value)
