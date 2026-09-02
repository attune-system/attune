from enum import StrEnum


class WorkerType(StrEnum):
    CONTAINER = "container"
    LOCAL = "local"
    REMOTE = "remote"

    def __str__(self) -> str:
        return str(self.value)
