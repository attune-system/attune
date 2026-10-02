from enum import StrEnum


class OidcDevicePollResponse200DataType1Status(StrEnum):
    AUTHORIZED = "authorized"

    def __str__(self) -> str:
        return str(self.value)
