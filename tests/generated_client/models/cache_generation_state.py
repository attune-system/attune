from enum import Enum

class CacheGenerationState(str, Enum):
    ACTIVE = "active"
    FAILED = "failed"
    READY = "ready"
    RETIRED = "retired"
    STAGING = "staging"

    def __str__(self) -> str:
        return str(self.value)
