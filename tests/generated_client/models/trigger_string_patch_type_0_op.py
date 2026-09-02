from enum import StrEnum


class TriggerStringPatchType0Op(StrEnum):
    SET = "set"

    def __str__(self) -> str:
        return str(self.value)
