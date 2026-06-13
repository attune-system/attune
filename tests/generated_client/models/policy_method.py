from enum import Enum


class PolicyMethod(str, Enum):
    CANCEL = "cancel"
    ENQUEUE = "enqueue"

    def __str__(self) -> str:
        return str(self.value)
