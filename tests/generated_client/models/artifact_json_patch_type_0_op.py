from enum import StrEnum


class ArtifactJsonPatchType0Op(StrEnum):
    SET = "set"

    def __str__(self) -> str:
        return str(self.value)
