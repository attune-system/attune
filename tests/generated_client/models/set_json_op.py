from enum import Enum


class SetJsonOp(str, Enum):
    SET = "set"

    def __str__(self) -> str:
        return str(self.value)
