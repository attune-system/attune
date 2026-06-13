from enum import Enum


class EnforcementStatus(str, Enum):
    CREATED = "created"
    DISABLED = "disabled"
    PROCESSED = "processed"

    def __str__(self) -> str:
        return str(self.value)
