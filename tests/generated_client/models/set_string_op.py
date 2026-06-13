from enum import Enum


class SetStringOp(str, Enum):
    SET = "set"

    def __str__(self) -> str:
        return str(self.value)
