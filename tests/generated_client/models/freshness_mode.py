from enum import Enum

class FreshnessMode(str, Enum):
    AGGREGATE_ONLY = "aggregate_only"
    AGGREGATE_PLUS_TAIL = "aggregate_plus_tail"
    RAW_ONLY = "raw_only"
    RAW_ONLY_FALLBACK = "raw_only_fallback"

    def __str__(self) -> str:
        return str(self.value)
