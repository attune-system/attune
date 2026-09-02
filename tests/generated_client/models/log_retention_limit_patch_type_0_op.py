from enum import StrEnum


class LogRetentionLimitPatchType0Op(StrEnum):
    SET = "set"

    def __str__(self) -> str:
        return str(self.value)
