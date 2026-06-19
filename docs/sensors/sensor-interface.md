# Sensor Interface Specification

**Version:** 1.0  
**Last Updated:** 2025-01-27  
**Status:** Draft

## Overview

This document specifies the standard interface that all Attune sensors must implement. Sensors are lightweight, long-running daemon processes that monitor for events and emit them into the Attune platform. Each sensor type has exactly one process instance running at a time, and individual sensor instances are managed dynamically based on active rules.

> Scope note: this interface describes **managed sensor processes**. Internal platform services (such as `attune-sensor`) may continue using RabbitMQ for internal coordination.

## Design Principles

1. **Single Process Per Sensor Type**: Each sensor type (e.g., timer, webhook, file_watcher) runs as a single daemon process
2. **Lightweight & Async**: Sensors should be event-driven and non-blocking
3. **Rule-Driven Behavior**: Sensors manage multiple concurrent "instances" based on active rules
4. **WebSocket Control Stream for Managed Sensors**: Managed sensor processes receive rule lifecycle control messages over authenticated notifier WebSocket subscriptions
5. **API Integration**: Sensors use the Attune API to emit events and fetch configuration
6. **Standard Authentication**: Sensors authenticate using transient API tokens
7. **Graceful Lifecycle**: Sensors handle startup, shutdown, and dynamic reconfiguration

## Sensor Lifecycle

### 1. Initialization

When a sensor starts, it must:

1. **Read Configuration** from environment variables or stdin
2. **Authenticate** with the Attune API using a transient token
3. **Connect to Notifier WebSocket** and subscribe to trigger-scoped lifecycle updates
4. **Load Active Rules** from the API that use its trigger types
5. **Start Monitoring** for each active rule
6. **Signal Ready** (log startup completion)

### 2. Runtime Operation

During normal operation, a sensor:

1. **Listens to notifier WebSocket** for rule lifecycle messages (`rule.created`, `rule.enabled`, `rule.disabled`, `rule.deleted`)
2. **Monitors External Sources** (timers, webhooks, file systems, etc.) based on active rules
3. **Emits Events** to the Attune API when trigger conditions are met
4. **Handles Errors** gracefully without crashing
5. **Reports Health** (periodic heartbeat/metrics - future)

### 3. Shutdown

On shutdown (SIGTERM/SIGINT), a sensor must:

1. **Stop Accepting New Work** (stop listening to lifecycle stream)
2. **Cancel Active Monitors** (stop timers, close connections)
3. **Flush Pending Events** (send any buffered events to API)
4. **Close Connections** (WebSocket, HTTP clients)
5. **Exit Cleanly** with appropriate exit code

## Configuration

### Environment Variables

Sensors MUST accept the following environment variables:

| Variable | Required | Description | Example |
|----------|----------|-------------|---------|
| `ATTUNE_API_URL` | Yes | Base URL of Attune API | `http://localhost:8080` |
| `ATTUNE_API_TOKEN` | Yes | Transient API token for authentication | `sensor_abc123...` |
| `ATTUNE_SENSOR_ID` | Yes | Sensor database ID | `42` |
| `ATTUNE_SENSOR_REF` | Yes | Reference name of this sensor | `core.timer` |
| `ATTUNE_NOTIFIER_WS_URL` | Yes | Notifier websocket URL for lifecycle stream | `ws://localhost:8081/ws` |
| `ATTUNE_LOG_LEVEL` | No | Logging verbosity | `info` (default) |

**Note:** These environment variables provide parity with action execution context (see `QUICKREF-execution-environment.md`). Sensors receive:
- `ATTUNE_SENSOR_ID` - analogous to `ATTUNE_EXEC_ID` for actions
- `ATTUNE_SENSOR_REF` - analogous to `ATTUNE_ACTION` for actions
- `ATTUNE_API_TOKEN` and `ATTUNE_API_URL` - same as actions for API access

### Alternative: stdin Configuration

