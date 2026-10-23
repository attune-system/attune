"""Verify the parameter-contract E2E fixtures receive complete flat stdin JSON."""

import json
import sys

parameters = json.load(sys.stdin)
expected_values = [
    {"port": 8080, "enabled": True},
    {"message": "hello", "extra": {"count": 0}},
    {"message": "Hello from defaults", "count": 3, "debug": True},
    {"message": "", "count": 0, "debug": False},
]
assert parameters in expected_values, "Unexpected parameter shape or values"
print(json.dumps(parameters))
