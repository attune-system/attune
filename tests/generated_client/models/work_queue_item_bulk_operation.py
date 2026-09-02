from enum import StrEnum


class WorkQueueItemBulkOperation(StrEnum):
    CANCEL = "cancel"
    PATCH_PAYLOAD = "patch_payload"
    REPRIORITIZE = "reprioritize"

    def __str__(self) -> str:
        return str(self.value)