For containerized or orchestrated deployments, sensors MAY accept configuration as JSON on stdin:

```json
{
  "api_url": "http://localhost:8080",
  "api_token": "sensor_abc123...",
  "sensor_ref": "core.timer",
  "notifier_ws_url": "ws://localhost:8081/ws",
  "log_level": "info"
}
```

If stdin is provided, it takes precedence over environment variables. The JSON must be a single line or complete object, followed by EOF or newline.

## API Authentication: Transient Tokens

### Token Requirements

- **Type**: JWT with `service_account` identity type
- **Scope**: Limited to sensor operations (create events, read rules)
- **Lifetime**: Long-lived (90 days) and auto-expires
- **Rotation**: Automatic refresh (sensor refreshes token when 80% of TTL elapsed)
- **Zero-Downtime**: Hot-reload new tokens without restart

### Token Format

Sensors receive a standard JWT that includes:

```json
{
  "sub": "sensor:core.timer",
  "jti": "abc123def456",  // JWT ID for revocation tracking
  "identity_id": 123,
  "identity_type": "service_account",
  "scope": "sensor",
  "iat": 1738800000,  // Issued at
  "exp": 1738886400,  // Expires in 24-72 hours (REQUIRED)
  "metadata": {
    "trigger_types": ["core.timer"]  // Enforced by API
  }
}
```

### API Endpoints Used by Sensors

Sensors interact with the following API endpoints:

| Method | Endpoint | Purpose | Auth |
|--------|----------|---------|------|
| GET | `/rules?trigger_type={ref}` | Fetch active rules for this sensor's triggers | Required |
| GET | `/triggers/{ref}` | Fetch trigger metadata | Required |
| POST | `/events` | Create new event | Required |
| POST | `/auth/refresh` | Refresh token before expiration | Required |
| GET | `/health` | Verify API connectivity | Optional |

## Rule Lifecycle Stream (Managed Sensors)

Managed sensor processes consume lifecycle deltas through `attune-notifier` over WebSocket.

### Subscription model

- Sensors connect to `/ws` with `Authorization: Bearer <sensor-token>` (or browser-equivalent subprotocol token transport).
- Sensors subscribe using `trigger_ref:<trigger-ref>` filters.
- Subscription ACLs are enforced against sensor token `metadata.trigger_types`.
- Managed sensors should subscribe to all trigger refs they support.

### Message Format

Lifecycle payloads are delivered inside notifier `notification` envelopes. The `payload` field follows this schema:

```json
{
  "event_type": "rule.created | rule.enabled | rule.disabled | rule.deleted",
  "rule_id": 123,
  "rule_ref": "core.timer_rule",
  "trigger_ref": "core.timer",
  "trigger_params": {
    "interval_seconds": 5
  },
  "active": true,
  "timestamp": "2025-01-27T12:34:56Z"
}
```

### Message Handling

Sensors MUST:

