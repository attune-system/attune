# Quick Reference: Execution Environment Variables

**Last Updated:** 2026-07-21
**Status:** Standard for all action executions

## Overview

The worker automatically provides standard environment variables to all action executions. These variables provide context about the execution and enable actions to interact with the Attune API.

## Standard Environment Variables

All actions receive the following environment variables:

| Variable | Type | Description | Always Present |
|----------|------|-------------|----------------|
| `ATTUNE_ACTION` | string | Action ref (e.g., `core.http_request`) | ✅ Yes |
| `ATTUNE_EXEC_ID` | integer | Execution database ID | ✅ Yes |
| `ATTUNE_API_URL` | string | Base URL for demand-driven API access | ✅ Yes |
| `ATTUNE_API_TOKEN` | string | Execution-scoped API token | ❌ Only with non-empty `permission_set_refs` |
| `ATTUNE_PACK_REF` | string | Pack containing the executing action | ✅ Yes |
| `ATTUNE_TRACE_TAG` | string | Execution trace/correlation tag | ❌ Only when trace tag is set |
| `ATTUNE_RULE` | string | Rule ref that triggered execution | ❌ Only if from rule |
| `ATTUNE_TRIGGER` | string | Trigger ref that caused enforcement | ❌ Only if from trigger |

### ATTUNE_ACTION

**Purpose:** Identifies which action is being executed.

**Format:** `{pack_ref}.{action_name}`

**Examples:**
```bash
ATTUNE_ACTION="core.http_request"
ATTUNE_ACTION="core.echo"
ATTUNE_ACTION="slack.post_message"
ATTUNE_ACTION="aws.ec2.describe_instances"
```

**Use Cases:**
- Logging and telemetry
- Conditional behavior based on action
- Error reporting with context

**Example Usage:**
```bash
#!/bin/bash
echo "Executing action: $ATTUNE_ACTION" >&2
# Perform action logic...
echo "Action $ATTUNE_ACTION completed successfully" >&2
```

### ATTUNE_EXEC_ID

**Purpose:** Unique identifier for this execution instance.

**Format:** Integer (database ID)

**Examples:**
```bash
ATTUNE_EXEC_ID="12345"
ATTUNE_EXEC_ID="67890"
```

**Use Cases:**
- Correlate logs with execution records
- Report progress back to API
- Create child executions (workflows)
- Generate unique temporary file names

**Example Usage:**
```bash
#!/bin/bash
# Create execution-specific temp file
TEMP_FILE="/tmp/attune-exec-${ATTUNE_EXEC_ID}.tmp"

# Log with execution context
echo "[Execution $ATTUNE_EXEC_ID] Processing request..." >&2

# Report progress to API
curl -s -X PATCH \
    -H "Authorization: Bearer $ATTUNE_API_TOKEN" \
    "$ATTUNE_API_URL/api/v1/executions/$ATTUNE_EXEC_ID" \
    -d '{"status": "running"}'
```

### ATTUNE_API_TOKEN

**Purpose:** Execution-scoped bearer token for authenticating with Attune API.

**Format:** JWT token string

**Security:**
- ✅ Scoped to this execution
- ✅ Limited lifetime (expires with execution)
- ✅ Effective permissions come only from the execution's signed permission-set snapshot
- ✅ Cache `standard` access is read-only for the executing action/pack and containing workflow refs
- ❌ Cannot access other executions
- ❌ Cannot modify system configuration

The worker deliberately omits this variable when `permission_set_refs` is
empty. API clients must return an explicit configuration/authentication
error in that case; they must not fall back to unauthenticated requests or
direct database access.

`ATTUNE_*` names are worker-owned. Runtime environment definitions cannot
replace them, and the worker explicitly removes any parent-process
`ATTUNE_API_TOKEN` before launching an action unless it minted a token for that
execution.

Cache data is fetched on demand through the cache API. It is never inserted
into ambient action parameters or the key-backed secret stdin channel.
Refresh/upload/seal/promotion operations require a named explicit cache write
grant; `standard` does not grant cache writes.

A workflow task is a child execution and uses its own execution-scoped token.
Cache `standard` access includes the executing task action/pack and the signed
containing workflow action/pack refs; it does not provide ambient access to
other workflow data or namespaces. Data Caches are reconstructable snapshots,
not authoritative business storage, a general query database, or a secret
store.

Execution-token TTL is the action execution timeout plus a short worker grace
period, and tokens cannot be refreshed mid-execution. Configure a timeout that
exceeds any page-at-a-time scan. Prefer bounded multi-ID lookup, or a
server-side streaming response when available, for large datasets that may
otherwise outlive the token.

