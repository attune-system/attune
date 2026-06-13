from enum import Enum


class OwnerType(str, Enum):
    ACTION = "action"
    IDENTITY = "identity"
    PACK = "pack"
    SENSOR = "sensor"
    SYSTEM = "system"

    def __str__(self) -> str:
        return str(self.value)
