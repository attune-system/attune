# Attune Timer Sensor

A standalone sensor daemon for the Attune automation platform that monitors timer-based triggers and emits events. This sensor manages multiple concurrent timer schedules based on active rules.

## Overview

The timer sensor is a lightweight, event-driven process that:

- Listens for rule lifecycle events via notifier WebSocket
- Manages per-rule timer tasks dynamically
- Emits events to the Attune API when timers fire
- Supports interval-based, cron-based, and datetime-based timers
- Authenticates using service account tokens

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ Timer Sensor Process                                         │
│                                                              │
│  ┌────────────────┐    ┌──────────────────┐                │
│  │ Rule Lifecycle │───▶│  Timer Manager   │                │
│  │   Listener     │    │                  │                │
│  │  (WebSocket)   │    │ ┌──────────────┐ │                │
│  └────────────────┘    │ │ Rule 1 Timer │ │                │
│                        │ ├──────────────┤ │                │
│                        │ │ Rule 2 Timer │ │───┐            │
│                        │ ├──────────────┤ │   │            │
│                        │ │ Rule 3 Timer │ │   │            │
│                        │ └──────────────┘ │   │            │
│                        └──────────────────┘   │            │
│                                               │            │
│  ┌────────────────┐                           │            │
│  │  API Client    │◀──────────────────────────┘            │
│  │ (Create Events)│                                        │
│  └────────────────┘                                        │
└─────────────────────────────────────────────────────────────┘
         │                                  ▲
         │ Events                           │ Rule Lifecycle
         ▼                                  │ Messages
┌─────────────────┐              ┌─────────────────┐
│  Attune API     │              │ attune-notifier │
└─────────────────┘              └─────────────────┘
```

## Features

- **Per-Rule Timers**: Each rule gets its own independent timer task
- **Dynamic Management**: Timers start/stop automatically based on rule lifecycle
- **Multiple Timer Types**:
  - **Interval**: Fire every N seconds/minutes/hours/days
  - **Cron**: Fire based on cron expression (planned)
  - **DateTime**: Fire at a specific date/time
- **Resilient**: Retries event creation with exponential backoff
- **Secure**: Token-based authentication with trigger type restrictions
- **Observable**: Structured JSON logging for monitoring

## Installation

### From Source

```bash
cargo build --release --package core-timer-sensor
sudo cp target/release/attune-core-timer-sensor /usr/local/bin/
```

### Using Cargo Install

```bash
cargo install --path crates/core-timer-sensor
```

## Configuration

### Environment Variables

The sensor requires the following environment variables:

| Variable | Required | Description | Example |
|----------|----------|-------------|---------|
| `ATTUNE_API_URL` | Yes | Base URL of the Attune API | `http://localhost:8080` |
| `ATTUNE_API_TOKEN` | Yes | Service account token | `eyJhbGci...` |
| `ATTUNE_SENSOR_REF` | Yes | Sensor reference (must be `core.timer`) | `core.timer` |
| `ATTUNE_NOTIFIER_WS_URL` | Yes | Notifier websocket URL for lifecycle updates | `ws://localhost:8081/ws` |
| `ATTUNE_LOG_LEVEL` | No | Logging verbosity | `info` (default) |

### Example: Environment Variables

```bash
export ATTUNE_API_URL="http://localhost:8080"
export ATTUNE_API_TOKEN="eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
export ATTUNE_SENSOR_REF="core.timer"
export ATTUNE_NOTIFIER_WS_URL="ws://localhost:8081/ws"
export ATTUNE_LOG_LEVEL="info"

attune-core-timer-sensor
```

### Example: stdin Configuration

```bash
echo '{
  "api_url": "http://localhost:8080",
  "api_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "sensor_ref": "core.timer",
  "notifier_ws_url": "ws://localhost:8081/ws",
  "log_level": "info"
}' | attune-core-timer-sensor --stdin-config
```

## Service Account Setup

Before running the sensor, you need to create a service account with the appropriate permissions:

```bash
# Create service account (requires admin token)
curl -X POST http://localhost:8080/service-accounts \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "sensor:core.timer",
    "scope": "sensor",
    "description": "Timer sensor for interval-based triggers",
    "ttl_hours": 72,
    "metadata": {
      "trigger_types": ["core.timer"]
    }
  }'

# Response will include the token (save this - it's only shown once!)
{
  "identity_id": 123,
  "name": "sensor:core.timer",
  "scope": "sensor",
  "token": "eyJhbGci...",  # Use this as ATTUNE_API_TOKEN
  "expires_at": "2025-01-30T12:34:56Z"  # 72 hours from now
}
```

**Important**: 
- The token is only displayed once. Store it securely!
- Sensor tokens expire after 24-72 hours and must be rotated
- Plan to rotate the token before expiration (set up monitoring/alerts)

## Timer Configuration

Rules using the `core.timer` trigger must provide configuration in `trigger_params`:

### Interval Timer

Fires every N units of time:

```json
{
  "type": "interval",
  "interval": 30,
  "unit": "seconds"  // "seconds", "minutes", "hours", "days"
}
```

Examples:
- Every 5 seconds: `{"type": "interval", "interval": 5, "unit": "seconds"}`
- Every 10 minutes: `{"type": "interval", "interval": 10, "unit": "minutes"}`
- Every 1 hour: `{"type": "interval", "interval": 1, "unit": "hours"}`
- Every 1 day: `{"type": "interval", "interval": 1, "unit": "days"}`

### DateTime Timer

Fires at a specific date/time (one-time):

