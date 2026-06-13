from enum import Enum


class TaintEffect(str, Enum):
    NO_SCHEDULE = "no_schedule"
    PREFER_NO_SCHEDULE = "prefer_no_schedule"

    def __str__(self) -> str:
        return str(self.value)
