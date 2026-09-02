from enum import StrEnum


class ChecksumSubject(StrEnum):
    ARCHIVE_BYTES = "archive_bytes"
    DIRECTORY_CONTENT = "directory_content"

    def __str__(self) -> str:
        return str(self.value)
