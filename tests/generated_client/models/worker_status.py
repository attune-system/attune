from enum import Enum


class WorkerStatus(str, Enum):
    ACTIVE = "active"
    BUSY = "busy"
    ERROR = "error"
    INACTIVE = "inactive"

    def __str__(self) -> str:
        return str(self.value)
