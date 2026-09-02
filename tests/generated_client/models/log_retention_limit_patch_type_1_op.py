from enum import StrEnum


class LogRetentionLimitPatchType1Op(StrEnum):
    CLEAR = "clear"

    def __str__(self) -> str:
        return str(self.value)
