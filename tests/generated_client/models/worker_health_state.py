from enum import StrEnum


class WorkerHealthState(StrEnum):
    ACTIVE = "active"
    BUSY = "busy"
    CORDONED = "cordoned"
    ERROR = "error"
    INACTIVE = "inactive"
    OFFLINE = "offline"

    def __str__(self) -> str:
        return str(self.value)
