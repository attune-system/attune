# Core Pack Actions

## Overview

All actions in the core pack are implemented as **pure POSIX shell scripts** with **zero external dependencies** (except `curl` for HTTP actions). This design ensures maximum portability and minimal runtime requirements.

**Key Principles:**
- **POSIX shell only** - No bash-specific features, works everywhere
- **DOTENV parameter format** - Simple key=value format, no JSON parsing needed
- **No jq/yq/Python/Node.js** - Core pack depends only on standard POSIX utilities
- **Stdin parameter delivery** - Secure, never exposed in process list
- **Explicit output formats** - text, json, or yaml

## Parameter Delivery Method

**All actions use stdin with DOTENV format:**
- Parameters read from **stdin** in `key=value` format
- Use `parameter_delivery: stdin` and `parameter_format: dotenv` in YAML
- Stdin is closed after delivery; scripts read until EOF
- **DO NOT** use environment variables for parameters

**Example DOTENV input:**
```
message="Hello World"
seconds=5
enabled=true
```

## Output Format

**All actions must specify an `output_format`:**
- `text` - Plain text output (stored as-is, no parsing)
- `json` - JSON structured data (parsed into JSONB field)
- `yaml` - YAML structured data (parsed into JSONB field)

**Output schema:**
- Only applicable for `json` and `yaml` formats
- Describes the structure of data written to stdout
- **Should NOT include** stdout/stderr/exit_code (captured automatically)

## Environment Variables

### Standard Environment Variables (Provided by Worker)

The worker automatically provides these environment variables to all action executions:

| Variable | Description | Always Present |
|----------|-------------|----------------|
| `ATTUNE_ACTION` | Action ref (e.g., `core.http_request`) | ✅ Yes |
| `ATTUNE_EXEC_ID` | Execution database ID | ✅ Yes |
| `ATTUNE_API_TOKEN` | Execution-scoped API token | ✅ Yes |
| `ATTUNE_RULE` | Rule ref that triggered execution | ❌ Only if from rule |
| `ATTUNE_TRIGGER` | Trigger ref that caused enforcement | ❌ Only if from trigger |

**Use cases:**
- Logging with execution context
- Calling Attune API (using `ATTUNE_API_TOKEN`)
- Conditional logic based on rule/trigger
- Creating child executions
- Accessing secrets via API

### Custom Environment Variables (Optional)

Custom environment variables can be set via `execution.env_vars` field for:
- **Debug/logging controls** (e.g., `DEBUG=1`, `LOG_LEVEL=debug`)
- **Runtime configuration** (e.g., custom paths, feature flags)

Environment variables should **NEVER** be used for:
- Action parameters (use stdin DOTENV instead)
- Secrets or credentials (use `ATTUNE_API_TOKEN` to fetch from key vault)
- User-provided data (use stdin parameters)

## Implementation Pattern

### POSIX Shell Actions (Standard Pattern)

All core pack actions follow this pattern:

```sh
#!/bin/sh
# Action Name - Core Pack
# Brief description
#
# This script uses pure POSIX shell without external dependencies like jq.
# It reads parameters in DOTENV format from stdin until EOF.

set -e

# Initialize variables with defaults
param1=""
param2="default_value"

# Read DOTENV-formatted parameters from stdin until EOF
while IFS= read -r line; do
    [ -z "$line" ] && continue

    key="${line%%=*}"
    value="${line#*=}"

    # Remove quotes if present
    case "$value" in
        \"*\") value="${value#\"}"; value="${value%\"}" ;;
        \'*\') value="${value#\'}"; value="${value%\'}" ;;
    esac

    # Process parameters
    case "$key" in
        param1) param1="$value" ;;
        param2) param2="$value" ;;
    esac
done

# Validate required parameters
if [ -z "$param1" ]; then
    echo "ERROR: param1 is required" >&2
    exit 1
fi

# Action logic
echo "Processing: $param1"

exit 0
```

### Boolean Normalization

