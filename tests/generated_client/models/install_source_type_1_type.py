from enum import StrEnum


class InstallSourceType1Type(StrEnum):
    ARCHIVE = "archive"

    def __str__(self) -> str:
        return str(self.value)