**Use Cases:**
- Query execution status
- Retrieve execution parameters
- Create child executions (sub-workflows)
- Report progress or intermediate results
- Access secrets via API

**Example Usage:**
```bash
#!/bin/bash
# Query execution details
curl -s -H "Authorization: Bearer $ATTUNE_API_TOKEN" \
    "$ATTUNE_API_URL/api/v1/executions/$ATTUNE_EXEC_ID"

# Create child execution
curl -s -X POST \
    -H "Authorization: Bearer $ATTUNE_API_TOKEN" \
    -H "Content-Type: application/json" \
    "$ATTUNE_API_URL/api/v1/executions" \
    -d '{
        "action_ref": "core.echo",
        "parameters": {"message": "Child execution"},
        "parent_id": '"$ATTUNE_EXEC_ID"'
    }'

# Retrieve secret from key vault
SECRET=$(curl -s \
    -H "Authorization: Bearer $ATTUNE_API_TOKEN" \
    "$ATTUNE_API_URL/api/v1/keys/my-secret" | jq -r '.value')
```

### ATTUNE_TRACE_TAG

**Purpose:** Correlates automatically-created executions across rules, queues, workflows, and nested execution-token calls.

**Examples:**
```bash
ATTUNE_TRACE_TAG="core.timer.1234"
ATTUNE_TRACE_TAG="core.inbox.9876"
```

### ATTUNE_RULE

**Purpose:** Identifies the rule that triggered this execution (if applicable).

**Format:** `{pack_ref}.{rule_name}`

**Present:** Only when execution was triggered by a rule enforcement.

**Examples:**
```bash
ATTUNE_RULE="core.timer_to_echo"
ATTUNE_RULE="monitoring.disk_space_alert"
ATTUNE_RULE="ci.deploy_on_push"
```

**Use Cases:**
- Conditional logic based on triggering rule
- Logging rule context
- Different behavior for manual vs automated executions

**Example Usage:**
```bash
#!/bin/bash
if [ -n "$ATTUNE_RULE" ]; then
    echo "Triggered by rule: $ATTUNE_RULE" >&2
    # Rule-specific logic
else
    echo "Manual execution (no rule)" >&2
    # Manual execution logic
fi
```

## Custom Environment Variables

**Purpose:** Optional user-provided environment variables for manual executions.

**Set Via:** Web UI or API when creating manual executions.

**Format:** Key-value pairs (string → string mapping)

**Use Cases:**
- Debug flags (e.g., `DEBUG=true`)
- Log levels (e.g., `LOG_LEVEL=debug`)
- Runtime configuration (e.g., `MAX_RETRIES=5`)
- Feature flags (e.g., `ENABLE_EXPERIMENTAL=true`)

**Important Distinctions:**
- ❌ **NOT for sensitive data** - Use action parameters marked as `secret: true` instead
- ❌ **NOT for action parameters** - Use stdin JSON for actual action inputs
- ✅ **FOR runtime configuration** - Debug settings, feature flags, etc.
- ✅ **FOR execution context** - Additional metadata about how to run

**Example via API:**
```bash
curl -X POST http://localhost:8080/api/v1/executions/execute \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "action_ref": "core.http_request",
    "parameters": {
      "url": "https://api.example.com",
      "method": "GET"
    },
    "env_vars": {
      "DEBUG": "true",
      "LOG_LEVEL": "debug",
      "TIMEOUT_SECONDS": "30"
    }
  }'
```

**Example via Web UI:**
In the Execute Action modal, the "Environment Variables" section allows adding multiple key-value pairs for custom environment variables.

**Action Script Usage:**
```bash
#!/bin/bash
# Custom env vars are available as standard environment variables
if [ "$DEBUG" = "true" ]; then
    set -x  # Enable bash debug mode
    echo "Debug mode enabled" >&2
fi

# Use custom log level
LOG_LEVEL="${LOG_LEVEL:-info}"
echo "Using log level: $LOG_LEVEL" >&2

# Apply custom timeout
TIMEOUT="${TIMEOUT_SECONDS:-60}"
echo "Timeout set to: ${TIMEOUT}s" >&2

# ... action logic with custom configuration ...
```

**Security Note:**
Custom environment variables are stored in the database and logged. Never use them for:
- Passwords or API keys (use secrets API + `secret: true` parameters)
- Personally identifiable information (PII)
- Any sensitive data

For sensitive data, use action parameters marked with `secret: true` in the action YAML.

### ATTUNE_TRIGGER

**Purpose:** Identifies the trigger type that caused the rule enforcement (if applicable).

**Format:** `{pack_ref}.{trigger_name}`

**Present:** Only when execution was triggered by an event/trigger.

