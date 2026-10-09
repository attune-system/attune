from enum import StrEnum


class MaintenanceJob(StrEnum):
    PARTITION = "partition"
    RETENTION = "retention"
    SUMMARY = "summary"

    def __str__(self) -> str:
        return str(self.value)
