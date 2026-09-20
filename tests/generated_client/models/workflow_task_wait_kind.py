from enum import StrEnum


class WorkflowTaskWaitKind(StrEnum):
    EXECUTION = "execution"
    INQUIRY = "inquiry"
    WORK_QUEUE_ITEM = "work_queue_item"

    def __str__(self) -> str:
        return str(self.value)
