from enum import StrEnum


class ArtifactVisibility(StrEnum):
    PRIVATE = "private"
    PUBLIC = "public"

    def __str__(self) -> str:
        return str(self.value)
