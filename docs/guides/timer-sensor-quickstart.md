# Timer Sensor Quick Start Guide

**Last Updated:** 2025-01-27  
**Audience:** Developers

## Overview

This guide will help you get the timer sensor up and running for development and testing.

## Prerequisites

- Rust 1.70+ installed
- PostgreSQL 14+ running
- Notifier service running (websocket endpoint)
- Attune API service running

## Step 1: Start Dependencies

### Using Docker Compose

```bash
# From project root
docker-compose up -d postgres notifier
```

### Manual Setup

```bash
# PostgreSQL (already running on localhost:5432)
# Notifier (already running on localhost:8081)
```

Verify services are running:

```bash
# PostgreSQL
psql -h localhost -U postgres -c "SELECT version();"

# Notifier
curl http://localhost:8081/health
```

## Step 2: Start the API Service

```bash
# Terminal 1
cd attune
make run-api

# Or manually:
cd crates/api
cargo run
```

Verify API is running:

```bash
curl http://localhost:8080/health
```

## Step 3: Create a Service Account for the Sensor

**NOTE:** Service accounts are not yet implemented. This step will be available after implementing the service account system.

For now, you'll need to use a user token or skip authentication during development.

### When Service Accounts Are Implemented

```bash
# Get admin token (from login or existing session)
export ADMIN_TOKEN="your_admin_token_here"

# Create sensor service account
curl -X POST http://localhost:8080/service-accounts \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "sensor:core.timer",
    "scope": "sensor",
    "description": "Timer sensor for development",
    "ttl_days": 90,
    "metadata": {
      "trigger_types": ["core.timer"]
    }
  }'

# Save the returned token
export SENSOR_TOKEN="eyJhbGci..."
```

## Step 4: Start the Timer Sensor

```bash
# Terminal 2
cd attune

# Set environment variables
export ATTUNE_API_URL="http://localhost:8080"
export ATTUNE_API_TOKEN="your_sensor_token_here"  # Or user token for now
export ATTUNE_SENSOR_REF="core.timer"
export ATTUNE_NOTIFIER_WS_URL="ws://localhost:8081/ws"
export ATTUNE_LOG_LEVEL="debug"

# Run the sensor
cargo run --package core-timer-sensor
```

You should see output like:

```json
{"timestamp":"2025-01-27T12:34:56Z","level":"info","message":"Starting Attune Timer Sensor"}
{"timestamp":"2025-01-27T12:34:56Z","level":"info","message":"Configuration loaded successfully","sensor_ref":"core.timer","api_url":"http://localhost:8080"}
{"timestamp":"2025-01-27T12:34:56Z","level":"info","message":"API connectivity verified"}
{"timestamp":"2025-01-27T12:34:56Z","level":"info","message":"Timer manager initialized"}
{"timestamp":"2025-01-27T12:34:56Z","level":"info","message":"Connected to notifier websocket"}
{"timestamp":"2025-01-27T12:34:56Z","level":"info","message":"Subscribed to trigger_ref:core.timer"}
```

## Step 5: Create a Timer-Based Rule

### Via API

```bash
# Create a simple timer rule that fires every 5 seconds
curl -X POST http://localhost:8080/rules \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "timer_every_5s",
    "label": "Timer Every 5 Seconds",
    "description": "Test timer that fires every 5 seconds",
    "pack": "core",
    "trigger_type": "core.timer",
    "trigger_params": {
      "type": "interval",
      "interval": 5,
      "unit": "seconds"
    },
    "action_ref": "core.echo",
    "action_params": {
      "message": "Timer fired!"
    },
    "enabled": true
  }'
```

### Via CLI

```bash
# Not yet implemented
# attune rule create timer_every_5s --trigger core.timer --action core.echo
```

## Step 6: Watch the Sensor Logs

In the sensor terminal, you should see:

```json
{"timestamp":"2025-01-27T12:34:56Z","level":"info","message":"Handling RuleCreated","rule_id":123,"ref":"timer_every_5s"}
{"timestamp":"2025-01-27T12:34:56Z","level":"info","message":"Starting timer for rule 123"}
{"timestamp":"2025-01-27T12:34:56Z","level":"info","message":"Timer started for rule 123"}
{"timestamp":"2025-01-27T12:34:56Z","level":"info","message":"Interval timer loop started for rule 123","interval":5}
```

After 5 seconds:

```json
{"timestamp":"2025-01-27T12:35:01Z","level":"info","message":"Timer fired for rule 123, created event 456"}
```

## Step 7: Verify Events Are Created

```bash
# List events
curl http://localhost:8080/events \
  -H "Authorization: Bearer ${TOKEN}"

# Should show events with trigger_type "core.timer"
```

## Step 8: Test Rule Disable/Enable

### Disable the rule