```sh
case "$bool_param" in
    true|True|TRUE|yes|Yes|YES|1) bool_param="true" ;;
    *) bool_param="false" ;;
esac
```

### Numeric Validation

```sh
case "$number" in
    ''|*[!0-9]*)
        echo "ERROR: must be a number" >&2
        exit 1
        ;;
esac
```

## Core Pack Actions

### Simple Actions

1. **echo.sh** - Outputs a message (reference implementation)
2. **sleep.sh** - Pauses execution for a specified duration
3. **noop.sh** - Does nothing (useful for testing and placeholder workflows)
4. **run_agent_command.sh** - Launches an AI/agent harness with execution-scoped `attune-mcp` access

### HTTP Action

5. **http_request.sh** - Makes HTTP requests with full feature support:
   - Multiple HTTP methods (GET, POST, PUT, PATCH, DELETE, etc.)
   - Custom headers and query parameters
   - Authentication (basic, bearer token)
   - SSL verification control
   - Redirect following
   - JSON output with parsed response

### Pack Management Actions (API Wrappers)

These actions wrap Attune API endpoints for pack management:

6. **download_packs.sh** - Downloads packs from explicit Git/archive URLs; registry refs use atomic pack install
7. **build_pack_envs.sh** - Builds runtime environments for packs
8. **register_packs.sh** - Registers packs in the database
9. **get_pack_dependencies.sh** - Analyzes pack dependencies

All API wrappers:
- Accept parameters via DOTENV format
- Build JSON request bodies manually (no jq)
- Make authenticated API calls with curl
- Extract response data using simple sed patterns
- Return structured JSON output

### Agent Harness Actions

`run_agent_command.sh` is the reference bridge for AI-agent actions that need Attune MCP access from inside a workflow execution.

It:
- requires the worker-provided `ATTUNE_API_TOKEN` execution token
- exports `ATTUNE_MCP_COMMAND` (default `/opt/attune/agent/attune-mcp`)
- exports `ATTUNE_MCP_TRANSPORT=stdio`
- optionally creates `ATTUNE_AGENT_STATE_DIR` under the shared artifacts volume

This lets an agent harness spawn `attune-mcp` as a child process and ensure all tool calls run under the current execution identity rather than a shared service token.

## Testing Actions Locally

Test actions by echoing DOTENV format to stdin:

```bash
# Test echo action
printf 'message="Hello World"\n' | ./echo.sh

# Test with empty parameters
printf '' | ./echo.sh

# Test sleep action
printf 'seconds=2\nmessage="Sleeping..."\n' | ./sleep.sh

# Test http_request action
printf 'url="https://api.github.com"\nmethod="GET"\n' | ./http_request.sh

# Test with file input
cat params.dotenv | ./echo.sh
```

## YAML Configuration Example

```yaml
ref: core.example_action
label: "Example Action"
description: "Example action demonstrating DOTENV format"
enabled: true
runner_type: shell
entry_point: example.sh

# IMPORTANT: Use DOTENV format for POSIX shell compatibility
parameter_delivery: stdin
parameter_format: dotenv

# Output format: text, json, or yaml
output_format: text

parameters:
  type: object
  properties:
    message:
      type: string
      description: "Message to output"
      default: ""
    count:
      type: integer
      description: "Number of times to repeat"
      default: 1
  required:
    - message
```

## Dependencies

**Core pack has ZERO runtime dependencies:**

✅ **Required (universally available):**
- POSIX-compliant shell (`/bin/sh`)
- `curl` (for HTTP actions only)
- Standard POSIX utilities: `sed`, `mktemp`, `cat`, `printf`, `sleep`

❌ **NOT Required:**
- `jq` - Eliminated (was used for JSON parsing)
- `yq` - Never used
- Python - Not used in core pack actions
- Node.js - Not used in core pack actions
- bash - Scripts are POSIX-compliant
- Any other external tools or libraries

This makes the core pack **maximally portable** and suitable for minimal containers (Alpine, distroless, etc.).

