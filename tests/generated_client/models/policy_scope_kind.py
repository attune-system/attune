from enum import Enum


class PolicyScopeKind(str, Enum):
    ACTION = "action"
    GLOBAL = "global"
    PACK = "pack"

    def __str__(self) -> str:
        return str(self.value)
