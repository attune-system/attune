from enum import Enum


class TimeoutSecondsPatchType0Op(str, Enum):
    SET = "set"

    def __str__(self) -> str:
        return str(self.value)
