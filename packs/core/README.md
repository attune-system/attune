# Attune Core Pack

Core 1.0.4 includes the canonical two-file `core.install_packs` workflow.
See [pack installation](workflows/PACK_INSTALLATION.md) for inputs, API authority,
candidate-test ownership and the distinction between runtime detection and
worker environment setup. The installer uses real shipped actions and does not
ship a separate queue.

The **Core Pack** is the foundational system pack for Attune, providing essential automation components including timer triggers, HTTP utilities, and basic shell actions.

## Overview

The core pack is automatically installed with Attune and provides the building blocks for creating automation workflows. It includes:

- **Timer Triggers**: Interval-based, cron-based, and one-shot datetime timers
- **HTTP Actions**: Make HTTP requests to external APIs
- **Shell Actions**: Execute basic shell commands (echo, sleep, noop)
- **Agent Actions**: Launch AI/agent harnesses with execution-scoped MCP access
- **Built-in Sensors**: System sensors for monitoring time-based events

## Components

### Actions

#### `core.echo`
Outputs a message to stdout.

**Parameters:**
- `message` (string, required): Message to echo
- `uppercase` (boolean, optional): Convert message to uppercase

**Example:**
```yaml
action: core.echo
parameters:
  message: "Hello, Attune!"
  uppercase: false
```

---

#### `core.sleep`
Pauses execution for a specified duration.

**Parameters:**
- `seconds` (integer, required): Number of seconds to sleep (0-3600)
- `message` (string, optional): Optional message to display before sleeping

**Example:**
```yaml
action: core.sleep
parameters:
  seconds: 30
  message: "Waiting 30 seconds..."
```

---

#### `core.noop`
Does nothing - useful for testing and placeholder workflows.

**Parameters:**
- `message` (string, optional): Optional message to log
- `exit_code` (integer, optional): Exit code to return (default: 0)

**Example:**
```yaml
action: core.noop
parameters:
  message: "Testing workflow structure"
```

---

#### `core.http_request`
Make HTTP requests to external APIs with full control over headers, authentication, and request body.

**Parameters:**
- `url` (string, required): URL to send the request to
- `method` (string, optional): HTTP method (GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS)
- `headers` (object, optional): HTTP headers as key-value pairs
- `body` (string, optional): Request body for POST/PUT/PATCH
- `json_body` (object, optional): JSON request body (alternative to `body`)
- `query_params` (object, optional): URL query parameters
- `timeout` (integer, optional): Request timeout in seconds (default: 30)
- `verify_ssl` (boolean, optional): Verify SSL certificates (default: true)
- `auth_type` (string, optional): Authentication type (none, basic, bearer)
- `auth_username` (string, optional): Username for basic auth
- `auth_password` (string, secret, optional): Password for basic auth
- `auth_token` (string, secret, optional): Bearer token
- `follow_redirects` (boolean, optional): Follow HTTP redirects (default: true)
- `max_redirects` (integer, optional): Maximum redirects to follow (default: 10)

**Output:**
- `status_code` (integer): HTTP status code
- `headers` (object): Response headers
- `body` (string): Response body as text
- `json` (object): Parsed JSON response (if applicable)
- `elapsed_ms` (integer): Request duration in milliseconds
- `url` (string): Final URL after redirects
- `success` (boolean): Whether request was successful (2xx status)

**Example:**
```yaml
action: core.http_request
parameters:
  url: "https://api.example.com/users"
  method: "POST"
  json_body:
    name: "John Doe"
    email: "john@example.com"
  headers:
    Content-Type: "application/json"
  auth_type: "bearer"
  auth_token: "${secret:api_token}"
```

---

#### `core.run_agent_command`
Run an AI/agent harness command with execution-scoped Attune MCP access.

This action is designed for workflow tasks that launch agent-capable tools inside an Attune execution. The worker already provides `ATTUNE_API_TOKEN` and `ATTUNE_API_URL`; this action exports `ATTUNE_MCP_COMMAND` pointing at the local `attune-mcp` binary so the harness can spawn MCP as a child process and all tool calls stay bound to the current execution identity.

**Parameters:**
- `command` (string, required): Shell command to run for the agent harness
- `working_dir` (string, optional): Working directory before launch
- `mcp_command` (string, optional): Path to the MCP binary (default: `/opt/attune/agent/attune-mcp`)
- `state_dir` (string, optional): Directory for transcript/state files
- `require_mcp_binary` (boolean, optional): Fail if the MCP binary is unavailable

**Example:**
```yaml
action: core.run_agent_command
parameters:
  command: "my-agent-harness --mcp-command \"$ATTUNE_MCP_COMMAND\""
```

---

### Triggers

#### `core.intervaltimer`
Fires at regular intervals based on time unit and interval.

**Parameters:**
- `unit` (string, required): Time unit (seconds, minutes, hours)
- `interval` (integer, required): Number of time units between triggers

**Payload:**
- `type`: "interval"
- `interval_seconds`: Total interval in seconds
- `fired_at`: ISO 8601 timestamp
- `execution_count`: Number of times fired
- `sensor_ref`: Reference to the sensor

**Example:**
```yaml
trigger: core.intervaltimer
config:
  unit: "minutes"
  interval: 5
```

---

#### `core.crontimer`
Fires based on cron schedule expressions.

**Parameters:**
- `expression` (string, required): Cron expression (6 fields: second minute hour day month weekday)
- `timezone` (string, optional): Timezone (default: UTC)
- `description` (string, optional): Human-readable schedule description