```json
{
  "type": "date_time",
  "fire_at": "2025-01-27T15:00:00Z"
}
```

### Cron Timer (Planned)

Fires based on cron expression:

```json
{
  "type": "cron",
  "expression": "0 0 * * *"  // Daily at midnight
}
```

**Note**: Cron timers are not yet implemented.

## Running the Sensor

### Development

```bash
# Terminal 1: Start dependencies
docker-compose up -d postgres notifier

# Terminal 2: Start API
cd crates/api
cargo run

# Terminal 3: Start sensor
export ATTUNE_API_URL="http://localhost:8080"
export ATTUNE_API_TOKEN="your_sensor_token_here"
export ATTUNE_SENSOR_REF="core.timer"
export ATTUNE_NOTIFIER_WS_URL="ws://localhost:8081/ws"

cargo run --package core-timer-sensor
```

### Production (systemd)

Create a systemd service file at `/etc/systemd/system/attune-core-timer-sensor.service`:

```ini
[Unit]
Description=Attune Timer Sensor
After=network.target

[Service]
Type=simple
User=attune
WorkingDirectory=/opt/attune
ExecStart=/usr/local/bin/attune-core-timer-sensor
Restart=always
RestartSec=10

# Environment variables
Environment="ATTUNE_API_URL=https://attune.example.com"
Environment="ATTUNE_SENSOR_REF=core.timer"
Environment="ATTUNE_NOTIFIER_WS_URL=wss://notifier.example.com/ws"
Environment="ATTUNE_LOG_LEVEL=info"

# Load token from file
EnvironmentFile=/etc/attune/sensor-timer.env

# Security
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true

[Install]
WantedBy=multi-user.target
```

Create `/etc/attune/sensor-timer.env`:

```bash
ATTUNE_API_TOKEN=eyJhbGci...
```

Enable and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable attune-core-timer-sensor
sudo systemctl start attune-core-timer-sensor
sudo systemctl status attune-core-timer-sensor
```

**Token Rotation:**

Sensor tokens expire after 24-72 hours. To rotate:

```bash
# 1. Create new service account token (via API)
# 2. Update /etc/attune/sensor-timer.env with new token
sudo nano /etc/attune/sensor-timer.env

# 3. Restart sensor
sudo systemctl restart attune-core-timer-sensor
```

Set up a cron job or monitoring alert to remind you to rotate tokens every 72 hours.

View logs:

```bash
sudo journalctl -u attune-core-timer-sensor -f
```

## Monitoring

### Logs

The sensor outputs structured JSON logs:

```json
{
  "timestamp": "2025-01-27T12:34:56Z",
  "level": "info",
  "message": "Timer fired for rule 123, created event 456",
  "rule_id": 123,
  "event_id": 456
}
```

### Health Checks

The sensor verifies API connectivity on startup. Monitor the logs for:

- `"API connectivity verified"` - Sensor connected successfully
- `"Timer started for rule"` - Timer activated for a rule
- `"Timer fired for rule"` - Event created by timer
- `"Failed to create event"` - Event creation error (check token/permissions)

## Troubleshooting

### "Invalid sensor_ref: expected 'core.timer'"

The `ATTUNE_SENSOR_REF` must be exactly `core.timer`. This sensor only handles timer triggers.

### "Failed to connect to Attune API"

- Verify `ATTUNE_API_URL` is correct and reachable
- Check that the API service is running
- Ensure no firewall blocking the connection

### "Insufficient permissions to create event for trigger type 'core.timer'"

The service account token doesn't have permission to create timer events. Ensure the token's metadata includes `"trigger_types": ["core.timer"]`.

### "Failed to connect to notifier websocket"

- Verify `ATTUNE_NOTIFIER_WS_URL` is correct
- Check that the notifier service is running and reachable
- Ensure websocket authentication token is valid

### "Token expired"

The service account token has exceeded its TTL (24-72 hours). This is expected behavior.

**Solution:**
1. Create a new service account token via API
2. Update `ATTUNE_API_TOKEN` environment variable
3. Restart the sensor

**Prevention:**
- Set up monitoring to alert 6 hours before token expiration
- Plan regular token rotation (every 72 hours maximum)

### Timer not firing

1. Check that the rule is enabled
2. Verify the rule's `trigger_type` is `core.timer`
3. Check the sensor logs for "Timer started for rule"
4. Ensure `trigger_params` is valid JSON matching the timer config format

## Development

### Running Tests

```bash
cargo test --package core-timer-sensor
```

### Building

```bash
# Debug build
cargo build --package core-timer-sensor

# Release build
cargo build --release --package core-timer-sensor
```

### Code Structure

```
crates/core-timer-sensor/
├── src/
│   ├── main.rs           # Entry point, initialization
│   ├── config.rs         # Configuration loading (env/stdin)
│   ├── api_client.rs     # Attune API communication
│   ├── timer_manager.rs  # Per-rule timer task management
│   ├── rule_listener.rs  # Notifier websocket listener
│   └── types.rs          # Shared types and enums
├── Cargo.toml
└── README.md
```

## Contributing

When adding new timer types:

1. Add variant to `TimerConfig` enum in `types.rs`
2. Implement spawn logic in `timer_manager.rs`
3. Add tests for the new timer type
4. Update this README with examples

## License

MIT License - see LICENSE file for details.

## See Also

- [Sensor Interface Specification](../../docs/sensor-interface.md)
- [Service Accounts Documentation](../../docs/service-accounts.md)
- [Sensor Authentication Overview](../../docs/sensor-authentication-overview.md)
