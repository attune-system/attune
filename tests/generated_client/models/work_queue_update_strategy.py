from enum import Enum


class WorkQueueUpdateStrategy(str, Enum):
    IMMUTABLE = "immutable"
    MERGE_PATCH = "merge_patch"
    REPLACE = "replace"

    def __str__(self) -> str:
        return str(self.value)
