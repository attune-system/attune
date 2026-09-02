from enum import StrEnum


class WorkQueueBatchMode(StrEnum):
    BATCH = "batch"
    SINGLE = "single"

    def __str__(self) -> str:
        return str(self.value)
