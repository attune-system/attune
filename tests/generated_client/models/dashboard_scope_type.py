from enum import Enum

class DashboardScopeType(str, Enum):
    GLOBAL = "global"
    IDENTITY = "identity"
    PACK = "pack"
    TENANT = "tenant"

    def __str__(self) -> str:
        return str(self.value)
