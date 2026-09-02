from enum import StrEnum


class DashboardVisibility(StrEnum):
    PACK = "pack"
    PRIVATE = "private"
    PUBLIC = "public"

    def __str__(self) -> str:
        return str(self.value)
