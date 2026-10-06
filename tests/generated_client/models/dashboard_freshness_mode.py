from enum import StrEnum


class DashboardFreshnessMode(StrEnum):
    RAW_ONLY = "raw_only"
    RAW_ONLY_FALLBACK = "raw_only_fallback"

    def __str__(self) -> str:
        return str(self.value)
