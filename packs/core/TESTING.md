# Core Pack Testing Guide

Quick reference for testing core pack actions and sensors locally.

---

## Prerequisites

```bash
# Ensure scripts are executable
chmod +x packs/core/actions/*.sh
chmod +x packs/core/actions/*.py
chmod +x packs/core/sensors/*.py

# Install Python dependencies
pip install requests>=2.28.0
```

---

## Testing Actions

Actions receive parameters via environment variables prefixed with `ATTUNE_ACTION_`.

### Test `core.echo`

```bash
# Basic echo
export ATTUNE_ACTION_MESSAGE="Hello, Attune!"
./packs/core/actions/echo.sh

# With uppercase conversion
export ATTUNE_ACTION_MESSAGE="test message"
export ATTUNE_ACTION_UPPERCASE=true
./packs/core/actions/echo.sh
```

**Expected Output:**
```
Hello, Attune!
TEST MESSAGE
```

---

### Test `core.sleep`

```bash
# Sleep for 2 seconds
export ATTUNE_ACTION_SECONDS=2
export ATTUNE_ACTION_MESSAGE="Sleeping..."
time ./packs/core/actions/sleep.sh
```

**Expected Output:**
```
Sleeping...
Slept for 2 seconds

real    0m2.004s
```

---

### Test `core.noop`

```bash
# No operation with message
export ATTUNE_ACTION_MESSAGE="Testing noop"
./packs/core/actions/noop.sh

# With custom exit code
export ATTUNE_ACTION_EXIT_CODE=0
./packs/core/actions/noop.sh
echo "Exit code: $?"
```

**Expected Output:**
```
[NOOP] Testing noop
No operation completed successfully
Exit code: 0
```

---

### Test `core.http_request`

```bash
# Simple GET request
export ATTUNE_ACTION_URL="https://httpbin.org/get"
export ATTUNE_ACTION_METHOD="GET"
python3 ./packs/core/actions/http_request.py

# POST with JSON body
export ATTUNE_ACTION_URL="https://httpbin.org/post"
export ATTUNE_ACTION_METHOD="POST"
export ATTUNE_ACTION_JSON_BODY='{"name": "test", "value": 123}'
python3 ./packs/core/actions/http_request.py

# With custom headers
export ATTUNE_ACTION_URL="https://httpbin.org/headers"
export ATTUNE_ACTION_METHOD="GET"
export ATTUNE_ACTION_HEADERS='{"X-Custom-Header": "test-value"}'
python3 ./packs/core/actions/http_request.py

# With query parameters
export ATTUNE_ACTION_URL="https://httpbin.org/get"
export ATTUNE_ACTION_METHOD="GET"
export ATTUNE_ACTION_QUERY_PARAMS='{"foo": "bar", "page": "1"}'
python3 ./packs/core/actions/http_request.py

# With timeout
export ATTUNE_ACTION_URL="https://httpbin.org/delay/5"
export ATTUNE_ACTION_METHOD="GET"
export ATTUNE_ACTION_TIMEOUT=2
python3 ./packs/core/actions/http_request.py
```

**Expected Output:**
```json
{
  "status_code": 200,
  "headers": {
    "Content-Type": "application/json",
    ...
  },
  "body": "...",
  "json": {
    "args": {},
    "headers": {...},
    ...
  },
  "elapsed_ms": 234,
  "url": "https://httpbin.org/get",
  "success": true
}
```

---

## Testing Sensors

Connection metadata stays in the environment. Sensor settings and initial rule
configurations arrive in one stdin JSON object. The bundled timer is a native
process, not the former Python interval sensor.

### Test the timer bootstrap

```bash
# Requires API/notifier connection metadata and a token for a current admitted
# core.timer_sensor workload. RULE_ID must identify its enabled interval rule.
jq -n --argjson id "$RULE_ID" '{
  config: {},
  triggers: [{id: $id, ref: "core.local_timer", trigger_ref: "core.intervaltimer",
    config: {unit: "seconds", interval: 5}}]
}' | attune-core-timer-sensor
```

The process logs startup and sends timer events through the API every five
seconds. Stdout is a log stream, not an event transport. Check the Events page or
event API for `core.intervaltimer`. Build the binary and configure connection
metadata as described in `crates/core-timer-sensor/README.md`.

Press `Ctrl+C` to stop the sensor.

---

## Testing with Multiple Trigger Instances

```bash
# These must be enabled rule IDs belonging to the same admitted workload.
jq -n --argjson first "$FIRST_RULE_ID" --argjson second "$SECOND_RULE_ID" '{
  config: {},
  triggers: [
    {id: $first, trigger_ref: "core.intervaltimer", config: {unit: "seconds", interval: 3}},
    {id: $second, trigger_ref: "core.intervaltimer", config: {unit: "seconds", interval: 5}}
  ]
}' | attune-core-timer-sensor
```

