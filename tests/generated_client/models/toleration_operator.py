from enum import Enum


class TolerationOperator(str, Enum):
    EQUAL = "equal"
    EXISTS = "exists"

    def __str__(self) -> str:
        return str(self.value)
