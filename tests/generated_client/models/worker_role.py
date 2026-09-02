from enum import StrEnum


class WorkerRole(StrEnum):
    ACTION = "action"
    SENSOR = "sensor"

    def __str__(self) -> str:
        return str(self.value)
