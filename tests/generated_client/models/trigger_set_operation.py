from enum import StrEnum


class TriggerSetOperation(StrEnum):
    SET = "set"

    def __str__(self) -> str:
        return str(self.value)
