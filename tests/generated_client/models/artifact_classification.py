from enum import StrEnum


class ArtifactClassification(StrEnum):
    GENERAL = "general"
    RUNTIME_LOG = "runtime_log"

    def __str__(self) -> str:
        return str(self.value)