**Payload:**
- `type`: "cron"
- `fired_at`: ISO 8601 timestamp
- `scheduled_at`: When trigger was scheduled to fire
- `expression`: The cron expression
- `timezone`: Timezone used
- `next_fire_at`: Next scheduled fire time
- `execution_count`: Number of times fired
- `sensor_ref`: Reference to the sensor

**Cron Format:**
```
┌───────── second (0-59)
│ ┌─────── minute (0-59)
│ │ ┌───── hour (0-23)
│ │ │ ┌─── day of month (1-31)
│ │ │ │ ┌─ month (1-12)
│ │ │ │ │ ┌ day of week (0-6, 0=Sunday)
│ │ │ │ │ │
* * * * * *
```

**Examples:**
- `0 0 * * * *` - Every hour
- `0 0 0 * * *` - Every day at midnight
- `0 */15 * * * *` - Every 15 minutes
- `0 30 8 * * 1-5` - 8:30 AM on weekdays

---

#### `core.datetimetimer`
Fires once at a specific date and time.

**Parameters:**
- `fire_at` (string, required): ISO 8601 timestamp when timer should fire
- `timezone` (string, optional): Timezone (default: UTC)
- `description` (string, optional): Human-readable description

**Payload:**
- `type`: "one_shot"
- `fire_at`: Scheduled fire time
- `fired_at`: Actual fire time
- `timezone`: Timezone used
- `delay_ms`: Delay between scheduled and actual fire time
- `sensor_ref`: Reference to the sensor

**Example:**
```yaml
trigger: core.datetimetimer
config:
  fire_at: "2024-12-31T23:59:59Z"
  description: "New Year's countdown"
```

---

### Sensors

#### `core.interval_timer_sensor`
Built-in sensor that monitors time and fires interval timer triggers.

**Configuration:**
- `check_interval_seconds` (integer, optional): How often to check triggers (default: 1)

This sensor automatically runs as part of the Attune sensor service and manages all interval timer trigger instances.

---

## Configuration

The core pack supports the following configuration options:

```yaml
# config.yaml
packs:
  core:
    max_action_timeout: 300        # Maximum action timeout in seconds
    enable_debug_logging: false    # Enable debug logging
```

## Dependencies

### Python Dependencies
- `requests>=2.28.0` - For HTTP request action
- `croniter>=1.4.0` - For cron timer parsing (future)

### Runtime Dependencies
- Shell (bash/sh) - For shell-based actions
- Python 3.8+ - For Python-based actions and sensors

## Installation

The core pack is automatically installed with Attune. No manual installation is required.

To verify the core pack is loaded:

```bash
# Using CLI
attune pack list | grep core

# Using API
curl http://localhost:8080/api/v1/packs/core
```

## Usage Examples

### Example 1: Echo Every 10 Seconds

Create a rule that echoes "Hello, World!" every 10 seconds:

```yaml
ref: core.hello_world_rule
trigger: core.intervaltimer
trigger_config:
  unit: "seconds"
  interval: 10
action: core.echo
action_params:
  message: "Hello, World!"
  uppercase: false
```

### Example 2: HTTP Health Check Every 5 Minutes

Monitor an API endpoint every 5 minutes:

```yaml
ref: core.health_check_rule
trigger: core.intervaltimer
trigger_config:
  unit: "minutes"
  interval: 5
action: core.http_request
action_params:
  url: "https://api.example.com/health"
  method: "GET"
  timeout: 10
```

### Example 3: Daily Report at Midnight

Generate a report every day at midnight:

```yaml
ref: core.daily_report_rule
trigger: core.crontimer
trigger_config:
  expression: "0 0 0 * * *"
  timezone: "UTC"
  description: "Daily at midnight"
action: core.http_request
action_params:
  url: "https://api.example.com/reports/generate"
  method: "POST"
```

### Example 4: One-Time Reminder

Set a one-time reminder for a specific date and time:

```yaml
ref: core.meeting_reminder
trigger: core.datetimetimer
trigger_config:
  fire_at: "2024-06-15T14:00:00Z"
  description: "Team meeting reminder"
action: core.echo
action_params:
  message: "Team meeting starts in 15 minutes!"
```

## Development

### Adding New Actions

1. Create action metadata file: `actions/<action_name>.yaml`
2. Create action implementation: `actions/<action_name>.sh` or `actions/<action_name>.py`
3. Make script executable: `chmod +x actions/<action_name>.sh`
4. Update pack manifest if needed
5. Test the action

### Testing Actions Locally

Test actions directly by setting environment variables:

```bash
# Test echo action
export ATTUNE_ACTION_MESSAGE="Test message"
export ATTUNE_ACTION_UPPERCASE=true
./actions/echo.sh

# Test HTTP request action
export ATTUNE_ACTION_URL="https://httpbin.org/get"
export ATTUNE_ACTION_METHOD="GET"
python3 actions/http_request.py
```

## Contributing

The core pack is part of the Attune project. Contributions are welcome!

1. Follow the existing code style and structure
2. Add tests for new actions/sensors
3. Update documentation
4. Submit a pull request

## License

The core pack is licensed under the same license as Attune.

## Support

- Documentation: https://docs.attune.io/packs/core
- Issues: https://github.com/attune-io/attune/issues
- Discussions: https://github.com/attune-io/attune/discussions