```bash
curl -X POST http://localhost:8080/rules/timer_every_5s/disable \
  -H "Authorization: Bearer ${TOKEN}"
```

Sensor logs should show:

```json
{"timestamp":"2025-01-27T12:35:10Z","level":"info","message":"Handling RuleDisabled","rule_id":123}
{"timestamp":"2025-01-27T12:35:10Z","level":"info","message":"Stopped timer for rule 123"}
```

### Re-enable the rule

```bash
curl -X POST http://localhost:8080/rules/timer_every_5s/enable \
  -H "Authorization: Bearer ${TOKEN}"
```

Sensor logs should show:

```json
{"timestamp":"2025-01-27T12:35:20Z","level":"info","message":"Handling RuleEnabled","rule_id":123}
{"timestamp":"2025-01-27T12:35:20Z","level":"info","message":"Starting timer for rule 123"}
```

## Step 9: Test Different Timer Types

### Every 1 minute

```json
{
  "trigger_params": {
    "type": "interval",
    "interval": 1,
    "unit": "minutes"
  }
}
```

### Every 1 hour

```json
{
  "trigger_params": {
    "type": "interval",
    "interval": 1,
    "unit": "hours"
  }
}
```

### One-time at specific datetime

```json
{
  "trigger_params": {
    "type": "date_time",
    "fire_at": "2025-01-27T15:00:00Z"
  }
}
```

## Development Workflow

### Making Changes to the Sensor

```bash
# 1. Make code changes in crates/core-timer-sensor/src/

# 2. Build and check for errors
cargo build --package core-timer-sensor

# 3. Run tests
cargo test --package core-timer-sensor

# 4. Restart the sensor
# Stop with Ctrl+C, then:
cargo run --package core-timer-sensor
```

### Testing Edge Cases

1. **Sensor restart with active rules:**
   - Create a rule
   - Stop the sensor (Ctrl+C)
   - Start the sensor again
   - Verify it loads and starts the timer for existing rules

2. **Multiple rules with different intervals:**
   - Create 3 rules with 5s, 10s, and 15s intervals
   - Verify all timers fire independently

3. **Rule updates:**
   - Update a rule's trigger_params
   - Currently requires disable/enable cycle
   - Future: should handle updates automatically

4. **Network failures:**
   - Stop the API service
   - Observe sensor logs (should show retry attempts)
   - Restart API
   - Verify sensor reconnects

## Debugging

### Enable Debug Logging

```bash
export ATTUNE_LOG_LEVEL="debug"
cargo run --package core-timer-sensor
```

### Common Issues

**"Failed to connect to Attune API"**
- Verify API is running: `curl http://localhost:8080/health`
- Check `ATTUNE_API_URL` is correct

**"Failed to connect to notifier websocket"**
- Verify notifier is running: `curl http://localhost:8081/health`
- Check `ATTUNE_NOTIFIER_WS_URL` is correct
- Ensure the sensor token is valid and not expired

**"Insufficient permissions to create event"**
- Service account system not yet implemented
- Use a user token temporarily
- Or wait for service account implementation

**"Timer not firing"**
- Check sensor logs for "Timer started for rule X"
- Verify rule is enabled
- Check trigger_params format is correct
- Enable debug logging to see more details

**"No timers loaded on startup"**
- API endpoint `/rules?trigger_type=core.timer` not yet implemented
- Create a rule after sensor starts
- Timers are managed via notifier websocket lifecycle messages

## Next Steps

1. **Implement Service Account System** - See `docs/service-accounts.md`
2. **Add Cron Timer Support** - Implement cron parsing and scheduling
3. **Add Tests** - Integration tests for full sensor workflow
4. **Add Metrics** - Prometheus metrics for monitoring
5. **Production Deployment** - systemd service, Docker image, Kubernetes deployment

## Resources

- [Sensor Interface Specification](./sensor-interface.md)
- [Service Accounts Documentation](./service-accounts.md)
- [Timer Sensor README](../../crates/core-timer-sensor/README.md)
- [Sensor Authentication Overview](./sensor-authentication-overview.md)

## Troubleshooting Tips

### Check notifier health

```bash
curl http://localhost:8081/health
```

### Monitor notifier logs

```bash
docker compose logs -f notifier
```

### Monitor API Logs

```bash
# In API terminal, should see:
# "Published rule lifecycle notifier event for rule timer_every_5s"
```

### Test Token Manually

```bash
# Decode JWT to inspect claims
echo "eyJhbGci..." | jq -R 'split(".") | .[1] | @base64d | fromjson'

# Should show:
{
  "sub": "sensor:core.timer",
  "scope": "sensor",
  "metadata": {
    "trigger_types": ["core.timer"]
  }
}
```

## Happy Hacking! 🚀