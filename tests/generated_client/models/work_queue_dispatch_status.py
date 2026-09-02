from enum import StrEnum


class WorkQueueDispatchStatus(StrEnum):
    CANCELLED = "cancelled"
    COMPLETED = "completed"
    DISPATCHED = "dispatched"
    FAILED = "failed"
    LEASED = "leased"
    RELEASED = "released"

    def __str__(self) -> str:
        return str(self.value)
