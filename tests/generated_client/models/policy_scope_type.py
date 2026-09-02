from enum import StrEnum


class PolicyScopeType(StrEnum):
    ACTION = "action"
    GLOBAL = "global"
    PACK = "pack"

    def __str__(self) -> str:
        return str(self.value)
