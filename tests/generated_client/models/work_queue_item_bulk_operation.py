from enum import Enum


class WorkQueueItemBulkOperation(str, Enum):
    CANCEL = "cancel"
    PATCH_PAYLOAD = "patch_payload"
    REPRIORITIZE = "reprioritize"

    def __str__(self) -> str:
        return str(self.value)
