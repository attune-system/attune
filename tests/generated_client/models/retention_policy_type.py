from enum import Enum


class RetentionPolicyType(str, Enum):
    DAYS = "days"
    HOURS = "hours"
    MINUTES = "minutes"
    VERSIONS = "versions"

    def __str__(self) -> str:
        return str(self.value)
