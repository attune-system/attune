from enum import Enum

class AuthorizationBasis(str, Enum):
    DASHBOARDS = "dashboards"
    ENFORCEMENTS = "enforcements"
    EVENTS = "events"
    EXECUTIONS = "executions"
    INQUIRIES = "inquiries"
    KEYS = "keys"
    QUEUES = "queues"
    QUEUE_ITEMS = "queue_items"
    SENSORS = "sensors"
    WORKERS = "workers"

    def __str__(self) -> str:
        return str(self.value)
