from enum import StrEnum


class SourceAvailability(StrEnum):
    AVAILABLE_NOW = "available_now"
    PARTIAL = "partial"
    PLANNED = "planned"

    def __str__(self) -> str:
        return str(self.value)
