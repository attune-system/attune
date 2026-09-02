from enum import StrEnum


class DashboardAuthorizationMode(StrEnum):
    IDENTITY_FILTERED = "identity_filtered"
    OPERATOR_GLOBAL = "operator_global"

    def __str__(self) -> str:
        return str(self.value)
