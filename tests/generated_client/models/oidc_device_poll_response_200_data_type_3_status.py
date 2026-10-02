from enum import StrEnum


class OidcDevicePollResponse200DataType3Status(StrEnum):
    EXPIRED = "expired"

    def __str__(self) -> str:
        return str(self.value)
