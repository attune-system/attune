# Quick Start: Running the Example Rule

This guide walks you through running the pre-seeded example that echoes "hello, world" every 10 seconds.

---

## Prerequisites

- Stock PostgreSQL 16+ running, no TimescaleDB extension required
- RabbitMQ 3.12+ running
- Rust toolchain installed
- Database migrations applied

---

## Step 1: Seed the Database

```bash
# Set your database URL
export DATABASE_URL="postgresql://user:pass@localhost:5432/attune"

# Run migrations (if not already done)
sqlx database create
sqlx migrate run

# Seed the core pack with example data
psql $DATABASE_URL -f scripts/seed_core_pack.sql
```

**Expected output:**
```
NOTICE:  Core pack seeded successfully
NOTICE:  Pack ID: 1
NOTICE:  Action Runtime ID: 1
NOTICE:  Sensor Runtime ID: 2
NOTICE:  Trigger Types: intervaltimer=1, crontimer=2, datetimetimer=3
NOTICE:  Actions: core.echo, core.sleep, core.noop
NOTICE:  Sensors: core.timer_10s_sensor (id=1)
NOTICE:  Rules: core.rule.timer_10s_echo
```

---

## Step 2: Configure Environment

Create a configuration file or set environment variables:

```bash
# Database
export ATTUNE__DATABASE__URL="postgresql://user:pass@localhost:5432/attune"

# Message Queue
export ATTUNE__RABBITMQ__URL="amqp://guest:guest@localhost:5672"

# JWT Secret (required for API service)
export ATTUNE__JWT_SECRET="your-secret-key-change-in-production"

# Optional: Set log level
export RUST_LOG="info,attune_sensor=debug,attune_executor=debug,attune_worker=debug"
```

---

## Step 3: Start the Services

Open **three separate terminals** and run:

### Terminal 1: Sensor Service
```bash
cd attune
cargo run --bin attune-sensor
```

**What it does:**
- Monitors the `core.timer_10s_sensor` sensor
- Fires a `core.intervaltimer` event every 10 seconds

**What to look for:**
```
[INFO] Sensor core.timer_10s_sensor started
[DEBUG] Timer fired: interval=10s
[DEBUG] Publishing event for trigger core.intervaltimer
```

### Terminal 2: Executor Service
```bash
cd attune
cargo run --bin attune-executor
```

**What it does:**
- Listens for events from sensors
- Evaluates rules against events
- Creates executions for matched rules

**What to look for:**
```
[INFO] Executor service started
[DEBUG] Event received for trigger core.intervaltimer
[DEBUG] Rule matched: core.rule.timer_10s_echo
[DEBUG] Creating enforcement for rule
[DEBUG] Scheduling execution for action core.echo
```

### Terminal 3: Worker Service
```bash
cd attune
cargo run --bin attune-worker
```

**What it does:**
- Receives execution requests
- Runs the `core.echo` action
- Returns results

**What to look for:**
```
[INFO] Worker service started
[DEBUG] Execution request received for action core.echo
[DEBUG] Running: echo "hello, world"
[INFO] Output: hello, world
[DEBUG] Execution completed successfully
```

---

## Step 4: Verify It's Working

You should see the complete flow every 10 seconds:

1. **Sensor** fires timer event
2. **Executor** matches rule and schedules execution
3. **Worker** executes action and outputs "hello, world"

---

## Understanding What You Created

### Components Seeded

| Component | Ref | Description |
|-----------|-----|-------------|
| **Trigger Type** | `core.intervaltimer` | Generic interval timer definition |
| **Sensor Instance** | `core.timer_10s_sensor` | Configured to fire every 10 seconds |
| **Action** | `core.echo` | Echoes a message to stdout |
| **Rule** | `core.rule.timer_10s_echo` | Connects trigger to action |

### The Flow

