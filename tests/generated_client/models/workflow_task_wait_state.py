from enum import StrEnum


class WorkflowTaskWaitState(StrEnum):
    CANCELLED = "cancelled"
    FAILED = "failed"
    RELEASED = "released"
    TIMED_OUT = "timed_out"
    WAITING = "waiting"

    def __str__(self) -> str:
        return str(self.value)
