from enum import Enum


class WorkQueueItemStatus(str, Enum):
    CANCELLED = "cancelled"
    COMPLETED = "completed"
    FAILED = "failed"
    LEASED = "leased"
    QUEUED = "queued"
    RETRY = "retry"
    SKIPPED = "skipped"

    def __str__(self) -> str:
        return str(self.value)
