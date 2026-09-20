from enum import StrEnum


class AbsentMetadataPolicy(StrEnum):
    DISABLE = "disable"
    REMOVE = "remove"
    RETAIN = "retain"

    def __str__(self) -> str:
        return str(self.value)
