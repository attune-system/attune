# Attune CLI

The Attune CLI is a comprehensive command-line tool for interacting with the Attune automation platform. It provides an intuitive interface for managing all aspects of the platform including packs, actions, rules, executions, and more.

## Overview

The CLI is designed to be:
- **Intuitive**: Natural command structure with helpful prompts
- **Flexible**: Multiple output formats for human and machine consumption
- **Powerful**: Full access to all API functionality
- **Scriptable**: JSON/YAML output for automation

## Installation

### From Source

```bash
cd attune
cargo install --path crates/cli
```

This will install the `attune` binary to your cargo bin directory (usually `~/.cargo/bin`).

### Development

```bash
cargo build -p attune-cli
./target/debug/attune --help
./target/debug/attune-mcp --help
```

## Configuration

The CLI stores configuration in `~/.config/attune/config.yaml` (respects `$XDG_CONFIG_HOME`).

### Configuration Structure

```yaml
api_url: http://localhost:8080
auth_token: <jwt-access-token>
refresh_token: <jwt-refresh-token>
output_format: table
```

### Environment Variables

- `ATTUNE_API_URL`: Override the API endpoint
- `ATTUNE_PROFILE`: Select the saved profile to use
- `XDG_CONFIG_HOME`: Change config directory location

### Global Options

All commands support:
- `--api-url <URL>`: Override API endpoint
- `--output <format>`: Set output format (table, json, yaml; `ndjson` only for full cache scans)
- `-j, --json`: Output as JSON (shorthand for `--output json`)
- `-y, --yaml`: Output as YAML (shorthand for `--output yaml`)
- `-v, --verbose`: Enable debug logging

## Command Reference

### MCP Server

The CLI package also ships an MCP server binary named `attune-mcp`.

Use it when you want an MCP-capable agent or harness to interact with Attune through a curated tool surface backed by the existing API.

```bash
# Uses the active Attune CLI profile and auth tokens from ~/.config/attune/config.yaml
./target/debug/attune-mcp

# Override the API endpoint or profile explicitly
./target/debug/attune-mcp --api-url http://localhost:8080
./target/debug/attune-mcp --profile prod

# Run as a loopback-only HTTP service (POST /mcp requires this inbound token)
./target/debug/attune-mcp --transport http --http-bearer-token "$MCP_CLIENT_TOKEN"

# Remote/container exposure requires both an explicit public-listen opt-in and auth
./target/debug/attune-mcp --transport http --listen-addr 0.0.0.0:8090 \
  --public-listen --http-bearer-token "$MCP_CLIENT_TOKEN"

# Enable local pack checking over HTTP for explicitly mounted roots
./target/debug/attune-mcp --transport http --http-bearer-token "$MCP_CLIENT_TOKEN" \
  --packs-check-root /workspace/packs

# Run with an execution-scoped token inside an Attune action/worker
ATTUNE_API_URL=http://attune-api:8080 ATTUNE_API_TOKEN="$ATTUNE_API_TOKEN" ./target/debug/attune-mcp
```

Current MCP tool families:
- actions: list, get, execute
- packs: list, get, update configuration, list actions, and check local pack metadata
- workflows: list, get
- executions: get, cancel
- queues: list, get, enqueue
- artifacts: list, get
- events: list, get
- inquiries: list, respond
- caches: owner-scoped namespace lifecycle, bounded entry lookup/scan, generation inspection, and bounded refresh lifecycle

