from enum import StrEnum


class DashboardSourceStatus(StrEnum):
    EMPTY = "empty"
    ERROR = "error"
    FORBIDDEN = "forbidden"
    INVALID = "invalid"
    OK = "ok"
    PARTIAL = "partial"
    STALE = "stale"

    def __str__(self) -> str:
        return str(self.value)
