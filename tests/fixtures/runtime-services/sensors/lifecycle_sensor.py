import json
import os
import sys
import threading
from urllib.request import Request, urlopen

assert "ATTUNE_SENSOR_CONFIG" not in os.environ
assert "ATTUNE__DATABASE__URL" not in os.environ
bootstrap = json.load(sys.stdin)
for rule in bootstrap["triggers"]:
    body = {"trigger_ref": rule["trigger_ref"], "trigger_instance_id": f"rule_{rule['id']}",
            "config": rule["config"], "payload": {"message": "sensor-contract"}}
    request = Request(os.environ["ATTUNE_API_URL"].rstrip("/") + "/api/v1/events",
                      data=json.dumps(body).encode(), headers={
                          "Content-Type": "application/json",
                          "Authorization": "Bearer " + os.environ["ATTUNE_API_TOKEN"],
                      }, method="POST")
    with urlopen(request, timeout=10) as response:
        assert response.status in (200, 201)
print("sensor-bootstrap-ok", flush=True)
threading.Event().wait()
