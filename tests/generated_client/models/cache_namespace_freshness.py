from enum import StrEnum


class CacheNamespaceFreshness(StrEnum):
    FRESH = "fresh"
    STALE = "stale"
    UNPOPULATED = "unpopulated"

    def __str__(self) -> str:
        return str(self.value)
