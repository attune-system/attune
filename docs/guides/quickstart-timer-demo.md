# Quick Start: Timer Echo Demo

This guide will help you run a simple demonstration of Attune's timer-based automation: an "echo Hello World" action that runs every 10 seconds.

## Prerequisites

- Stock PostgreSQL 16+ running, no TimescaleDB extension required
- RabbitMQ 3.12+ running
- Rust toolchain installed
- `jq` installed (for setup script)

## Architecture Overview

This demo exercises the complete Attune event flow:

```
Timer Manager → Event → Rule Match → Enforcement → Execution → Worker → Action
```

**Components involved:**
1. **Sensor Service** - Timer manager fires every 10 seconds
2. **API Service** - Provides REST endpoints for rule management
3. **Executor Service** - Processes enforcements and schedules executions
4. **Worker Service** - Executes the echo action

## Step 1: Database Setup

First, ensure your database is running and create the schema:

```bash
# Set database URL
export DATABASE_URL="postgresql://user:password@localhost:5432/attune"

# Run migrations
cd attune
sqlx database create
sqlx migrate run
```

## Step 2: Seed Core Pack Data

Load the core pack with timer triggers and basic actions:

```bash
psql $DATABASE_URL -f scripts/seed_core_pack.sql
```

This creates:
- **Core pack** with timer triggers and basic actions
- **Timer triggers**: `core.timer_10s`, `core.timer_1m`, `core.timer_hourly`
- **Actions**: `core.echo`, `core.sleep`, `core.noop`
- **Shell runtime** for executing shell commands

## Step 3: Configure Services

Create a configuration file or use environment variables:

```yaml
# config.development.yaml
environment: development

database:
  url: "postgresql://user:password@localhost:5432/attune"
  max_connections: 10

message_queue:
  url: "amqp://guest:guest@localhost:5672/%2F"

api:
  host: "0.0.0.0"
  port: 8080

jwt:
  secret: "your-secret-key-change-in-production"
  access_token_ttl: 3600
  refresh_token_ttl: 604800

worker:
  name: "worker-1"
  max_concurrent_tasks: 10
  task_timeout: 300
```

Or use environment variables:

```bash
export ATTUNE__DATABASE__URL="postgresql://user:password@localhost:5432/attune"
export ATTUNE__MESSAGE_QUEUE__URL="amqp://guest:guest@localhost:5672/%2F"
export ATTUNE__JWT__SECRET="your-secret-key"
```

## Step 4: Create Default User

Create an admin user for API access:

```sql
INSERT INTO attune.identity (username, email, password_hash, enabled)
VALUES (
    'admin',
    'admin@example.com',
    -- Password: 'admin' (hashed with Argon2id)
    '$argon2id$v=19$m=19456,t=2,p=1$...',
    true
);
```

Or use the API's registration endpoint after starting the API service.

## Step 5: Start Services

Open 4 terminal windows and start each service:

### Terminal 1: API Service
```bash
cd attune
cargo run --bin attune-api
```

Wait for: `Attune API Server listening on 0.0.0.0:8080`

### Terminal 2: Sensor Service
```bash
cd attune
cargo run --bin attune-sensor
```

Wait for: `Started X timer triggers`

### Terminal 3: Executor Service
```bash
cd attune
cargo run --bin attune-executor
```

Wait for: `Executor Service initialized successfully`

### Terminal 4: Worker Service
```bash
cd attune
cargo run --bin attune-worker
```

Wait for: `Attune Worker Service is ready`

## Step 6: Create the Timer Echo Rule

Run the setup script to create a rule that runs echo every 10 seconds:

```bash
cd attune
./scripts/setup_timer_echo_rule.sh
```

The script will:
1. Authenticate with the API
2. Verify core pack, trigger, and action exist
3. Create a rule: `core.timer_echo_10s`

## Step 7: Observe the System

### Watch Worker Logs

In the worker terminal, you should see output every 10 seconds:

```
[INFO] Received execution request: ...
[INFO] Executing action core.echo
[INFO] Action completed successfully
```

### Watch Sensor Logs

In the sensor terminal, you should see:

```
[DEBUG] Interval timer core.timer_10s fired
[INFO] Generated event 123 from timer trigger core.timer_10s
```

### Watch Executor Logs

In the executor terminal, you should see:

```
[INFO] Processing enforcement 456
[INFO] Scheduling execution for action core.echo
[INFO] Execution scheduled: 789
```

