from enum import StrEnum


class ManagedTable(StrEnum):
    AUDIT_EVENT = "audit_event"
    EVENT = "event"
    EXECUTION_HISTORY = "execution_history"

    def __str__(self) -> str:
        return str(self.value)
