# Sensor Lifecycle Management

## Overview

Attune implements intelligent sensor lifecycle management to optimize resource usage and enhance security. Sensors are only started when there are active rules that subscribe to their triggers, and they are stopped (with token revocation) when no active rules exist.

This ensures:
- **Resource efficiency**: No CPU/memory wasted on sensors without consumers
- **Security**: API tokens are revoked when sensors are not in use
- **Cost optimization**: Reduced cloud infrastructure costs
- **Clean architecture**: Sensors operate on-demand based on actual usage

## Architecture

### Components

1. **SensorManager** - Manages sensor process lifecycle
2. **RuleLifecycleListener** - Monitors rule creation/enable/disable events via RabbitMQ
3. **Token Management** - Issues and revokes sensor authentication tokens
4. **Database Queries** - Tracks active rule counts per sensor

### Data Flow

```
Rule Change Event (RabbitMQ)
    ↓
RuleLifecycleListener
    ↓
SensorManager.handle_rule_change()
    ↓
Check active rule count for sensor
    ↓
┌─────────────────────────────┐
│ Active rules > 0?           │
├─────────────────────────────┤
│ YES → Sensor not running?   │
│       ├─ Issue token        │
│       ├─ Start sensor       │
│       └─ Register process   │
│                             │
│ NO → Sensor running?        │
│      ├─ Stop sensor         │
│      ├─ Revoke token        │
│      └─ Cleanup process     │
└─────────────────────────────┘
```

## Rule-Sensor-Trigger Relationship

### Database Schema

```sql
-- A sensor monitors a specific trigger type
sensor.trigger → trigger.id

-- A rule subscribes to a trigger
rule.trigger → trigger.id

-- Relationship: sensor ← trigger → rule(s)
-- Multiple rules can subscribe to the same trigger
-- One sensor can serve multiple rules (all sharing the trigger type)
```

### Active Rule Query

To determine if a sensor should be running:

```sql
SELECT COUNT(*)
FROM rule
WHERE trigger = (SELECT trigger FROM sensor WHERE id = $sensor_id)
  AND enabled = TRUE;
```

If count > 0: Sensor should be running
If count = 0: Sensor should be stopped

## Lifecycle States

### Sensor States

1. **STOPPED** - Sensor process not running, no token issued
2. **STARTING** - Token issued, process spawning
3. **RUNNING** - Process active, monitoring for trigger events
4. **STOPPING** - Process shutting down, token being revoked
5. **ERROR** - Failed to start/stop (requires manual intervention)

### State Transitions

```
STOPPED ──(rule created/enabled)──> STARTING ──(process ready)──> RUNNING
                                                                      │
                                                                      │
STOPPED <──(token revoked)──< STOPPING <──(rule disabled/deleted)────┘
```

## Implementation Details

### SensorManager Methods

#### `start_sensor(sensor_id)`

1. Query database for sensor configuration
2. Issue service account token via API
   - Type: `sensor`
   - Scope: Sensor-specific trigger types
   - TTL: 90 days (with auto-refresh)
3. Start sensor process:
   - **Native sensors**: Spawn binary with environment config
   - **Python/Script sensors**: Execute via runtime
4. Register process handle in memory
5. Monitor process health

#### `stop_sensor(sensor_id, revoke_token)`

1. Send SIGTERM to sensor process
2. Wait for graceful shutdown (timeout: 30s)
3. Force kill (SIGKILL) if timeout exceeded
4. If `revoke_token == true`:
   - Call API to revoke sensor token
   - Add token to revocation table
5. Remove from running sensors registry
6. Log shutdown event

#### `handle_rule_change(trigger_id)`

1. Find all sensors for the given trigger
2. For each sensor:
   - Query active rule count
   - Check if sensor is currently running
   - Determine action based on state matrix:

