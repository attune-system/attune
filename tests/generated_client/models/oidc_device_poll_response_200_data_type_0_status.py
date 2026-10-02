from enum import StrEnum


class OidcDevicePollResponse200DataType0Status(StrEnum):
    WAITING = "waiting"

    def __str__(self) -> str:
        return str(self.value)
