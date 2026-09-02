from enum import StrEnum


class TriggerStringPatchType1Op(StrEnum):
    CLEAR = "clear"

    def __str__(self) -> str:
        return str(self.value)
