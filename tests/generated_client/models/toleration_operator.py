from enum import StrEnum


class TolerationOperator(StrEnum):
    EQUAL = "equal"
    EXISTS = "exists"

    def __str__(self) -> str:
        return str(self.value)
