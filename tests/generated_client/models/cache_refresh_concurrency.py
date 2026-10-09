from enum import StrEnum


class CacheRefreshConcurrency(StrEnum):
    CONFLICT = "conflict"
    PARALLEL = "parallel"
    REUSE = "reuse"

    def __str__(self) -> str:
        return str(self.value)
