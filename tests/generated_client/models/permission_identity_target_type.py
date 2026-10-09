from enum import StrEnum


class PermissionIdentityTargetType(StrEnum):
    IDENTITY = "identity"

    def __str__(self) -> str:
        return str(self.value)
