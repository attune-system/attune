import json
import os
import sys

assert "ATTUNE_API_TOKEN" not in os.environ
assert "ATTUNE__DATABASE__URL" not in os.environ
print(json.dumps(json.load(sys.stdin)))
