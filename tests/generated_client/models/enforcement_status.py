from enum import StrEnum


class EnforcementStatus(StrEnum):
    CREATED = "created"
    DISABLED = "disabled"
    PROCESSED = "processed"

    def __str__(self) -> str:
        return str(self.value)
