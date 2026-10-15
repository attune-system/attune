# Quick Reference: Sensor vs Action Environment Variables

**Last Updated:** 2026-02-07  
**Status:** Current Implementation

## Overview

Both sensors and actions receive standard environment variables that provide execution context and API access. This document compares the environment variables provided to each to show the parity between the two execution models.

## Side-by-Side Comparison

| Purpose | Sensor Variable | Action Variable | Notes |
|---------|----------------|-----------------|-------|
| **Database ID** | `ATTUNE_SENSOR_ID` | `ATTUNE_EXEC_ID` | Unique identifier in database |
| **Reference Name** | `ATTUNE_SENSOR_REF` | `ATTUNE_ACTION` | Human-readable ref (e.g., `core.timer`, `core.http_request`) |
| **API Access Token** | `ATTUNE_API_TOKEN` | `ATTUNE_API_TOKEN` | ✅ Same variable name |
| **API Base URL** | `ATTUNE_API_URL` | `ATTUNE_API_URL` | ✅ Same variable name |
| **Triggering Rule** | N/A | `ATTUNE_RULE` | Only for actions triggered by rules |
| **Triggering Event** | N/A | `ATTUNE_TRIGGER` | Only for actions triggered by events |
| **Trigger instances** | stdin `triggers` section | N/A | Initial rules to monitor, potentially sensitive |
| **Message Queue URL** | `ATTUNE_MQ_URL` | N/A | Sensor-specific: for event publishing |
| **MQ Exchange** | `ATTUNE_MQ_EXCHANGE` | N/A | Sensor-specific: event destination |
| **Log Level** | `ATTUNE_LOG_LEVEL` | N/A | Sensor-specific: runtime logging config |

## Common Pattern: Identity and Context

Both sensors and actions follow the same pattern for identity and API access:

### Identity Variables
- **Database ID**: Unique numeric identifier
  - Sensors: `ATTUNE_SENSOR_ID`
  - Actions: `ATTUNE_EXEC_ID`
- **Reference Name**: Human-readable pack.name format
  - Sensors: `ATTUNE_SENSOR_REF`
  - Actions: `ATTUNE_ACTION`

### API Access Variables (Shared)
- `ATTUNE_API_URL` - Base URL for API calls
- `ATTUNE_API_TOKEN` - Authentication token

## Sensor-Specific Variables

Sensors receive additional variables for their unique responsibilities:

### Event Publishing
- `ATTUNE_MQ_URL` - RabbitMQ connection for publishing events
- `ATTUNE_MQ_EXCHANGE` - Exchange name for event routing

### Monitoring Configuration
- stdin JSON supplies the flat `config` map and initial `triggers` array
- `ATTUNE_LOG_LEVEL` - Runtime logging verbosity

### Example Sensor Environment
```bash
ATTUNE_SENSOR_ID=42
ATTUNE_SENSOR_REF=core.interval_timer_sensor
ATTUNE_API_URL=http://localhost:8080
ATTUNE_API_TOKEN=eyJ0eXAiOiJKV1QiLCJhbGc...
ATTUNE_MQ_URL=amqp://localhost:5672
ATTUNE_MQ_EXCHANGE=attune.events
ATTUNE_LOG_LEVEL=info
```

## Action-Specific Variables

Actions receive additional context about their triggering source:

### Execution Context
- `ATTUNE_RULE` - Rule that triggered this execution (if applicable)
- `ATTUNE_TRIGGER` - Trigger type that caused the event (if applicable)

### Example Action Environment (Rule-Triggered)
```bash
ATTUNE_EXEC_ID=12345
ATTUNE_ACTION=core.http_request
ATTUNE_API_URL=http://localhost:8080
ATTUNE_API_TOKEN=eyJ0eXAiOiJKV1QiLCJhbGc...
ATTUNE_RULE=monitoring.disk_space_alert
ATTUNE_TRIGGER=core.intervaltimer
```

### Example Action Environment (Manual Execution)
```bash
ATTUNE_EXEC_ID=12346
ATTUNE_ACTION=core.echo
ATTUNE_API_URL=http://localhost:8080
ATTUNE_API_TOKEN=eyJ0eXAiOiJKV1QiLCJhbGc...
# Note: ATTUNE_RULE and ATTUNE_TRIGGER not present for manual executions
```

## Implementation Status