### Query via API

Check recent executions:

```bash
# Get auth token
TOKEN=$(curl -s -X POST http://localhost:8080/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"admin"}' | jq -r '.data.access_token')

# List recent executions
curl -H "Authorization: Bearer $TOKEN" \
  http://localhost:8080/api/v1/executions | jq '.data[0:5]'

# Get specific execution details
curl -H "Authorization: Bearer $TOKEN" \
  http://localhost:8080/api/v1/executions/123 | jq
```

## Step 8: Experiment

### Change the Timer Interval

Edit the trigger in the database to fire every 5 seconds:

```sql
UPDATE attune.trigger
SET param_schema = '{"type": "interval", "seconds": 5}'
WHERE ref = 'core.timer_10s';
```

Restart the sensor service to pick up the change.

### Change the Echo Message

Update the rule's action parameters:

```bash
curl -X PUT http://localhost:8080/api/v1/rules/core.timer_echo_10s \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "action_params": {
      "message": "Custom message from timer!"
    }
  }'
```

### Add Rule Conditions

Modify the rule to only fire during business hours (requires implementing rule conditions):

```json
{
  "conditions": {
    "condition": "all",
    "rules": [
      {
        "field": "fired_at",
        "operator": "greater_than",
        "value": "09:00:00"
      },
      {
        "field": "fired_at",
        "operator": "less_than",
        "value": "17:00:00"
      }
    ]
  }
}
```

### Create a Cron-Based Rule

Create a rule that fires on a cron schedule:

```bash
curl -X POST http://localhost:8080/api/v1/rules \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "core.hourly_echo",
    "pack": 1,
    "pack_ref": "core",
    "label": "Hourly Echo",
    "description": "Echoes a message every hour",
    "enabled": true,
    "trigger_ref": "core.timer_hourly",
    "action_ref": "core.echo",
    "action_params": {
      "message": "Hourly chime!"
    }
  }'
```

## Troubleshooting

### Timer Not Firing

1. **Check sensor service logs** for "Started X timer triggers"
2. **Verify trigger is enabled**: `SELECT * FROM attune.trigger WHERE ref = 'core.timer_10s';`
3. **Check timer configuration**: Ensure `param_schema` has valid timer config

### No Executions Created

1. **Check if rule exists and is enabled**: `SELECT * FROM attune.rule WHERE ref = 'core.timer_echo_10s';`
2. **Check sensor logs** for event generation
3. **Check executor logs** for enforcement processing

### Worker Not Executing

1. **Check worker service is running** and connected to message queue
2. **Check executor logs** for "Execution scheduled" messages
3. **Verify runtime exists**: `SELECT * FROM attune.runtime WHERE ref = 'shell';`
4. **Check worker has permission** to execute shell commands

### Database Connection Errors

1. Verify PostgreSQL is running
2. Check connection string is correct
3. Ensure `attune` database exists
4. Verify migrations ran successfully

### Message Queue Errors

1. Verify RabbitMQ is running
2. Check connection string is correct
3. Ensure exchanges and queues are created

## Next Steps

- **Add more actions**: Create Python or Node.js actions
- **Create workflows**: Chain multiple actions together
- **Add policies**: Implement concurrency limits or rate limiting
- **Human-in-the-loop**: Add inquiry actions that wait for user input
- **Custom sensors**: Write sensors that monitor external systems
- **Webhooks**: Implement webhook triggers for external events

## Clean Up

To stop the demo:

1. Press Ctrl+C in each service terminal
2. Disable the rule:
   ```bash
   curl -X PUT http://localhost:8080/api/v1/rules/core.timer_echo_10s \
     -H "Authorization: Bearer $TOKEN" \
     -H "Content-Type: application/json" \
     -d '{"enabled": false}'
   ```
3. (Optional) Clean up data:
   ```sql
   DELETE FROM attune.execution WHERE created < NOW() - INTERVAL '1 hour';
   DELETE FROM attune.event WHERE created < NOW() - INTERVAL '1 hour';
   DELETE FROM attune.enforcement WHERE created < NOW() - INTERVAL '1 hour';
   ```

## Learn More

- [Architecture Overview](architecture.md)
- [Data Model](data-model.md)
- [API Documentation](api-overview.md)
- [Creating Custom Actions](creating-actions.md)
- [Writing Sensors](writing-sensors.md)