Notes:
- `attune-mcp` defaults to **stdio transport** for MCP client launchers. HTTP transport defaults to `127.0.0.1:8090`; `POST /mcp` requires `Authorization: Bearer <token>` configured separately with `--http-bearer-token` or `ATTUNE_MCP_HTTP_BEARER_TOKEN`. `GET /health` is intentionally public for service probes.
- A non-loopback `--listen-addr` is rejected unless `--public-listen` (or `ATTUNE_MCP_PUBLIC_LISTEN=true`) is also set and the inbound bearer token is configured. Inbound HTTP auth never reuses `ATTUNE_AUTH_TOKEN`, `ATTUNE_API_TOKEN`, saved profile tokens, or login credentials used for outbound Attune API calls.
- It reuses the same CLI config/profile/auth state as `attune`, and also supports non-interactive startup auth via `ATTUNE_AUTH_TOKEN` / `ATTUNE_REFRESH_TOKEN` or `ATTUNE_LOGIN` / `ATTUNE_PASSWORD`.
- For Attune-managed executions, `ATTUNE_API_TOKEN` is supported as an **execution-scoped auth source** and takes precedence over saved profile tokens.
- The main `attune` CLI uses the same token env precedence, so helper commands running inside worker containers can reuse execution-scoped tokens without creating a profile on disk.
- When a container image does not provide a system CA bundle, the CLI falls back to bundled Mozilla root certificates so internal execution-token API calls do not panic during client initialization.
- Direct event creation is intentionally not exposed in MCP because the Attune API restricts event emission to sensor/execution token flows.
- Cache scans return one bounded page. Entry values are omitted unless the client explicitly sets `include_values`; MCP intentionally does not expose unbounded scans or file-based bulk cache imports.
- `packs_check` reads paths from the `attune-mcp` process host. It is available without path restrictions over the local stdio transport.
- HTTP transport disables `packs_check` by default. Enable it with one or more `--packs-check-root PATH` options, or comma-separated `ATTUNE_MCP_PACKS_CHECK_ROOTS`. Requested paths and configured roots are canonicalized, and checks outside those roots are rejected. A container can therefore check only directories mounted beneath an allowlisted root.

Container deployment surfaces:
- Docker Compose includes an optional `mcp` profile-backed service on loopback port `8090`; set `ATTUNE_MCP_HTTP_BEARER_TOKEN` before enabling the profile.
- The Helm chart includes an optional `mcp.enabled` deployment/service, disabled by default. Set `mcp.httpBearerToken`, or provide the `ATTUNE_MCP_HTTP_BEARER_TOKEN` key when using `security.existingSecret`.

### Authentication

#### Login
```bash
attune auth login --username admin
# Prompts for password securely
```

#### SSO Login (OIDC)
```bash
attune auth sso-login
# Opens a browser and saves the returned tokens to the active profile

attune auth sso-login --no-browser
# Prints the login URL for headless environments
```

#### Passwordless Token Login
```bash
attune auth token-login --token attune_it_...
# Or omit --token to prompt securely
```

#### Integration Token Management
```bash
attune auth token create --identity-id 42 --label "CI deploy bot"
attune auth token list --identity-id 42
attune auth token revoke --identity-id 42 7 --reason "rotated"
attune auth token delete --identity-id 42 7 --yes
```

Created integration tokens are displayed once. Store them in the integration's secret manager and revoke old tokens after rotation.

#### Logout
```bash
attune auth logout
```

#### Check Current User
```bash
attune auth whoami
```

### Pack Management

#### List Packs
```bash
attune pack list
attune pack list --name core
attune pack list --output json  # Long form
attune pack list -j             # Shorthand for JSON
attune pack list -y             # Shorthand for YAML
```

#### Show Pack Details
```bash
attune pack show core
attune pack show 1
```

#### Install Pack
```bash
attune pack install https://github.com/example/pack-example
attune pack install https://github.com/example/pack-example --ref v1.0.0
attune pack install <url> --force
```

#### Register Local Pack
```bash
attune pack register /path/to/pack
```

#### Check Local Pack
```bash
attune pack check .
attune pack check /path/to/pack --output json
```

`pack check` is read-only and local: it does not require authentication or contact an Attune server. It checks `pack.yaml`, all registrar-supported component directories, workflow definitions, referenced files, duplicate refs, and local component references. Invalid packs return a nonzero exit status; JSON and YAML output include stable diagnostic codes for automation.

#### Uninstall Pack
```bash
attune pack uninstall core
attune pack uninstall core --yes
```

### Action Management

#### List Actions
```bash
attune action list
attune action list --pack core
attune action list --name echo
```

#### Show Action Details
```bash
attune action show core.echo
attune action show 1
```

#### Execute Action
```bash
# With key=value parameters
attune action execute core.echo --param message="Hello" --param count=3

# With JSON parameters
attune action execute core.echo --params-json '{"message": "Hello", "count": 5}'

# Watch until completion
attune action execute core.long_task --watch

# Watch with timeout
attune action execute core.long_task --watch --timeout 600
```

#### Enable/Disable Actions
```bash
attune action enable core.echo
attune action disable core.echo
```

### Rule Management

#### List Rules
```bash
attune rule list
attune rule list --pack core
attune rule list --enabled true
```

