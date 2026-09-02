from enum import StrEnum


class ArtifactJsonPatchType1Op(StrEnum):
    CLEAR = "clear"

    def __str__(self) -> str:
        return str(self.value)
