from enum import StrEnum


class InquiryResponseOptionStyle(StrEnum):
    DEFAULT = "default"
    DESTRUCTIVE = "destructive"
    POSITIVE = "positive"

    def __str__(self) -> str:
        return str(self.value)
