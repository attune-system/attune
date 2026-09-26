from enum import StrEnum


class DashboardScopeType(StrEnum):
    GLOBAL = "global"
    IDENTITY = "identity"
    PACK = "pack"

    def __str__(self) -> str:
        return str(self.value)
