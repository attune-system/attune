from enum import StrEnum


class PolicyMethod(StrEnum):
    CANCEL = "cancel"
    ENQUEUE = "enqueue"

    def __str__(self) -> str:
        return str(self.value)
