from enum import StrEnum


class InstallSourceType0Type(StrEnum):
    GIT = "git"

    def __str__(self) -> str:
        return str(self.value)
