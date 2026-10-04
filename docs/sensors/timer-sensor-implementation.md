# Timer Sensor Implementation

## Overview

The timer sensor (`attune-core-timer-sensor`) is a standalone sensor service that monitors all timer-based triggers in Attune and fires events according to their schedules. It uses the [tokio-cron-scheduler](https://crates.io/crates/tokio-cron-scheduler) library for efficient asynchronous scheduling.

## Supported Timer Types

The timer sensor supports three distinct timer types, each with its own use case:

### 1. Interval Timers (`core.intervaltimer`)

Fires at regular intervals based on a specified time unit and interval value.

**Use Cases:**
- Periodic health checks
- Regular data synchronization
- Scheduled backups
- Continuous monitoring tasks

**Configuration:**
```yaml
trigger_ref: core.intervaltimer
parameters:
  unit: "seconds"    # Options: seconds, minutes, hours, days
  interval: 30       # Fire every 30 seconds
```

**Event Payload:**
```json
{
  "type": "interval",
  "interval_seconds": 30,
  "fired_at": "2024-01-20T15:30:00Z",
  "execution_count": 42,
  "sensor_ref": "core.interval_timer_sensor"
}
```

**Examples:**
- Fire every 10 seconds: `{unit: "seconds", interval: 10}`
- Fire every 5 minutes: `{unit: "minutes", interval: 5}`
- Fire every 2 hours: `{unit: "hours", interval: 2}`
- Fire daily: `{unit: "days", interval: 1}`

### 2. Cron Timers (`core.crontimer`)

Fires based on cron schedule expressions, providing flexible scheduling with fine-grained control.

**Use Cases:**
- Business hour operations (weekdays 9-5)
- Scheduled reports (daily at midnight, weekly on Monday)
- Complex recurring schedules
- Time-zone-aware scheduling

**Configuration:**
```yaml
trigger_ref: core.crontimer
parameters:
  expression: "0 0 9 * * 1-5"  # Weekdays at 9 AM
  timezone: "UTC"               # Optional, defaults to UTC
```

**Cron Format:**
```
second minute hour day_of_month month day_of_week
  |      |     |        |         |        |
  0-59  0-59  0-23    1-31      1-12    0-6 (0=Sun)
```

**Event Payload:**
```json
{
  "type": "cron",
  "fired_at": "2024-01-20T09:00:00Z",
  "scheduled_at": "2024-01-20T09:00:00Z",
  "expression": "0 0 9 * * 1-5",
  "timezone": "UTC",
  "next_fire_at": "2024-01-21T09:00:00Z",
  "execution_count": 15,
  "sensor_ref": "core.interval_timer_sensor"
}
```

**Examples:**
- Every hour: `"0 0 * * * *"`
- Every 15 minutes: `"0 */15 * * * *"`
- Daily at midnight: `"0 0 0 * * *"`
- Weekdays at 9 AM: `"0 0 9 * * 1-5"`
- Every Monday at 8:30 AM: `"0 30 8 * * 1"`

### 3. DateTime Timers (`core.datetimetimer`)

Fires once at a specific date and time. This is a one-shot timer that automatically removes itself after firing.

**Use Cases:**
- Scheduled deployments
- One-time notifications
- Event reminders
- Deadline triggers

**Configuration:**
```yaml
trigger_ref: core.datetimetimer
parameters:
  fire_at: "2024-12-31T23:59:59Z"  # ISO 8601 timestamp
  timezone: "UTC"                   # Optional, defaults to UTC
```

**Event Payload:**
```json
{
  "type": "one_shot",
  "fire_at": "2024-12-31T23:59:59Z",
  "fired_at": "2024-12-31T23:59:59.123Z",
  "timezone": "UTC",
  "delay_ms": 123,
  "sensor_ref": "core.interval_timer_sensor"
}
```

**Examples:**
- New Year countdown: `{fire_at: "2024-12-31T23:59:59Z"}`
- Specific deployment time: `{fire_at: "2024-06-15T14:00:00Z", timezone: "America/New_York"}`

## Implementation Details

### Architecture

The timer sensor uses a shared `JobScheduler` from tokio-cron-scheduler to manage all timer types efficiently:

1. **Initialization**: Creates a `JobScheduler` instance and starts it
2. **Job Creation**: Converts each timer config into the appropriate Job type
3. **Job Management**: Tracks active jobs by rule_id → job_uuid mapping
4. **Cleanup**: Properly shuts down the scheduler on service termination

### Key Components

**TimerManager** (`timer_manager.rs`):
- Central component that manages all timer jobs
- Methods:
  - `new()`: Creates and starts the scheduler
  - `start_timer()`: Adds/replaces a timer for a rule
  - `stop_timer()`: Removes a specific timer
  - `stop_all()`: Removes all timers
  - `shutdown()`: Gracefully shuts down the scheduler

**Job Types**:
- **Interval**: Uses `Job::new_repeated_async()` with fixed duration
- **Cron**: Uses `Job::new_async()` with cron expression
- **DateTime**: Uses `Job::new_one_shot_async()` with duration until fire time

### Event Creation

All timer types create events via the Attune API using the appropriate trigger ref:
- Interval → `core.intervaltimer`
- Cron → `core.crontimer`
- DateTime → `core.datetimetimer`

Each event includes:
- Trigger-specific metadata (execution count, next fire time, etc.)
- Timestamp information
- Sensor reference for tracking

### Rule Lifecycle Integration

The timer sensor listens to rule lifecycle events via RabbitMQ:
- **RuleCreated/RuleEnabled**: Starts timer for the rule
- **RuleDisabled**: Stops timer for the rule
- **RuleDeleted**: Stops and removes timer for the rule

Timer configuration is extracted from rule trigger parameters and converted to the appropriate `TimerConfig` enum variant.

## Dependencies

```toml
tokio-cron-scheduler = "0.15"  # Core scheduling library
chrono = "0.4"                  # Date/time handling
tokio = { version = "1.41", features = ["full"] }
```

## Testing

The implementation includes comprehensive tests covering:

1. **Unit Tests**:
   - Timer creation for all types
   - Validation (zero intervals, past dates, invalid cron)
   - Timer start/stop/restart
   - Job replacement

2. **Integration Tests**:
   - Multiple concurrent timers
   - Mixed timer type scenarios
   - Cron expression validation
   - Future datetime validation

Run tests:
```bash
cargo test -p core-timer-sensor
```

## Configuration

The timer sensor is configured via environment variables:

```bash
ATTUNE_API_URL=http://localhost:8080
ATTUNE_API_TOKEN=<scoped_sensor_token>
ATTUNE_SENSOR_REF=core.interval_timer_sensor
ATTUNE_LOG_LEVEL=info
```

Or via stdin JSON for containerized environments.

## Future Enhancements

Possible improvements for the timer sensor:

1. **Timezone Support**: Full timezone handling for cron expressions (currently UTC only)
2. **Persistence**: Store scheduled jobs in database for recovery after restart
3. **Job History**: Track execution history and statistics
4. **Advanced Scheduling**: Support for job chaining, dependencies, and priorities
5. **Performance Metrics**: Expose metrics on job execution timing and success rates

## References

- [tokio-cron-scheduler Documentation](https://docs.rs/tokio-cron-scheduler/)
- [Cron Expression Format](https://en.wikipedia.org/wiki/Cron)
- [ISO 8601 DateTime Format](https://en.wikipedia.org/wiki/ISO_8601)
