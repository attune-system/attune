from enum import StrEnum


class WorkerStatus(StrEnum):
    ACTIVE = "active"
    BUSY = "busy"
    ERROR = "error"
    INACTIVE = "inactive"

    def __str__(self) -> str:
        return str(self.value)
