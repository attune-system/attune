from enum import StrEnum


class SetStringOp(StrEnum):
    SET = "set"

    def __str__(self) -> str:
        return str(self.value)
