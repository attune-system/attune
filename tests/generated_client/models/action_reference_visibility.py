from enum import StrEnum


class ActionReferenceVisibility(StrEnum):
    PRIVATE = "private"
    PUBLIC = "public"
    RESTRICTED = "restricted"

    def __str__(self) -> str:
        return str(self.value)