**Examples:**
```bash
ATTUNE_TRIGGER="core.intervaltimer"
ATTUNE_TRIGGER="core.webhook"
ATTUNE_TRIGGER="github.push"
ATTUNE_TRIGGER="aws.ec2.instance_state_change"
```

**Use Cases:**
- Different behavior based on trigger type
- Event-specific processing
- Logging event context

**Example Usage:**
```bash
#!/bin/bash
case "$ATTUNE_TRIGGER" in
    core.intervaltimer)
        echo "Scheduled execution" >&2
        ;;
    core.webhook)
        echo "Webhook-triggered execution" >&2
        ;;
    *)
        echo "Unknown or manual trigger" >&2
        ;;
esac
```

## Environment Variable Precedence

Environment variables are set in the following order (later overrides earlier):

1. **System defaults** - `PATH`, `HOME`, `USER`, etc.
2. **Standard Attune variables** - `ATTUNE_ACTION`, `ATTUNE_EXEC_ID`, etc. (always present)
3. **Custom environment variables** - User-provided via API/UI (optional)

**Note:** Custom env vars cannot override standard Attune variables or critical system variables.

## Additional Standard Variables

The worker also provides standard system environment variables:

| Variable | Description |
|----------|-------------|
| `PATH` | Standard PATH with Attune utilities |
| `HOME` | Home directory for execution |
| `USER` | Execution user (typically `attune`) |
| `PWD` | Working directory |
| `TMPDIR` | Temporary directory path |

## API Base URL

The API URL is typically available via configuration or a standard environment variable:

| Variable | Description | Example |
|----------|-------------|---------|
| `ATTUNE_API_URL` | Base URL for Attune API | `http://localhost:8080` |

## Usage Patterns

### Pattern 1: Logging with Context

```bash
#!/bin/bash
log() {
    local level="$1"
    shift
    echo "[${level}] [Action: $ATTUNE_ACTION] [Exec: $ATTUNE_EXEC_ID] $*" >&2
}

log INFO "Starting execution"
log DEBUG "Parameters: $INPUT"
# ... action logic ...
log INFO "Execution completed"
```

### Pattern 2: API Interaction

```bash
#!/bin/bash
# Function to call Attune API
attune_api() {
    local method="$1"
    local endpoint="$2"
    shift 2
    
    curl -s -X "$method" \
        -H "Authorization: Bearer $ATTUNE_API_TOKEN" \
        -H "Content-Type: application/json" \
        "$ATTUNE_API_URL/api/v1/$endpoint" \
        "$@"
}

# Query execution
EXEC_INFO=$(attune_api GET "executions/$ATTUNE_EXEC_ID")

# Create child execution
CHILD_EXEC=$(attune_api POST "executions" -d '{
    "action_ref": "core.echo",
    "parameters": {"message": "Child"},
    "parent_id": '"$ATTUNE_EXEC_ID"'
}')
```

### Pattern 3: Conditional Behavior

```bash
#!/bin/bash
# Behave differently for manual vs automated executions
if [ -n "$ATTUNE_RULE" ]; then
    # Automated execution (from rule)
    echo "Automated execution via rule: $ATTUNE_RULE" >&2
    NOTIFICATION_CHANNEL="automated"
else
    # Manual execution
    echo "Manual execution" >&2
    NOTIFICATION_CHANNEL="manual"
fi

# Different behavior based on trigger
if [ "$ATTUNE_TRIGGER" = "core.webhook" ]; then
    echo "Processing webhook payload..." >&2
elif [ "$ATTUNE_TRIGGER" = "core.intervaltimer" ]; then
    echo "Processing scheduled task..." >&2
fi
```

### Pattern 4: Temporary Files

```bash
#!/bin/bash
# Create execution-specific temp files
WORK_DIR="/tmp/attune-exec-${ATTUNE_EXEC_ID}"
mkdir -p "$WORK_DIR"

# Use temp directory
echo "Working in: $WORK_DIR" >&2
cp input.json "$WORK_DIR/input.json"

# Process files
process_data "$WORK_DIR/input.json" > "$WORK_DIR/output.json"

# Output result
cat "$WORK_DIR/output.json"

# Cleanup
rm -rf "$WORK_DIR"
```

### Pattern 5: Progress Reporting

```bash
#!/bin/bash
report_progress() {
    local message="$1"
    local percent="$2"
    
    echo "$message" >&2
    
    # Optional: Report to API (if endpoint exists)
    curl -s -X PATCH \
        -H "Authorization: Bearer $ATTUNE_API_TOKEN" \
        -H "Content-Type: application/json" \
        "$ATTUNE_API_URL/api/v1/executions/$ATTUNE_EXEC_ID" \
        -d "{\"progress\": $percent, \"message\": \"$message\"}" \
        > /dev/null 2>&1 || true
}

report_progress "Starting download" 0
# ... download ...
report_progress "Processing data" 50
# ... process ...
report_progress "Uploading results" 90
# ... upload ...
report_progress "Completed" 100
```

