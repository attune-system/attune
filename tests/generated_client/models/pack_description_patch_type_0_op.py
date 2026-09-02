from enum import StrEnum


class PackDescriptionPatchType0Op(StrEnum):
    SET = "set"

    def __str__(self) -> str:
        return str(self.value)