Events should arrive at independent three-second and five-second intervals.

---

## Validation Tests

### Validate YAML Schemas

```bash
# Install yamllint (optional)
pip install yamllint

# Validate all YAML files
yamllint packs/core/**/*.yaml
```

### Validate JSON Schemas

```bash
# Check parameter schemas are valid JSON Schema
cat packs/core/actions/http_request.yaml | grep -A 50 "parameters:" | python3 -c "
import sys, yaml, json
data = yaml.safe_load(sys.stdin)
print(json.dumps(data, indent=2))
"
```

---

## Error Testing

### Test Invalid Parameters

```bash
# Invalid seconds value for sleep
export ATTUNE_ACTION_SECONDS=-1
./packs/core/actions/sleep.sh
# Expected: ERROR: seconds must be between 0 and 3600

# Invalid exit code for noop
export ATTUNE_ACTION_EXIT_CODE=999
./packs/core/actions/noop.sh
# Expected: ERROR: exit_code must be between 0 and 255

# Missing required parameter for HTTP request
unset ATTUNE_ACTION_URL
python3 ./packs/core/actions/http_request.py
# Expected: ERROR: Required parameter 'url' not provided
```

---

## Performance Testing

### Measure Action Execution Time

```bash
# Echo action
time for i in {1..100}; do
  export ATTUNE_ACTION_MESSAGE="Test $i"
  ./packs/core/actions/echo.sh > /dev/null
done

# HTTP request action
time for i in {1..10}; do
  export ATTUNE_ACTION_URL="https://httpbin.org/get"
  python3 ./packs/core/actions/http_request.py > /dev/null
done
```

---

## Integration Testing (with Attune Services)

### Prerequisites

```bash
# Start Attune services
docker-compose up -d postgres rabbitmq redis

# Run migrations
sqlx migrate run

# Load core pack (future)
# attune pack load packs/core
```

### Test Action Execution via API

```bash
# Create execution manually
curl -X POST http://localhost:8080/api/v1/executions \
  -H "Content-Type: application/json" \
  -d '{
    "action_ref": "core.echo",
    "parameters": {
      "message": "API test",
      "uppercase": true
    }
  }'

# Check execution status
curl http://localhost:8080/api/v1/executions/{execution_id}
```

### Test Sensor via Sensor Service

```bash
# Start sensor service (future)
# cargo run --bin attune-sensor

# Check events created
curl http://localhost:8080/api/v1/events?limit=10
```

---

## Troubleshooting

### Action Not Executing

```bash
# Check file permissions
ls -la packs/core/actions/

# Ensure scripts are executable
chmod +x packs/core/actions/*.sh
chmod +x packs/core/actions/*.py
```

### Python Import Errors

```bash
# Install required packages
pip install requests>=2.28.0

# Verify Python version
python3 --version  # Should be 3.8+
```

### Environment Variables Not Working

```bash
# Print all ATTUNE_* environment variables
env | grep ATTUNE_

# Test with explicit export
export ATTUNE_ACTION_MESSAGE="test"
echo $ATTUNE_ACTION_MESSAGE
```

---

## Automated Test Script

Create a test script `test_core_pack.sh`:

```bash
#!/bin/bash
set -e

echo "Testing Core Pack Actions..."

# Test echo
echo "→ Testing core.echo..."
export ATTUNE_ACTION_MESSAGE="Test"
./packs/core/actions/echo.sh > /dev/null
echo "✓ core.echo passed"

# Test sleep
echo "→ Testing core.sleep..."
export ATTUNE_ACTION_SECONDS=1
./packs/core/actions/sleep.sh > /dev/null
echo "✓ core.sleep passed"

# Test noop
echo "→ Testing core.noop..."
export ATTUNE_ACTION_MESSAGE="test"
./packs/core/actions/noop.sh > /dev/null
echo "✓ core.noop passed"

# Test HTTP request
echo "→ Testing core.http_request..."
export ATTUNE_ACTION_URL="https://httpbin.org/get"
export ATTUNE_ACTION_METHOD="GET"
python3 ./packs/core/actions/http_request.py > /dev/null
echo "✓ core.http_request passed"

echo ""
echo "All tests passed! ✓"
```

Run with:
```bash
chmod +x test_core_pack.sh
./test_core_pack.sh
```

---

## Next Steps

1. Implement pack loader to register components in database
2. Update worker service to execute actions from filesystem
3. Update sensor service to run sensors from filesystem
4. Add comprehensive integration tests
5. Create CLI commands for pack management

See `docs/core-pack-integration.md` for implementation details.
