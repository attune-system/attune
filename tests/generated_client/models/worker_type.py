from enum import Enum


class WorkerType(str, Enum):
    CONTAINER = "container"
    LOCAL = "local"
    REMOTE = "remote"

    def __str__(self) -> str:
        return str(self.value)
