from enum import StrEnum


class OwnerType(StrEnum):
    ACTION = "action"
    IDENTITY = "identity"
    PACK = "pack"
    SENSOR = "sensor"
    SYSTEM = "system"

    def __str__(self) -> str:
        return str(self.value)