## Security Considerations

### Token Scope

The `ATTUNE_API_TOKEN` is scoped to the execution:
- ✅ Can read own execution data
- ✅ Can create child executions
- ✅ Can access secrets owned by execution identity
- ❌ Cannot read other executions
- ❌ Cannot modify system configuration
- ❌ Cannot delete resources

### Token Lifetime

- Token is valid for the duration of the execution
- Token expires when execution completes
- Token is invalidated if execution is cancelled
- Do not cache or persist the token

### Best Practices

1. **Never log the API token:**
   ```bash
   # ❌ BAD
   echo "Token: $ATTUNE_API_TOKEN" >&2
   
   # ✅ GOOD
   echo "Using API token for authentication" >&2
   ```

2. **Validate token presence:**
   ```bash
   if [ -z "$ATTUNE_API_TOKEN" ]; then
       echo "ERROR: ATTUNE_API_TOKEN not set" >&2
       exit 1
   fi
   ```

3. **Use HTTPS in production:**
   ```bash
   # Check API URL uses HTTPS
   if [[ ! "$ATTUNE_API_URL" =~ ^https:// ]] && [ "$ENVIRONMENT" = "production" ]; then
       echo "WARNING: API URL should use HTTPS in production" >&2
   fi
   ```

## Distinction: Environment Variables vs Parameters

### Standard Environment Variables
- **Purpose:** Execution context and metadata
- **Source:** System-provided automatically
- **Examples:** `ATTUNE_ACTION`, `ATTUNE_EXEC_ID`, `ATTUNE_API_TOKEN`
- **Access:** Standard environment variable access
- **Used for:** Logging, API access, execution identity

### Custom Environment Variables
- **Purpose:** Runtime configuration and debug settings
- **Source:** User-provided via API/UI (optional)
- **Examples:** `DEBUG=true`, `LOG_LEVEL=debug`, `MAX_RETRIES=5`
- **Access:** Standard environment variable access
- **Used for:** Debug flags, feature toggles, non-sensitive runtime config

### Action Parameters
- **Purpose:** Action-specific input data
- **Source:** User-provided via API/UI (required/optional per action)
- **Examples:** `{"url": "...", "method": "POST", "data": {...}}`
- **Access:** Read from stdin as JSON
- **Used for:** Action-specific configuration and data

**Example:**
```bash
#!/bin/bash
# Standard environment variables - system context (always present)
echo "Action: $ATTUNE_ACTION" >&2
echo "Execution ID: $ATTUNE_EXEC_ID" >&2

# Custom environment variables - runtime config (optional)
DEBUG="${DEBUG:-false}"
LOG_LEVEL="${LOG_LEVEL:-info}"
if [ "$DEBUG" = "true" ]; then
    set -x
fi

# Action parameters - user data (from stdin)
INPUT=$(cat)
URL=$(echo "$INPUT" | jq -r '.url')
METHOD=$(echo "$INPUT" | jq -r '.method // "GET"')

# Use all three together
curl -s -X "$METHOD" \
    -H "X-Attune-Action: $ATTUNE_ACTION" \
    -H "X-Attune-Exec-Id: $ATTUNE_EXEC_ID" \
    -H "X-Debug-Mode: $DEBUG" \
    "$URL"
```

## Testing Locally

When testing actions locally, you can simulate these environment variables:

```bash
#!/bin/bash
# test-action.sh - Local testing script

export ATTUNE_ACTION="core.http_request"
export ATTUNE_EXEC_ID="99999"
export ATTUNE_API_TOKEN="test-token-local"
export ATTUNE_RULE="test.rule"
export ATTUNE_TRIGGER="test.trigger"
export ATTUNE_API_URL="http://localhost:8080"

# Simulate custom env vars
export DEBUG="true"
export LOG_LEVEL="debug"

echo '{"url": "https://httpbin.org/get"}' | ./http_request.sh
```

## References

- [Action Parameter Handling](./QUICKREF-action-parameters.md) - Stdin-based parameter delivery
- [Action Output Format](./QUICKREF-action-output-format.md) - Output format and schemas
- [Worker Service Architecture](./architecture/worker-service.md) - How workers execute actions
- [Core Pack Actions](../packs/core/actions/README.md) - Reference implementations

## See Also

- API authentication documentation
- Execution lifecycle documentation
- Secret management and key vault access
- Workflow and child execution patterns