### Fully Implemented ✅
- ✅ Sensor environment variables (all)
- ✅ Action identity variables (`ATTUNE_EXEC_ID`, `ATTUNE_ACTION`)
- ✅ Action API URL (`ATTUNE_API_URL`)
- ✅ Action rule/trigger context (`ATTUNE_RULE`, `ATTUNE_TRIGGER`)

### Partially Implemented ⚠️
- ⚠️ Action API token (`ATTUNE_API_TOKEN`) - Currently set to empty string
  - Variable is present but token generation not yet implemented
  - TODO: Implement execution-scoped JWT token generation
  - See: `work-summary/2026-02-07-env-var-standardization.md`

## Design Rationale

### Why Similar Patterns?

1. **Consistency**: Developers can apply the same mental model to both sensors and actions
2. **Tooling**: Shared libraries and utilities can work with both
3. **Documentation**: Single set of patterns to learn and document
4. **Testing**: Common test patterns for environment setup

### Why Different Variables?

1. **Separation of Concerns**: Sensors publish events; actions execute logic
2. **Message Queue Access**: Only sensors need direct MQ access for event publishing
3. **Execution Context**: Only actions need to know their triggering rule/event
4. **Configuration**: Sensors need runtime config (log level, trigger instances)

## Usage Examples

### Sensor using environment metadata and stdin values

```bash
#!/bin/bash
# Sensor script example

echo "Starting sensor: $ATTUNE_SENSOR_REF (ID: $ATTUNE_SENSOR_ID)" >&2

# Parse trigger instances
BOOTSTRAP=$(cat)
TRIGGERS=$(jq -c '.triggers' <<< "$BOOTSTRAP")

# Monitor for events and publish to MQ
# (Typically sensors use language-specific libraries, not bash)

# When event occurs, publish to Attune API
curl -X POST "$ATTUNE_API_URL/api/v1/events" \
  -H "Authorization: Bearer $ATTUNE_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "trigger_ref": "core.webhook",
    "payload": {...}
  }'
```

### Action Using Environment Variables

```bash
#!/bin/bash
# Action script example

echo "Executing action: $ATTUNE_ACTION (ID: $ATTUNE_EXEC_ID)" >&2

if [ -n "$ATTUNE_RULE" ]; then
  echo "Triggered by rule: $ATTUNE_RULE" >&2
  echo "Trigger type: $ATTUNE_TRIGGER" >&2
else
  echo "Manual execution (no rule)" >&2
fi

# Read parameters from stdin (NOT environment variables)
INPUT=$(cat)
MESSAGE=$(echo "$INPUT" | jq -r '.message')

# Perform action logic
echo "Processing: $MESSAGE"

# Optional: Call API for additional data
EXEC_INFO=$(curl -s "$ATTUNE_API_URL/api/v1/executions/$ATTUNE_EXEC_ID" \
  -H "Authorization: Bearer $ATTUNE_API_TOKEN")

# Output result to stdout (structured JSON or text)
echo '{"status": "success", "message": "'"$MESSAGE"'"}'
```

## Migration Notes

### Previous Variable Names (Deprecated)

The following variable names were used in earlier versions and should be migrated:

| Old Name | New Name | When to Migrate |
|----------|----------|----------------|
| `ATTUNE_EXECUTION_ID` | `ATTUNE_EXEC_ID` | Immediately |
| `ATTUNE_ACTION_REF` | `ATTUNE_ACTION` | Immediately |
| `ATTUNE_ACTION_ID` | *(removed)* | Not needed - use `ATTUNE_EXEC_ID` |

### Migration Script

If you have existing actions that reference old variable names:

```bash
# Replace in your action scripts
sed -i 's/ATTUNE_EXECUTION_ID/ATTUNE_EXEC_ID/g' *.sh
sed -i 's/ATTUNE_ACTION_REF/ATTUNE_ACTION/g' *.sh
```

## See Also

- [QUICKREF: Execution Environment Variables](./QUICKREF-execution-environment.md) - Full action environment reference
- [Sensor Interface Specification](./sensors/sensor-interface.md) - Complete sensor environment details
- [Worker Service Architecture](./architecture/worker-service.md) - How workers set environment variables
- [Sensor Service Architecture](./architecture/sensor-service.md) - How sensors are launched

## References

- Implementation: `crates/worker/src/executor.rs` (action env vars)
- Implementation: `crates/sensor/src/sensor_manager.rs` (sensor env vars)
- Migration Summary: `work-summary/2026-02-07-env-var-standardization.md`