1. **Validate** messages against expected schema
2. **Filter** messages to only process rules for their trigger refs (based on token's `metadata.trigger_types`)
3. **Handle Duplicates** idempotently (same rule_id + event_type)
4. **Reconnect** with backoff on socket disconnect
5. **Enforce Trigger Type Restrictions**: Only emit events for trigger refs declared in the sensor token metadata

## Event Emission

### Event Creation API

Sensors create events by POSTing to `/events`:

```http
POST /events
Authorization: Bearer {sensor_token}
Content-Type: application/json

{
  "trigger_ref": "core.timer",
  "payload": {
    "timestamp": "2025-01-27T12:34:56Z",
    "scheduled_time": "2025-01-27T12:34:56Z"
  },
  "trigger_instance_id": "rule_123"
}
```

> **Note**: `trigger_type` is accepted as an alias for `trigger_ref` for backward compatibility, but `trigger_ref` is the canonical field name.

**Important**: Sensors can only emit events for trigger types declared in their token's `metadata.trigger_types`. The API will reject event creation requests for unauthorized trigger types with a `403 Forbidden` error.

### Event Payload Guidelines

- **Timestamp**: Always include event occurrence time
- **Context**: Include relevant context for rule evaluation
- **Size**: Keep payloads small (<1KB recommended, <10KB max)
- **Sensitive Data**: Never include passwords, tokens, or PII unless explicitly required
- **Trigger Type Match**: The `trigger_type` field must match one of the sensor's declared trigger types

### Error Handling

If event creation fails:

1. **Retry** with exponential backoff (3 attempts)
2. **Log Error** with full context
3. **Continue Operating** (don't crash on single event failure)
4. **Alert** if failure rate exceeds threshold (future)

## Sensor-Specific Behavior

Each sensor type implements trigger-specific logic. The sensor monitors external sources and translates them into Attune events.

### Example: Timer Sensor

**Trigger Type**: `core.timer`

**Parameters**:
```json
{
  "interval_seconds": 60
}
```

**Behavior**:
- Maintains a hash map of `rule_id -> tokio::task::JoinHandle`
- On `RuleCreated`/`RuleEnabled`: Start an async timer loop for the rule
- On `RuleDisabled`/`RuleDeleted`: Cancel the timer task for the rule
- Timer loop: Every interval, emit an event with current timestamp

**Event Payload**:
```json
{
  "timestamp": "2025-01-27T12:34:56Z",
  "scheduled_time": "2025-01-27T12:34:56Z"
}
```

### Example: Webhook Sensor

**Trigger Type**: `core.webhook`

**Parameters**:
```json
{
  "path": "/hooks/deployment",
  "method": "POST",
  "secret": "shared_secret_123"
}
```

**Behavior**:
- Runs an HTTP server listening on configured port
- On `RuleCreated`/`RuleEnabled`: Register a route handler for the webhook path
- On `RuleDisabled`/`RuleDeleted`: Unregister the route handler
- On incoming request: Validate secret, emit event with request body

**Event Payload**:
```json
{
  "timestamp": "2025-01-27T12:34:56Z",
  "method": "POST",
  "path": "/hooks/deployment",
  "headers": {"Content-Type": "application/json"},
  "body": {"status": "deployed"}
}
```

### Example: File Watcher Sensor

**Trigger Type**: `core.file_changed`

**Parameters**:
```json
{
  "path": "/var/log/app.log",
  "event_types": ["modified", "created"]
}
```

**Behavior**:
- Uses inotify/FSEvents/equivalent to watch file system
- On `RuleCreated`/`RuleEnabled`: Add watch for the specified path
- On `RuleDisabled`/`RuleDeleted`: Remove watch for the path
- On file system event: Emit event with file details

**Event Payload**:
```json
{
  "timestamp": "2025-01-27T12:34:56Z",
  "path": "/var/log/app.log",
  "event_type": "modified",
  "size": 12345
}
```

## Implementation Guidelines

### Language & Runtime

- **Recommended**: Rust (for consistency with Attune services)
- **Alternatives**: Python, Node.js, Go (if justified by use case)
- **Async I/O**: Required for scalability

### Dependencies

Sensors should use:

- **HTTP Client**: For API communication (e.g., `reqwest` in Rust)
- **WebSocket Client**: For notifier lifecycle stream (e.g., `tokio-tungstenite` in Rust)
- **Async Runtime**: For concurrency (e.g., `tokio` in Rust)
- **JSON Parsing**: For message/event handling (e.g., `serde_json` in Rust)
- **Logging**: Structured logging (e.g., `tracing` in Rust)

### Error Handling

- **Panic/Crash**: Never panic on external input (messages, API responses)
- **Retry Logic**: Implement exponential backoff for transient failures
- **Circuit Breaker**: Consider circuit breaker for API calls (future)
- **Graceful Degradation**: Continue operating even if some rules fail

### Logging

Sensors MUST log:

- **Startup**: Configuration loaded, connections established
- **Rule Changes**: Rule added/removed/updated
- **Events Emitted**: Event type and rule_id (not full payload)
- **Errors**: All errors with context
- **Shutdown**: Graceful shutdown initiated and completed

Log format should be JSON for structured logging:

```json
{
  "timestamp": "2025-01-27T12:34:56Z",
  "level": "info",
  "sensor": "core.timer",
  "message": "Timer started for rule",
  "rule_id": 123,
  "interval_seconds": 5
}
```

### Testing

Sensors should include:

- **Unit Tests**: Test message parsing, event creation logic
- **Integration Tests**: Test against real notifier WebSocket and API (test environment)
- **Mock Tests**: Test with mocked API/WebSocket for isolated testing

## Security Considerations

### Token Storage

- **Never Log Tokens**: Redact tokens in logs
- **Memory Only**: Keep tokens in memory, never write to disk
- **Automatic Refresh**: Refresh token when 80% of TTL elapsed (no restart required)
- **Hot-Reload**: Update in-memory token without interrupting operations
- **Refresh Failure Handling**: Log errors and retry with exponential backoff

### Input Validation

- **Validate All Inputs**: lifecycle stream messages, API responses
- **Sanitize Payloads**: Prevent injection attacks in event payloads
- **Rate Limiting**: Prevent resource exhaustion from malicious triggers
- **Trigger Type Enforcement**: API validates that sensor tokens can only create events for declared trigger types

### Network Security

- **TLS**: Use HTTPS for API calls in production
- **WSS**: Use TLS for notifier WebSocket in production
- **Timeouts**: Set reasonable timeouts for all network calls

## Deployment

### Service Management

Sensors should be managed as system services:

- **systemd**: Linux deployments
- **launchd**: macOS deployments
- **Docker**: Container deployments
- **Kubernetes**: Orchestrated deployments (one pod per sensor type)

### Resource Limits

Recommended limits:

- **Memory**: 64-256 MB per sensor (depends on rule count)
- **CPU**: Minimal (<5% avg, spikes allowed)
- **Network**: Low bandwidth (<1 Mbps typical)
- **Disk**: Minimal (logs only)

### Monitoring

Sensors should expose metrics (future):

- **Rules Active**: Count of rules being monitored
- **Events Emitted**: Counter of events created
- **Errors**: Counter of errors by type
- **API Latency**: Histogram of API call durations
- **Lifecycle Stream Latency**: Histogram of message processing durations

## Compatibility

### Versioning

Sensors should:

- **Declare Version**: Include sensor version in logs and metrics
- **API Compatibility**: Support current API version
- **Message Compatibility**: Handle unknown fields gracefully

### Backwards Compatibility

When updating sensors:

- **Add Fields**: New message fields are optional
- **Deprecate Fields**: Old fields remain supported for 2+ versions
- **Breaking Changes**: Require major version bump and migration guide

## Appendix: Reference Implementation

See `attune/crates/sensor/` for the reference timer sensor implementation in Rust.

Key components:

- `src/main.rs` - Initialization and configuration
- `src/listener.rs` - lifecycle stream message handling
- `src/timer.rs` - Timer-specific logic
- `src/api_client.rs` - API communication

## Appendix: Message Queue Schema

### Rule Lifecycle Messages

**Exchange**: `attune` (topic exchange)

**RuleCreated**:
```json
{
  "event_type": "RuleCreated",
  "rule_id": 123,
  "rule_ref": "timer_every_5s",
  "trigger_type": "core.timer",
  "trigger_params": {"interval_seconds": 5},
  "enabled": true,
  "timestamp": "2025-01-27T12:34:56Z"
}
```

**RuleEnabled**:
```json
{
  "event_type": "RuleEnabled",
  "rule_id": 123,
  "trigger_type": "core.timer",
  "trigger_params": {"interval_seconds": 5},
  "timestamp": "2025-01-27T12:34:56Z"
}
```

**RuleDisabled**:
```json
{
  "event_type": "RuleDisabled",
  "rule_id": 123,
  "trigger_type": "core.timer",
  "timestamp": "2025-01-27T12:34:56Z"
}
```

**RuleDeleted**:
```json
{
  "event_type": "RuleDeleted",
  "rule_id": 123,
  "trigger_type": "core.timer",
  "timestamp": "2025-01-27T12:34:56Z"
}
```

## Appendix: API Token Management

### Creating Sensor Tokens

Tokens are created via the Attune API (admin only):

```http
POST /service-accounts
Authorization: Bearer {admin_token}
Content-Type: application/json

{
  "name": "sensor:core.timer",
  "description": "Timer sensor service account",
  "scope": "sensor",
  "ttl_days": 90
}
```

Response:
```json
{
  "identity_id": 123,
  "name": "sensor:core.timer",
  "token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "expires_at": "2025-04-27T12:34:56Z"
}
```

### Token Scopes

| Scope | Permissions |
|-------|-------------|
| `sensor` | Create events, read rules/triggers |
| `action` | Read keys, update execution status (for action runners) |
| `admin` | Full access (for CLI, web UI) |

## Token Lifecycle Management

### Automatic Token Refresh

Sensors automatically refresh their own tokens without human intervention:

**Refresh Timing:**
- Tokens have 90-day TTL
- Sensors refresh when 80% of TTL elapsed (72 days)
- Calculation: `refresh_at = issued_at + (TTL * 0.8)`

**Refresh Process:**
1. Background task monitors token expiration
2. When refresh threshold reached, call `POST /auth/refresh` with current token
3. Receive new token with fresh 90-day TTL
4. Hot-load new token (update in-memory reference)
5. Old token remains valid until original expiration
6. Continue operations without interruption

**Implementation Pattern:**
```rust
// Calculate when to refresh (80% of TTL)
let token_exp = decode_jwt(&token)?.exp;
let token_iat = decode_jwt(&token)?.iat;
let ttl_seconds = token_exp - token_iat;
let refresh_at = token_iat + (ttl_seconds * 8 / 10);

// Spawn background refresh task
tokio::spawn(async move {
    loop {
        let now = current_timestamp();
        if now >= refresh_at {
            match api_client.refresh_token().await {
                Ok(new_token) => {
                    update_token(new_token);
                    info!("Token refreshed successfully");
                }
                Err(e) => {
                    error!("Failed to refresh token: {}", e);
                    // Retry with exponential backoff
                }
            }
        }
        sleep(Duration::from_hours(1)).await;
    }
});
```

**Refresh Failure Handling:**
1. Log error with full context
2. Retry with exponential backoff (1min, 2min, 4min, 8min, max 1 hour)
3. Continue using old token (still valid until expiration)
4. Alert monitoring system after 3 consecutive failures
5. If old token expires before successful refresh, shut down gracefully

**Zero-Downtime:**
- Old token valid during refresh
- No service interruption
- Graceful degradation on failure
- No manual intervention required

### Token Expiration (Edge Case)

If automatic refresh fails and token expires:

1. API returns 401 Unauthorized
2. Sensor logs critical error
3. Sensor shuts down gracefully (stops accepting work, completes in-flight operations)
4. Operator must manually create new token and restart sensor

**This should rarely occur** if automatic refresh is working correctly.

## Future Enhancements

1. **Health Checks**: HTTP endpoint for liveness/readiness probes
2. **Metrics Export**: Prometheus-compatible metrics endpoint (including token refresh metrics)
3. **Dynamic Discovery**: Auto-discover available sensors from registry
4. **Sensor Scaling**: Support multiple instances per sensor type with work distribution
5. **Backpressure**: Handle event backlog when API is slow/unavailable
6. **Circuit Breaker**: Automatic failover when API is unreachable
7. **Sensor Plugins**: Dynamic loading of sensor implementations
8. **Configurable Refresh Threshold**: Allow custom refresh timing (e.g., 75%, 85%)
9. **Token Refresh Alerts**: Alert on refresh failures, not normal refresh events