| Active Rules | Running | Action                        |
|--------------|---------|-------------------------------|
| Yes          | Yes     | No action (continue running)  |
| Yes          | No      | Start sensor + issue token    |
| No           | Yes     | Stop sensor + revoke token    |
| No           | No      | No action (remain stopped)    |

### RuleLifecycleListener Integration

The `RuleLifecycleListener` subscribes to these RabbitMQ events:

- `rule.created` - New rule added
- `rule.enabled` - Existing rule activated
- `rule.disabled` - Existing rule deactivated
- `rule.deleted` - Rule removed (future)

On each event:

```rust
async fn handle_rule_event(event: RuleEvent) {
    // Extract trigger_id from rule
    let trigger_id = get_trigger_for_rule(event.rule_id).await?;
    
    // Notify sensor manager
    sensor_manager.handle_rule_change(trigger_id).await?;
}
```

## Token Management

### Token Issuance

When a sensor needs to start:

```rust
// Create service account for sensor
let token = api_client.create_sensor_token(SensorTokenRequest {
    sensor_id,
    sensor_ref: "core.interval_timer_sensor",
    trigger_types: vec!["core.intervaltimer"],
    ttl_days: 90,
}).await?;

// Pass token to sensor via environment variable
env::set_var("ATTUNE_API_TOKEN", token.access_token);
```

### Token Revocation

When a sensor is stopped:

```rust
// Revoke sensor token
api_client.revoke_token(token_id).await?;

// Token is added to revocation table with expiration
// Cleanup job removes expired revocations periodically
```

### Token Refresh

Native sensors (like `attune-core-timer-sensor`) implement automatic token refresh:

```rust
// TokenRefreshManager runs in background
// Refreshes token at 80% of TTL (72 days for 90-day tokens)
let refresh_manager = TokenRefreshManager::new(api_client, 0.8);
refresh_manager.start();
```

## Sensor Process Management

### Native Sensors (Rust Binaries)

Native sensors are standalone executables managed by the SensorManager:

```bash
# Start command
ATTUNE_API_URL=http://api:8080 \
ATTUNE_API_TOKEN=<token> \
ATTUNE_SENSOR_REF=core.interval_timer_sensor \
./attune-core-timer-sensor

# Process management
- PID tracking in SensorManager
- SIGTERM for graceful shutdown
- SIGKILL fallback after 30s
- Restart on crash (max 3 attempts)
```

### Script-Based Sensors (Python/Shell)

Managed sensor processes use scoped HTTP API access for event emission.
Their selected base environment and operator passthrough settings are documented
in [Child-process environments](../deployment/child-process-environments.md).

Script sensors are executed through the worker runtime:

```python
# Python sensor example
class IntervalTimerSensor:
    def __init__(self, api_token, sensor_ref):
        self.api_client = ApiClient(token=api_token)
        self.sensor_ref = sensor_ref
    
    def run(self):
        while True:
            # Check triggers
            # Emit events
            time.sleep(self.poll_interval)
```

Managed similarly to native sensors but executed via Python runtime.

## Database Schema Additions

### Sensor Process Tracking

```sql
-- Add to sensor table (future enhancement)
ALTER TABLE sensor ADD COLUMN process_id INTEGER;
ALTER TABLE sensor ADD COLUMN last_started TIMESTAMPTZ;
ALTER TABLE sensor ADD COLUMN last_stopped TIMESTAMPTZ;
ALTER TABLE sensor ADD COLUMN active_token_id BIGINT REFERENCES identity(id);
ALTER TABLE sensor ADD COLUMN restart_count INTEGER DEFAULT 0;
ALTER TABLE sensor ADD COLUMN status sensor_status_enum DEFAULT 'stopped';

CREATE TYPE sensor_status_enum AS ENUM (
    'stopped',
    'starting',
    'running',
    'stopping',
    'error'
);
```

### Active Rules View