```
┌─────────────────────────────────────┐
│ core.timer_10s_sensor               │
│ Config: {"unit":"seconds","interval":10} │
└─────────────────────────────────────┘
            │
            │ every 10 seconds
            ▼
┌─────────────────────────────────────┐
│ Event (core.intervaltimer)          │
│ Payload: {"type":"interval",...}    │
└─────────────────────────────────────┘
            │
            │ triggers
            ▼
┌─────────────────────────────────────┐
│ Rule: core.rule.timer_10s_echo      │
│ Params: {"message":"hello, world"}  │
└─────────────────────────────────────┘
            │
            │ executes
            ▼
┌─────────────────────────────────────┐
│ Action: core.echo                   │
│ Command: echo "hello, world"        │
│ Output: hello, world                │
└─────────────────────────────────────┘
```

---

## Next Steps

### Modify the Message

Update the rule to echo a different message:

```sql
UPDATE attune.rule
SET action_params = '{"message": "Attune is running!"}'::jsonb
WHERE ref = 'core.rule.timer_10s_echo';
```

Restart the executor service to pick up the change.

### Create a Different Timer

Create a sensor that fires every 30 seconds:

```sql
INSERT INTO attune.sensor (
    ref, pack, pack_ref, label, description,
    entrypoint, runtime, runtime_ref,
    trigger, trigger_ref, enabled, config
)
VALUES (
    'mypack.timer_30s',
    (SELECT id FROM attune.pack WHERE ref = 'core'),
    'core',
    '30 Second Timer',
    'Fires every 30 seconds',
    'builtin:interval_timer',
    (SELECT id FROM attune.runtime WHERE ref = 'core.sensor.builtin'),
    'core.sensor.builtin',
    (SELECT id FROM attune.trigger WHERE ref = 'core.intervaltimer'),
    'core.intervaltimer',
    true,
    '{"unit": "seconds", "interval": 30}'::jsonb
);
```

Restart the sensor service to activate the new sensor.

### Use Dynamic Parameters

Update the rule to use event data:

```sql
UPDATE attune.rule
SET action_params = '{"message": "Timer fired at {{ event.payload.fired_at }}"}'::jsonb
WHERE ref = 'core.rule.timer_10s_echo';
```

The executor will resolve the template with actual event data.

---

## Troubleshooting

### No events firing
- Check that the sensor service is running
- Verify the sensor is enabled: `SELECT * FROM attune.sensor WHERE ref = 'core.timer_10s_sensor';`
- Check sensor service logs for errors

### Events firing but no executions
- Check that the executor service is running
- Verify the rule is enabled: `SELECT * FROM attune.rule WHERE ref = 'core.rule.timer_10s_echo';`
- Check executor service logs for rule matching

### Executions created but not running
- Check that the worker service is running
- Verify the action exists: `SELECT * FROM attune.action WHERE ref = 'core.echo';`
- Check worker service logs for execution errors

### Check the database
```sql
-- View recent events
SELECT * FROM attune.event ORDER BY created DESC LIMIT 10;

-- View recent enforcements
SELECT * FROM attune.enforcement ORDER BY created DESC LIMIT 10;

-- View recent executions
SELECT * FROM attune.execution ORDER BY created DESC LIMIT 10;
```

---

## Clean Up

To remove the example data:

```sql
-- Remove rule
DELETE FROM attune.rule WHERE ref = 'core.rule.timer_10s_echo';

-- Remove sensor
DELETE FROM attune.sensor WHERE ref = 'core.timer_10s_sensor';

-- (Triggers and actions are part of core pack, keep them)
```

Or drop and recreate the database:

```bash
sqlx database drop
sqlx database create
sqlx migrate run
```

---

## Learn More

- **Architecture Guide:** `docs/trigger-sensor-architecture.md`
- **Rule Parameters:** `docs/examples/rule-parameter-examples.md`
- **API Documentation:** `docs/api-*.md`
- **Service Details:** `docs/executor-service.md`, `docs/sensor-service.md`, `docs/worker-service.md`
