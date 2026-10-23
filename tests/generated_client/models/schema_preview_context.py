from enum import StrEnum


class SchemaPreviewContext(StrEnum):
    FLAT_DOCUMENT = "flat_document"
    SINGLE_VALUE = "single_value"

    def __str__(self) -> str:
        return str(self.value)
