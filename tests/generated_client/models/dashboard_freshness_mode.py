from enum import StrEnum


class DashboardFreshnessMode(StrEnum):
    CACHE_RAWFALLBACK = "cache_rawfallback"
    RAW_ONLY = "raw_only"
    SUMMARY_ONLY = "summary_only"
    SUMMARY_PLUS_RAW = "summary_plus_raw"

    def __str__(self) -> str:
        return str(self.value)
