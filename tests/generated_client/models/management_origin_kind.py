from enum import StrEnum


class ManagementOriginKind(StrEnum):
    AD_HOC = "ad_hoc"
    PACK = "pack"
    PLATFORM = "platform"

    def __str__(self) -> str:
        return str(self.value)
