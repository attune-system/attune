from enum import StrEnum


class ArtifactType(StrEnum):
    FILE_BINARY = "file_binary"
    FILE_DATATABLE = "file_datatable"
    FILE_IMAGE = "file_image"
    FILE_TEXT = "file_text"
    OTHER = "other"
    PROGRESS = "progress"
    URL = "url"

    def __str__(self) -> str:
        return str(self.value)
