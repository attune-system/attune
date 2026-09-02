from enum import StrEnum


class RetentionPolicyType(StrEnum):
    DAYS = "days"
    HOURS = "hours"
    MINUTES = "minutes"
    VERSIONS = "versions"

    def __str__(self) -> str:
        return str(self.value)
