from enum import StrEnum


class PermissionRoleTargetType(StrEnum):
    ROLE = "role"

    def __str__(self) -> str:
        return str(self.value)
