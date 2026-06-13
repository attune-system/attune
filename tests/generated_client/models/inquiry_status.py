from enum import Enum


class InquiryStatus(str, Enum):
    CANCELLED = "cancelled"
    PENDING = "pending"
    RESPONDED = "responded"
    TIMEOUT = "timeout"

    def __str__(self) -> str:
        return str(self.value)