```sql
-- View to quickly check sensors that should be running
CREATE VIEW active_sensors AS
SELECT 
    s.id,
    s.ref AS sensor_ref,
    s.trigger,
    t.ref AS trigger_ref,
    COUNT(r.id) AS active_rule_count,
    CASE WHEN COUNT(r.id) > 0 THEN true ELSE false END AS should_be_running
FROM sensor s
JOIN trigger t ON t.id = s.trigger
LEFT JOIN rule r ON r.trigger = s.trigger AND r.enabled = TRUE
WHERE s.enabled = TRUE
GROUP BY s.id, s.ref, s.trigger, t.ref;
```

## Monitoring and Observability

### Metrics

Track the following metrics:

- **Sensor lifecycle events**: starts, stops, crashes
- **Token operations**: issued, refreshed, revoked
- **Active sensor count**: gauge of running sensors
- **Rule-to-sensor ratio**: avg rules per sensor
- **Token refresh success rate**: % of successful refreshes

### Logging

All lifecycle events are logged with structured data:

```json
{
  "event": "sensor_started",
  "sensor_id": 42,
  "sensor_ref": "core.interval_timer_sensor",
  "trigger_ref": "core.intervaltimer",
  "active_rules": 3,
  "token_issued": true,
  "timestamp": "2025-01-29T22:00:00Z"
}
```

```json
{
  "event": "sensor_stopped",
  "sensor_id": 42,
  "sensor_ref": "core.interval_timer_sensor",
  "reason": "no_active_rules",
  "token_revoked": true,
  "uptime_seconds": 3600,
  "timestamp": "2025-01-29T23:00:00Z"
}
```

### Health Checks

SensorManager runs a monitoring loop (every 60s) to:

- Check process health (is PID alive?)
- Verify event emission (has sensor emitted events recently?)
- Restart crashed sensors (if rules still active)
- Update sensor status in database

## API Endpoints

### Token Management

```http
POST /auth/sensor-token
Content-Type: application/json

{
  "sensor_id": 42,
  "sensor_ref": "core.interval_timer_sensor",
  "trigger_types": ["core.intervaltimer"],
  "ttl_days": 90
}

Response: {
  "access_token": "eyJ...",
  "token_type": "bearer",
  "expires_in": 7776000,
  "sensor_ref": "core.interval_timer_sensor"
}
```

```http
POST /auth/refresh
Authorization: Bearer <current_token>

Response: {
  "access_token": "eyJ...",
  "expires_in": 7776000
}
```

```http
DELETE /auth/token/:token_id
Authorization: Bearer <admin_token>

Response: 204 No Content
```

### Sensor Status

```http
GET /api/v1/sensors/:sensor_id/status
Authorization: Bearer <token>

Response: {
  "sensor_id": 42,
  "sensor_ref": "core.interval_timer_sensor",
  "status": "running",
  "active_rules": 3,
  "last_started": "2025-01-29T22:00:00Z",
  "uptime_seconds": 3600,
  "events_emitted": 120
}
```

## Edge Cases and Error Handling

### Rapid Rule Toggling

**Scenario**: Rule is rapidly enabled/disabled

**Solution**: Debounce sensor lifecycle changes (5s window)

```rust
// Only process one lifecycle change per sensor per 5 seconds
let last_change = sensor_manager.last_change_time(sensor_id);
if last_change.elapsed() < Duration::from_secs(5) {
    debug!("Debouncing lifecycle change for sensor {}", sensor_id);
    return Ok(());
}
```

### Sensor Crash During Startup

**Scenario**: Sensor process crashes immediately after starting

**Solution**: Exponential backoff with max retry limit

```rust
async fn start_sensor_with_retry(sensor_id: i64) -> Result<()> {
    for attempt in 1..=MAX_RETRIES {
        match start_sensor(sensor_id).await {
            Ok(_) => return Ok(()),
            Err(e) => {
                error!("Sensor start attempt {} failed: {}", attempt, e);
                if attempt < MAX_RETRIES {
                    let delay = Duration::from_secs(2u64.pow(attempt));
                    tokio::time::sleep(delay).await;
                } else {
                    return Err(e);
                }
            }
        }
    }
    Err(anyhow!("Max retries exceeded"))
}
```

