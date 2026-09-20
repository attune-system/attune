from enum import StrEnum


class ArtifactBodyState(StrEnum):
    CLEANUP_CLAIMED = "cleanup_claimed"
    DELETING = "deleting"
    PENDING = "pending"
    READY = "ready"

    def __str__(self) -> str:
        return str(self.value)
