from enum import Enum

class PolicyScopeType(str, Enum):
    ACTION = "action"
    GLOBAL = "global"
    PACK = "pack"

    def __str__(self) -> str:
        return str(self.value)
