from enum import StrEnum


class TimeoutSecondsPatchType1Op(StrEnum):
    CLEAR = "clear"

    def __str__(self) -> str:
        return str(self.value)
