from enum import Enum

class WorkQueueDispatchStatus(str, Enum):
    CANCELLED = "cancelled"
    COMPLETED = "completed"
    DISPATCHED = "dispatched"
    FAILED = "failed"
    LEASED = "leased"
    RELEASED = "released"

    def __str__(self) -> str:
        return str(self.value)
