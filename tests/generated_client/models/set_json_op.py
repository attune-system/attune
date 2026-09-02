from enum import StrEnum


class SetJsonOp(StrEnum):
    SET = "set"

    def __str__(self) -> str:
        return str(self.value)