## Security Benefits

1. **No process exposure** - Parameters never appear in `ps`, `/proc/<pid>/environ`
2. **Secure by default** - All actions use stdin, no special configuration needed
3. **Clear separation** - Action parameters vs. environment configuration
4. **Audit friendly** - All sensitive data flows through stdin, not environment
5. **Minimal attack surface** - No external dependencies to exploit

## Best Practices

### Parameters
1. **Always use stdin with DOTENV format** for action parameters
2. **Handle quoted values** - Remove both single and double quotes
3. **Provide sensible defaults** - Use empty string, 0, false as appropriate
4. **Validate required params** - Exit with error if truly required parameters missing
5. **Mark secrets** - Use `secret: true` in YAML for sensitive parameters
6. **Never use env vars for parameters** - Parameters come from stdin only

### Environment Variables
1. **Use standard ATTUNE_* variables** - Worker provides execution context
2. **Access API with ATTUNE_API_TOKEN** - Execution-scoped authentication
3. **Log with context** - Include `ATTUNE_ACTION` and `ATTUNE_EXEC_ID` in logs
4. **Never log ATTUNE_API_TOKEN** - Security sensitive
5. **Use env vars for runtime config only** - Not for user data or parameters

### Output Format
1. **Specify output_format** - Always set to "text", "json", or "yaml"
2. **Use text for simple output** - Messages, logs, unstructured data
3. **Use json for structured data** - API responses, complex results
4. **Define schema for structured output** - Only for json/yaml formats
5. **Use stderr for diagnostics** - Error messages go to stderr, not stdout
6. **Return proper exit codes** - 0 for success, non-zero for failure

### Shell Script Best Practices
1. **Use `#!/bin/sh`** - POSIX shell, not bash
2. **Use `set -e`** - Exit on error
3. **Quote all variables** - `"$var"` not `$var`
4. **Use `case` not `if`** - More portable for pattern matching
5. **Clean up temp files** - Use trap handlers
6. **Avoid bash-isms** - No `[[`, `${var^^}`, `=~`, arrays, etc.

## Execution Metadata (Automatic)

The following are **automatically captured** by the worker and should **NOT** be included in output schemas:

- `stdout` - Raw standard output (captured as-is)
- `stderr` - Standard error output (written to log file)
- `exit_code` - Process exit code (0 = success)
- `duration_ms` - Execution duration in milliseconds

These are execution system concerns, not action output concerns.

## Example: Complete Action

```sh
#!/bin/sh
# Example Action - Core Pack
# Demonstrates DOTENV parameter parsing and environment variable usage
#
# This script uses pure POSIX shell without external dependencies like jq.

set -e

# Log execution start
echo "[$ATTUNE_ACTION] [Exec: $ATTUNE_EXEC_ID] Starting" >&2

# Initialize variables
url=""
timeout="30"

# Read DOTENV parameters from stdin until EOF
while IFS= read -r line; do
    [ -z "$line" ] && continue

    key="${line%%=*}"
    value="${line#*=}"
    
    case "$value" in
        \"*\") value="${value#\"}"; value="${value%\"}" ;;
    esac

    case "$key" in
        url) url="$value" ;;
        timeout) timeout="$value" ;;
    esac
done

# Validate
if [ -z "$url" ]; then
    echo "ERROR: url is required" >&2
    exit 1
fi

# Execute
echo "Fetching: $url" >&2
result=$(curl -s --max-time "$timeout" "$url")

# Output
echo "$result"

echo "[$ATTUNE_ACTION] [Exec: $ATTUNE_EXEC_ID] Completed" >&2
exit 0
```

## Further Documentation

- **Pattern Reference:** `docs/QUICKREF-dotenv-shell-actions.md`
- **Pack Structure:** `docs/pack-structure.md`
- **Example Actions:**
  - `echo.sh` - Simplest reference implementation
  - `http_request.sh` - Complex action with full HTTP client
  - `register_packs.sh` - API wrapper with JSON construction