#### Show Rule Details
```bash
attune rule show core.on_webhook
attune rule show 1
```

#### Enable/Disable Rules
```bash
attune rule enable core.on_webhook
attune rule disable core.on_webhook
```

#### Create Rule
```bash
attune rule create \
  --name my_rule \
  --pack core \
  --trigger core.webhook \
  --action core.notify \
  --description "Notify on webhook" \
  --enabled

# With criteria
attune rule create \
  --name filtered_rule \
  --pack core \
  --trigger core.webhook \
  --action core.notify \
  --criteria '{"event.payload.severity": "critical"}'
```

#### Delete Rule
```bash
attune rule delete core.my_rule
attune rule delete core.my_rule --yes
```

### Execution Monitoring

#### List Executions
```bash
attune execution list
attune execution list --pack core
attune execution list --action core.echo
attune execution list --status succeeded
attune execution list --result "error"
attune execution list --pack monitoring --status failed --result "timeout"
attune execution list --limit 100
```

#### Show Execution Details
```bash
attune execution show 123
```

#### View Logs
```bash
attune execution logs 123
attune execution logs 123 --follow
```

#### Cancel Execution
```bash
attune execution cancel 123
attune execution cancel 123 --yes
```

#### Get Raw Execution Result
```bash
# Get result as JSON (default)
attune execution result 123

# Get result as YAML
attune execution result 123 --format yaml

# Pipe to jq for processing
attune execution result 123 | jq '.data.field'

# Extract specific field
attune execution result 123 | jq -r '.status'
```

### Trigger Management

#### List Triggers
```bash
attune trigger list
attune trigger list --pack core
```

#### Show Trigger Details
```bash
attune trigger show core.webhook
```

#### Enable/Disable Triggers
```bash
attune trigger enable core.webhook
attune trigger disable core.webhook
```

### Sensor Management

#### List Sensors
```bash
attune sensor list
attune sensor list --pack core
```

#### Show Sensor Details
```bash
attune sensor show core.file_watcher
```

#### Enable/Disable Sensors
```bash
attune sensor enable core.file_watcher
attune sensor disable core.file_watcher
```

### Queue Management

#### Show Queue Details
```bash
attune queue show core.inbox
```

#### Enable/Disable Queue Processing
```bash
attune queue enable core.inbox
attune queue disable core.inbox
```

#### Query and Maintain Pending Queue Items
Queue item selector commands use PostgreSQL SQL/JSONPath and only operate on pending mutable items (`queued` and `retry`).

```bash
# Preview up to 100 matching pending items
attune queue items core.inbox preview \
  --selector '$.payload.customer_id ? (@ == $customer_id)' \
  --vars-json '{"customer_id":123}'

# Merge-patch matching item payloads
attune queue items core.inbox update \
  --selector '$.payload.customer_id ? (@ == $customer_id)' \
  --vars-json '{"customer_id":123}' \
  --patch-json '{"status":"reviewed"}'

# Reprioritize matching items
attune queue items core.inbox reprioritize \
  --selector '$.metadata.source ? (@ == "import")' \
  --priority 50

# Delete matching pending items by marking them cancelled
attune queue items core.inbox delete \
  --selector '$.payload.customer_id ? (@ == $customer_id)' \
  --vars-json '{"customer_id":123}'
```

### Policy Management

Policies control execution concurrency, rate limits, and quota checks. Commands use structured flags for common policy features instead of requiring raw JSON.

#### List and Show Policies
```bash
attune policy list
attune policy list --scope action --action core.echo
attune policy list --pack core --enabled true
attune policy show core.limit_echo
```

#### Create Policies
```bash
# Action-scoped concurrency policy
attune policy create \
  --policy-ref core.limit_echo \
  --name "Limit echo" \
  --scope action \
  --action core.echo \
  --concurrency-limit 5 \
  --on-concurrency enqueue \
  --group-by customer_id

# Pack-scoped rate limit with quotas
attune policy create \
  --policy-ref core.pack_limits \
  --name "Core pack limits" \
  --scope pack \
  --pack core \
  --rate-limit-max 100 \
  --rate-limit-window 1h \
  --quota-running-executions 20 \
  --quota-executions-total 1000
```

