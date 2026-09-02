from enum import StrEnum


class TestStatus(StrEnum):
    ERROR = "error"
    FAILED = "failed"
    PASSED = "passed"
    SKIPPED = "skipped"

    def __str__(self) -> str:
        return str(self.value)
