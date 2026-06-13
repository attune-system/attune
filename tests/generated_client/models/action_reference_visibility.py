from enum import Enum


class ActionReferenceVisibility(str, Enum):
    PRIVATE = "private"
    PUBLIC = "public"
    RESTRICTED = "restricted"

    def __str__(self) -> str:
        return str(self.value)
