"""Check secret delivery without printing the delivered value."""

import hashlib
import json
import os
import sys

parameters = json.load(sys.stdin)
credential = parameters["credential"]
valid = hashlib.sha256(credential.encode()).hexdigest() == parameters["expected_digest"]
in_environment = any(credential in value for value in os.environ.values())
print("SECRET_RECEIVED: yes" if credential else "SECRET_RECEIVED: no")
print("SECRET_VALID: yes" if valid else "SECRET_VALID: no")
print("SECURITY_VIOLATION: Secret found in environment" if in_environment else
      "SECURITY_CHECK: Secret not in environment variables (GOOD)")
assert valid and not in_environment
