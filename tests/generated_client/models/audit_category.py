from enum import Enum


class AuditCategory(str, Enum):
    ADMIN = "admin"
    API = "api"
    AUTH = "auth"
    EXECUTION = "execution"
    PACK = "pack"
    RBAC = "rbac"
    SECRET = "secret"

    def __str__(self) -> str:
        return str(self.value)