#### Update, Enable, Disable, and Delete Policies
```bash
attune policy update core.limit_echo --priority 20 --concurrency-limit 10
attune policy update core.limit_echo --clear-rate-limit
attune policy enable core.limit_echo
attune policy disable core.limit_echo
attune policy delete core.limit_echo --yes
```

### Cache Management

`attune cache` manages versioned external-data caches separately from keys and
secrets. Every command requires an explicit typed owner, including
`--owner-type system` for system-owned data.
`--owner-type identity` always selects the authenticated identity and takes no
owner-ref/owner-ID flag.

```bash
# Namespace lifecycle and policy
attune cache namespace create salesforce.users --owner-type pack --owner-pack-ref salesforce
attune cache namespace list --owner-type pack --owner-pack-ref salesforce
attune cache namespace show salesforce.users --owner-type pack --owner-pack-ref salesforce
attune cache namespace delete salesforce.users --owner-type pack --owner-pack-ref salesforce --yes

# Deliberate, bounded reads
attune cache entry get salesforce.users 005xx --owner-type pack --owner-pack-ref salesforce
attune cache entry get-many salesforce.users --owner-type pack --owner-pack-ref salesforce \
  --external-id 005xx --external-id-file ids.txt
attune cache entry scan salesforce.users --owner-type pack --owner-pack-ref salesforce

# Stream every page of one pinned generation (records: stdout; cursors: stderr)
attune --output ndjson cache entry scan salesforce.users \
  --owner-type pack --owner-pack-ref salesforce --all > users.ndjson

# Copy-on-write refresh lifecycle
attune cache refresh begin salesforce.users --owner-type pack --owner-pack-ref salesforce \
  --expected-chunk-count 2 --expect-empty
attune cache refresh upload salesforce.users 123 --owner-type pack --owner-pack-ref salesforce \
  --chunk-index 0 --file users-part-0.ndjson
attune cache refresh seal salesforce.users 123 --owner-type pack --owner-pack-ref salesforce
attune cache refresh promote salesforce.users 123 --owner-type pack --owner-pack-ref salesforce \
  --expect-empty
```

`refresh apply --input <ndjson>` uses the same lifecycle and reads the input in
bounded chunks. It never force-promotes a generation; pass either
`--expected-active <id>` or `--expect-empty`.

### Configuration Management

#### List Configuration
```bash
attune config list
```

#### Get Value
```bash
attune config get api_url
```

#### Set Value
```bash
attune config set api_url https://attune.example.com
attune config set output_format json
```

#### Show Config Path
```bash
attune config path
```

## Output Formats

### Table (Default)

Human-readable format with colors and formatting:
```bash
attune pack list
```

Output:
```
╭────┬──────┬─────────┬─────────┬─────────────────╮
│ ID │ Name │ Version │ Enabled │ Description     │
├────┼──────┼─────────┼─────────┼─────────────────┤
│ 1  │ core │ 1.0.0   │ ✓       │ Core actions... │
╰────┴──────┴─────────┴─────────┴─────────────────╯
```

### JSON

Machine-readable format for scripting:
```bash
attune pack list --output json  # Long form
attune pack list -j             # Shorthand
```

Output:
```json
[
  {
    "id": 1,
    "name": "core",
    "version": "1.0.0",
    "enabled": true,
    "description": "Core actions..."
  }
]
```

### YAML

Alternative structured format:
```bash
attune pack list --output yaml  # Long form
attune pack list -y             # Shorthand
```

Output:
```yaml
- id: 1
  name: core
  version: 1.0.0
  enabled: true
  description: Core actions...
```

### NDJSON (cache scans only)

`--output ndjson` is accepted only with `attune cache entry scan --all`. It
writes one complete entry per stdout line and snapshot/cursor metadata to
stderr, avoiding whole-dataset materialization.

## Scripting Examples

### Bash Script: Deploy Pack

```bash
#!/bin/bash
set -e

PACK_URL="https://github.com/example/monitoring-pack"
PACK_NAME="monitoring"

# Install pack
echo "Installing pack..."
PACK_ID=$(attune pack install "$PACK_URL" -j | jq -r '.id')

# Verify installation
if [ -z "$PACK_ID" ]; then
  echo "Pack installation failed"
  exit 1
fi

echo "Pack installed: ID=$PACK_ID"

# Enable all rules
attune rule list --pack "$PACK_NAME" -j | \
  jq -r '.[].id' | \
  xargs -I {} attune rule enable {}

echo "All rules enabled"
```

