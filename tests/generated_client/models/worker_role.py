from enum import Enum


class WorkerRole(str, Enum):
    ACTION = "action"
    SENSOR = "sensor"

    def __str__(self) -> str:
        return str(self.value)
