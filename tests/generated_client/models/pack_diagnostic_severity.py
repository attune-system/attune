from enum import StrEnum


class PackDiagnosticSeverity(StrEnum):
    ERROR = "error"
    WARNING = "warning"

    def __str__(self) -> str:
        return str(self.value)
