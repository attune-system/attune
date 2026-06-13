from enum import Enum


class WorkerHealthState(str, Enum):
    ACTIVE = "active"
    BUSY = "busy"
    CORDONED = "cordoned"
    ERROR = "error"
    INACTIVE = "inactive"
    OFFLINE = "offline"

    def __str__(self) -> str:
        return str(self.value)
