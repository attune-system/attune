from enum import StrEnum


class CacheGenerationState(StrEnum):
    ACTIVE = "active"
    FAILED = "failed"
    READY = "ready"
    RETIRED = "retired"
    STAGING = "staging"

    def __str__(self) -> str:
        return str(self.value)