### Token Revocation Failure

**Scenario**: API is unreachable when trying to revoke token

**Solution**: Queue revocation for retry, proceed with shutdown

```rust
if let Err(e) = revoke_token(token_id).await {
    error!("Failed to revoke token {}: {}", token_id, e);
    // Queue for retry
    pending_revocations.push(token_id);
    // Continue with sensor shutdown anyway
}
```

### Database Connectivity Loss

**Scenario**: Cannot query active rule count

**Solution**: Fail-safe to keep sensors running (avoid downtime)

```rust
match get_active_rule_count(sensor_id).await {
    Ok(count) => handle_based_on_count(count),
    Err(e) => {
        error!("Cannot query rule count: {}", e);
        // Keep sensor running to avoid disruption
        warn!("Keeping sensor running due to DB error");
    }
}
```

## Migration Strategy

### Phase 1: Implement Core Logic (Current)

1. Add `has_active_rules()` to SensorManager ✓
2. Modify `start()` to check active rules before starting ✓
3. Add `handle_rule_change()` method ✓
4. Integrate with RuleLifecycleListener ✓

### Phase 2: Token Management

1. Add sensor token issuance to API
2. Implement token revocation endpoint
3. Add token cleanup job for expired revocations
4. Update sensor startup to use issued tokens

### Phase 3: Process Management

1. Track sensor PIDs in SensorManager
2. Implement graceful shutdown (SIGTERM)
3. Add process health monitoring
4. Implement restart logic with backoff

### Phase 4: Observability

1. Add structured logging for lifecycle events
2. Expose metrics for monitoring
3. Add sensor status endpoint to API
4. Create admin dashboard for sensor management

## Testing Strategy

### Unit Tests

```rust
#[tokio::test]
async fn test_sensor_starts_with_active_rules() {
    let manager = SensorManager::new(...);
    let sensor = create_test_sensor();
    let rule = create_test_rule(sensor.trigger);
    
    manager.handle_rule_change(sensor.trigger).await.unwrap();
    
    assert!(manager.is_running(sensor.id));
}

#[tokio::test]
async fn test_sensor_stops_when_last_rule_disabled() {
    let manager = SensorManager::new(...);
    let sensor = create_running_sensor();
    
    // Disable all rules
    disable_all_rules(sensor.trigger).await;
    
    manager.handle_rule_change(sensor.trigger).await.unwrap();
    
    assert!(!manager.is_running(sensor.id));
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_end_to_end_lifecycle() {
    // 1. Create sensor (should not start)
    let sensor = create_sensor().await;
    assert_sensor_stopped(sensor.id);
    
    // 2. Create enabled rule (sensor should start)
    let rule = create_enabled_rule(sensor.trigger).await;
    wait_for_sensor_running(sensor.id);
    
    // 3. Disable rule (sensor should stop)
    disable_rule(rule.id).await;
    wait_for_sensor_stopped(sensor.id);
    
    // 4. Verify token was revoked
    assert_token_revoked(sensor.token_id);
}
```

## Future Enhancements

1. **Smart Scheduling**: Start sensors 30s before first rule execution
2. **Shared Sensors**: Multiple sensor types sharing same infrastructure
3. **Auto-scaling**: Spawn multiple sensor instances for high-volume triggers
4. **Circuit Breakers**: Disable sensors that repeatedly fail
5. **Cost Tracking**: Track resource consumption per sensor
6. **Sensor Pools**: Pre-warmed sensor processes for fast activation

## See Also

- [Sensor Architecture](sensor-architecture.md)
- [Timer Sensor Implementation](../crates/core-timer-sensor/README.md)
- [Token Security](token-security.md)
- [Rule Lifecycle Events](rule-lifecycle.md)
