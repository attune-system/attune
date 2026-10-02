from enum import StrEnum


class DeviceWaitReason(StrEnum):
    AUTHORIZATION_PENDING = "authorization_pending"
    PROVIDER_TIMEOUT = "provider_timeout"
    SLOW_DOWN = "slow_down"

    def __str__(self) -> str:
        return str(self.value)
