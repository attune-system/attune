from enum import Enum

class CacheNamespaceFreshness(str, Enum):
    FRESH = "fresh"
    STALE = "stale"
    UNPOPULATED = "unpopulated"

    def __str__(self) -> str:
        return str(self.value)
