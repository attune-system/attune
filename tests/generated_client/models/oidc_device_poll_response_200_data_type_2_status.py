from enum import StrEnum


class OidcDevicePollResponse200DataType2Status(StrEnum):
    ACCESS_DENIED = "access_denied"

    def __str__(self) -> str:
        return str(self.value)
