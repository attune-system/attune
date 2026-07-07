from enum import Enum

class DashboardAuthorizationMode(str, Enum):
    IDENTITY_FILTERED = "identity_filtered"
    OPERATOR_GLOBAL = "operator_global"

    def __str__(self) -> str:
        return str(self.value)
