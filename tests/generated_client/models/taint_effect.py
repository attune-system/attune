from enum import StrEnum


class TaintEffect(StrEnum):
    NO_SCHEDULE = "no_schedule"
    PREFER_NO_SCHEDULE = "prefer_no_schedule"

    def __str__(self) -> str:
        return str(self.value)
