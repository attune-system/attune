from enum import StrEnum


class LabelExpressionOperator(StrEnum):
    DOES_NOT_EXIST = "does_not_exist"
    EXISTS = "exists"
    IN = "in"
    NOT_IN = "not_in"

    def __str__(self) -> str:
        return str(self.value)
