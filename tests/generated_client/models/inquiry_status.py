from enum import StrEnum


class InquiryStatus(StrEnum):
    CANCELLED = "cancelled"
    PENDING = "pending"
    RESPONDED = "responded"
    TIMEOUT = "timeout"

    def __str__(self) -> str:
        return str(self.value)
