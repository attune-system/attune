# Execute Action Modal: Environment Variables

**Feature:** Custom Environment Variables for Manual Executions  
**Added:** 2026-02-07  
**Location:** Actions Page → Execute Action Modal  

## Overview

The Execute Action modal now includes an "Environment Variables" section that allows users to specify optional runtime configuration for manual action executions. This is useful for debug flags, log levels, and other runtime settings.

## UI Components

### Modal Layout

```
┌──────────────────────────────────────────────────────────┐
│ Execute Action                                        X  │
├──────────────────────────────────────────────────────────┤
│                                                          │
│ Action: example.http_request                                │
│ Make an HTTP request to a specified URL                  │
│                                                          │
├──────────────────────────────────────────────────────────┤
│ Parameters                                               │
│ ┌────────────────────────────────────────────────────┐   │
│ │ URL *                                              │   │
│ │ https://api.example.com                            │   │
│ │                                                    │   │
│ │ Method                                             │   │
│ │ GET                                                │   │
│ └────────────────────────────────────────────────────┘   │
│                                                          │
├──────────────────────────────────────────────────────────┤
│ Environment Variables                                    │
│ Optional environment variables for this execution        │
│ (e.g., DEBUG, LOG_LEVEL)                                 │
│                                                          │
│ ┌──────────────────────┬──────────────────────┬────┐    │
│ │ Key                  │ Value                │    │    │
│ ├──────────────────────┼──────────────────────┼────┤    │
│ │ DEBUG                │ true                 │ X  │    │
│ ├──────────────────────┼──────────────────────┼────┤    │
│ │ LOG_LEVEL            │ debug                │ X  │    │
│ ├──────────────────────┼──────────────────────┼────┤    │
│ │ TIMEOUT_SECONDS      │ 30                   │ X  │    │
│ └──────────────────────┴──────────────────────┴────┘    │
│                                                          │
│ + Add Environment Variable                               │
│                                                          │
├──────────────────────────────────────────────────────────┤
│                                    [Cancel]  [Execute]   │
└──────────────────────────────────────────────────────────┘
```

## Features

### Dynamic Key-Value Rows

Each environment variable is entered as a key-value pair on a separate row:

- **Key Input:** Text field for the environment variable name (e.g., `DEBUG`, `LOG_LEVEL`)
- **Value Input:** Text field for the environment variable value (e.g., `true`, `debug`)
- **Remove Button:** X icon to remove the row (disabled when only one row remains)

### Add/Remove Functionality

- **Add:** Click "+ Add Environment Variable" to add a new empty row
- **Remove:** Click the X button on any row to remove it
- **Minimum:** At least one row is always present (remove button disabled on last row)
- **Empty Rows:** Rows with blank keys are filtered out when submitting

### Validation

- No built-in validation (flexible for debugging)
- Empty key rows are ignored
- Key-value pairs are sent as-is to the API

## Use Cases

### 1. Debug Mode
```
Key: DEBUG
Value: true
```
Action script can check `if [ "$DEBUG" = "true" ]; then set -x; fi`

### 2. Custom Log Level
```
Key: LOG_LEVEL
Value: debug
```
Action script can use `LOG_LEVEL="${LOG_LEVEL:-info}"`

### 3. Timeout Override
```
Key: TIMEOUT_SECONDS
Value: 30
```
Action script can use `TIMEOUT="${TIMEOUT_SECONDS:-60}"`

### 4. Feature Flags
```
Key: ENABLE_EXPERIMENTAL
Value: true
```
Action script can conditionally enable features

### 5. Retry Configuration
```
Key: MAX_RETRIES
Value: 5
```
Action script can adjust retry behavior

## Important Distinctions

### ❌ NOT for Sensitive Data
- Environment variables are stored in the database
- They appear in execution logs
- Use action parameters with `secret: true` for passwords/API keys

### ❌ NOT for Action Parameters
- Action parameters go via stdin as JSON
- Environment variables are for runtime configuration only
- Don't duplicate action parameters here

### ✅ FOR Runtime Configuration
- Debug flags and feature toggles
- Log levels and verbosity settings
- Timeout and retry overrides
- Non-sensitive execution metadata

## Example Workflow

### Step 1: Open Execute Modal
1. Navigate to Actions page
2. Find desired action
3. Click "Execute" button

### Step 2: Fill Parameters
Fill in required and optional action parameters as usual.

### Step 3: Add Environment Variables
1. Scroll to "Environment Variables" section
2. Enter first env var (e.g., `DEBUG` = `true`)
3. Click "+ Add Environment Variable" to add more rows
4. Enter additional env vars (e.g., `LOG_LEVEL` = `debug`)
5. Click X to remove any unwanted rows

### Step 4: Execute
Click "Execute" button. The execution will have:
- Action parameters delivered via stdin (JSON)
- Environment variables set in the process environment
- Standard Attune env vars (`ATTUNE_ACTION`, `ATTUNE_EXEC_ID`, etc.)

## API Request Example

When you click Execute with environment variables, the UI sends:

```json
POST /api/v1/executions/execute
{
  "action_ref": "example.http_request",
  "parameters": {
    "url": "https://api.example.com",
    "method": "GET"
  },
  "env_vars": {
    "DEBUG": "true",
    "LOG_LEVEL": "debug",
    "TIMEOUT_SECONDS": "30"
  }
}
```

## Action Script Usage

In your action script, environment variables are available as standard environment variables:

```bash
#!/bin/bash

# Check custom env vars
if [ "$DEBUG" = "true" ]; then
    set -x  # Enable debug mode
    echo "Debug mode enabled" >&2
fi

# Use custom log level
LOG_LEVEL="${LOG_LEVEL:-info}"
echo "Log level: $LOG_LEVEL" >&2

# Apply custom timeout
TIMEOUT="${TIMEOUT_SECONDS:-60}"
echo "Using timeout: ${TIMEOUT}s" >&2

# Read action parameters from stdin
INPUT=$(cat)
URL=$(echo "$INPUT" | jq -r '.url')

# Execute action logic
curl --max-time "$TIMEOUT" "$URL"
```

## Tips & Best Practices

### 1. Use Uppercase for Keys
Follow Unix convention: `DEBUG`, `LOG_LEVEL`, not `debug`, `log_level`

### 2. Provide Defaults in Scripts
```bash
DEBUG="${DEBUG:-false}"
LOG_LEVEL="${LOG_LEVEL:-info}"
```

### 3. Document Common Env Vars
Add comments in your action YAML:
```yaml
# Supports environment variables:
# - DEBUG: Enable debug mode (true/false)
# - LOG_LEVEL: Logging verbosity (debug/info/warn/error)
# - TIMEOUT_SECONDS: Request timeout in seconds
```

### 4. Don't Duplicate Parameters
If an action has a `timeout` parameter, use that instead of `TIMEOUT_SECONDS` env var.

### 5. Test Locally First
Test with env vars set locally before using in production:
```bash
DEBUG=true LOG_LEVEL=debug ./my_action.sh < params.json
```

## Related Documentation

- [QUICKREF: Execution Environment](../QUICKREF-execution-environment.md) - All environment variables
- [QUICKREF: Action Parameters](../QUICKREF-action-parameters.md) - Parameter delivery via stdin
- [Action Development Guide](../packs/pack-structure.md) - Writing actions

## See Also

- Execution detail page (shows env vars used)
- Workflow inheritance (child executions inherit env vars)
- Rule-triggered executions (no custom env vars)