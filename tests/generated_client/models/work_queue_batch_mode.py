from enum import Enum


class WorkQueueBatchMode(str, Enum):
    BATCH = "batch"
    SINGLE = "single"

    def __str__(self) -> str:
        return str(self.value)
