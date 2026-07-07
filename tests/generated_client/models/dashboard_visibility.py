from enum import Enum

class DashboardVisibility(str, Enum):
    PACK = "pack"
    PRIVATE = "private"
    PUBLIC = "public"

    def __str__(self) -> str:
        return str(self.value)