### Bash Script: Process Execution Results

```bash
#!/bin/bash
# Extract and process execution results

EXECUTION_ID=123

# Get raw result
RESULT=$(attune execution result $EXECUTION_ID)

# Extract specific fields
STATUS=$(echo "$RESULT" | jq -r '.status')
MESSAGE=$(echo "$RESULT" | jq -r '.message')

echo "Status: $STATUS"
echo "Message: $MESSAGE"

# Or pipe directly
attune execution result $EXECUTION_ID | jq -r '.errors[]'
```

### Python Script: Monitor Executions

```python
#!/usr/bin/env python3
import json
import subprocess
import time

def get_executions(status=None, pack=None, result_contains=None, limit=10):
    cmd = ["attune", "execution", "list", "-j", f"--limit={limit}"]
    if status:
        cmd.extend(["--status", status])
    if pack:
        cmd.extend(["--pack", pack])
    if result_contains:
        cmd.extend(["--result", result_contains])
    
    result = subprocess.run(cmd, capture_output=True, text=True)
    return json.loads(result.stdout)

def main():
    print("Monitoring failed executions with errors...")
    while True:
        # Find failed executions containing "error" in result
        failed = get_executions(status="failed", result_contains="error", limit=5)
        if failed:
            print(f"Found {len(failed)} failed executions:")
            for exec in failed:
                print(f"  - ID {exec['id']}: {exec['action_name']}")
        time.sleep(30)

if __name__ == "__main__":
    main()
```

## Troubleshooting

### Authentication Issues

**Problem**: "Not logged in" error

**Solution**:
```bash
# Check auth status
attune auth whoami

# Login again
attune auth login --username admin
```

### Connection Issues

**Problem**: Cannot connect to API

**Solution**:
```bash
# Check API URL
attune config get api_url

# Override temporarily
attune --api-url http://localhost:8080 auth whoami

# Update permanently
attune config set api_url http://localhost:8080
```

### Token Expiration

**Problem**: "Invalid token" error

**Solution**:
```bash
# Login again to refresh token
attune auth login --username admin
```

### Verbose Debugging

Enable verbose output to see HTTP requests:
```bash
attune --verbose pack list
```

## Best Practices

### Security

1. **Never hardcode passwords**: Use interactive prompts
2. **Protect config file**: Contains JWT tokens
3. **Use environment variables** for CI/CD: `ATTUNE_API_URL`

### Scripting

1. **Use JSON output** for parsing: `--output json`
2. **Check exit codes**: Non-zero on error
3. **Handle errors**: Use `set -e` in bash scripts
4. **Use jq** for JSON processing

### Performance

1. **Limit results**: Use `--limit` for large lists
2. **Filter server-side**: Use `--pack`, `--action`, `--status`, `--result` filters
3. **Avoid polling**: Use `--wait` for action execution
4. **Use specific filters**: Narrow results with combined filters for faster queries

## Architecture

### Components

```
attune-cli/
├── src/
│   ├── main.rs           # Entry point, CLI structure
│   ├── client.rs         # HTTP client wrapper
│   ├── config.rs         # Config file management
│   ├── output.rs         # Output formatting
│   └── commands/         # Command implementations
│       ├── auth.rs
│       ├── pack.rs
│       ├── action.rs
│       ├── rule.rs
│       ├── execution.rs
│       ├── trigger.rs
│       ├── sensor.rs
│       └── config.rs
```

### Key Dependencies

- **clap**: CLI argument parsing
- **reqwest**: HTTP client
- **serde_json/yaml**: Serialization
- **colored**: Terminal colors
- **comfy-table**: Table formatting
- **dialoguer**: Interactive prompts

### API Communication

The CLI communicates with the Attune API using:
- REST endpoints at `/api/v1/*`
- JWT bearer token authentication
- Standard JSON request/response format

## Future Enhancements

Potential future features:
- Shell completion (bash, zsh, fish)
- Interactive TUI mode
- Execution streaming (real-time logs)
- Bulk operations
- Pack development commands
- Workflow visualization
- Config profiles (dev, staging, prod)

## Related Documentation

- [Main README](../README.md)
- [API Documentation](api-overview.md)
- [Pack Development](packs.md)
- [Configuration Guide](configuration.md)
