from enum import StrEnum


class DashboardScopeType(StrEnum):
    GLOBAL = "global"
    IDENTITY = "identity"
    PACK = "pack"
    TENANT = "tenant"

    def __str__(self) -> str:
        return str(self.value)
