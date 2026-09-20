from enum import StrEnum


class PlatformCatalogStatus(StrEnum):
    CURRENT = "current"
    INCOMPATIBLE = "incompatible"
    UPGRADE_REQUIRED = "upgrade_required"

    def __str__(self) -> str:
        return str(self.value)
