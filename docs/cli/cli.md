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

# Run as an HTTP service for containers or remote MCP clients
./target/debug/attune-mcp --transport http --listen-addr 0.0.0.0:8090

# Enable local pack checking over HTTP for explicitly mounted roots
./target/debug/attune-mcp --transport http --packs-check-root /workspace/packs

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
- inquiries: list, get, respond, and execution-scoped create and cancel
- caches: owner-scoped namespace lifecycle, bounded entry lookup/scan, generation inspection, and bounded refresh lifecycle

Notes:
- `attune-mcp` defaults to **stdio transport** for MCP client launchers, but also supports **HTTP transport** at `POST /mcp` with `GET /health` for containerized deployment.
- It reuses the same CLI config/profile/auth state as `attune`, and also supports non-interactive startup auth via `ATTUNE_AUTH_TOKEN` / `ATTUNE_REFRESH_TOKEN` or `ATTUNE_LOGIN` / `ATTUNE_PASSWORD`.
- For Attune-managed executions, `ATTUNE_API_TOKEN` is supported as an **execution-scoped auth source** and takes precedence over saved profile tokens.
- The main `attune` CLI uses the same token env precedence, so helper commands running inside worker containers can reuse execution-scoped tokens without creating a profile on disk.
- When a container image does not provide a system CA bundle, the CLI falls back to bundled Mozilla root certificates so internal execution-token API calls do not panic during client initialization.
- Direct event creation is intentionally not exposed in MCP because the Attune API restricts event emission to sensor/execution token flows.
- Cache scans return one bounded page. Entry values are omitted unless the client explicitly sets `include_values`; MCP intentionally does not expose unbounded scans or file-based bulk cache imports.
- `packs_check` reads paths from the `attune-mcp` process host. It is available without path restrictions over the local stdio transport.
- HTTP transport disables `packs_check` by default. Enable it with one or more `--packs-check-root PATH` options, or comma-separated `ATTUNE_MCP_PACKS_CHECK_ROOTS`. Requested paths and configured roots are canonicalized, and checks outside those roots are rejected. A container can therefore check only directories mounted beneath an allowlisted root.

Container deployment surfaces:
- Docker Compose includes an optional `mcp` profile-backed service on port `8090`.
- The Helm chart includes an optional `mcp.enabled` deployment/service, disabled by default.

### Authentication

#### Login
```bash
attune auth login --username admin
# Prompts for password securely
```

#### SSO login with a device code
```bash
attune auth sso-login
# Displays the verification URL and user code, then opens the provider's approval page

attune auth sso-login --no-browser
# Approve in a browser on any machine; the CLI uses outbound requests only

attune auth sso-login --save-profile production --url https://attune.example.com --no-browser --timeout 300
```

SSO uses the OAuth 2.0 Device Authorization Grant from RFC 8628. The CLI opens no
local port or callback server. It polls Attune until approval, denial, or expiry,
and saves credentials to the selected profile only after successful approval.
The provider must advertise `device_authorization_endpoint` and enable the device
grant for the configured client. See [OIDC device login](../deployment/oidc-device-login.md)
for server and provider configuration.

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
attune pack install https://github.com/example/pack-example --ref-spec v1.0.0
attune pack install example@1.0.0 --registry-id 42
attune pack install https://example.com/example.tar.gz --no-registry
attune pack install <url> --force
```

`--registry-id` pins a registry ref to one enabled managed index.
`--no-registry` requires an explicit URL or a path already visible to the API server and never falls
back to registry lookup. They cannot be combined.

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

#### Pack indices

```bash
attune pack index list
attune pack index add https://example.invalid/index.json --name "Example index"
attune pack index update 42 --position 0
attune pack index browse
attune pack index browse postgres
attune pack index show postgres
attune pack index delete 42
```

Use `pack index` to manage server-side catalog URLs and browse their entries.
Use a registry ID with `pack install <ref>@<version> --registry-id <id>` to
resolve a pack from one specific enabled index.

#### Build index files

```bash
attune pack index-entry ./packs/my_pack \
  --git-url https://github.com/example/my_pack.git \
  --git-ref <40-character-commit-sha>
attune pack index-update --index index.json ./packs/my_pack \
  --git-url https://github.com/example/my_pack.git \
  --git-ref <40-character-commit-sha>
attune pack index-merge --file merged-index.json first-index.json second-index.json
```

`index-entry` prints one entry from a pack directory. `index-update` adds that
entry to an existing index. `index-merge` writes a merged index file.

### Shell completion

```bash
# Bash, current shell
source <(attune completion bash)

# Fish
attune completion fish > ~/.config/fish/completions/attune.fish

# Zsh
mkdir -p ~/.zsh/completions
attune completion zsh > ~/.zsh/completions/_attune
fpath=(~/.zsh/completions $fpath)
autoload -Uz compinit && compinit
```

The scripts complete commands and options without contacting Attune. Dynamic
action and parameter candidates use the active profile. If the API is not
available, completion returns only local candidates.

### Build information

Build identity for the local CLI and its selected-profile server is available through `attune info`. `attune info --local` reports only the local binary. MCP exposes `info_get` and `attune-mcp --info`. See [Build information](../deployment/build-information.md) for output fields and build-time SHA configuration.

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

# Configure an execution independently of how long the CLI watches it
attune action execute core.long_task \
  --env LOG_LEVEL=debug \
  --permission-set standard \
  --artifact-retention-policy hours \
  --artifact-retention-limit 24 \
  --worker-selector '{"pool":"batch"}' \
  --execution-timeout 1800 \
  --watch --timeout 1900

# Disable the execution API token and provide string-valued environment overrides
attune run core.echo \
  --param message=hello \
  --env-json '{"LOG_LEVEL":"debug","DEBUG":"true"}' \
  --no-api-token
```

`action execute`, `run`, and `execution rerun` accept the same execution options:

| Option | Request behavior |
| --- | --- |
| `--env KEY=VALUE` | Repeatable string assignments. Values remain strings, including `true` and `3`. |
| `--env-json JSON` | String-valued JSON object. Conflicts with `--env`. |
| `--permission-set REF` | Repeatable explicit execution-token permission sets. Omission inherits action defaults. |
| `--no-api-token` | Sends an empty permission list. Conflicts with `--permission-set`. |
| `--artifact-retention-policy` | `versions`, `days`, `hours`, or `minutes`. Applies to non-log artifacts. |
| `--artifact-retention-limit` | Positive retention limit. Omission inherits the action default. |
| `--worker-selector JSON` | Worker label requirements. `{}` clears the action selector. |
| `--worker-tolerations JSON` | Worker taint tolerations. `[]` clears action tolerations. |
| `--worker-affinity JSON` | Required, preferred, and anti-affinity terms. `{}` clears action affinity. |
| `--execution-timeout SECONDS` | Positive execution timeout. Omission inherits the action or application default. |

Execution environment values override runtime values and inherited process values. Names beginning with `ATTUNE_` are reserved and rejected. Environment values cannot contain NUL. Names cannot be empty or contain `=` or NUL.

Omitted placement options inherit action defaults. Pack placement constraints still apply after an action override. `--timeout` limits the CLI watch duration; it does not change the execution timeout.

Rerun reuses the previous parameters. Execution options supplied to rerun configure the new execution; omitted options use the current action defaults.

### Cancel or detach while watching

For `run --watch`, `action execute --watch`, `execution rerun --watch`, and
`execution watch <id>`, use these controls:

| Control | Behavior |
| --- | --- |
| Ctrl+C | Requests cancellation of the watched execution, stops local watchers, and exits with code `130` after the API accepts the request. |
| Ctrl+D | Stops local watchers without requesting cancellation and exits with code `0`. No Enter key is required. |

Cancellation uses your CLI credentials and requires permission to cancel that
execution. The CLI reports cancellation failures and exits with code `1`.
An accepted request can leave the execution in `canceling` while the worker stops
it. The CLI does not wait for that transition to finish.

Ctrl+D is available when stdin is an interactive terminal. Closed or redirected
stdin does not detach scripted watches. Ctrl+C also handles an external SIGINT
on Unix. The CLI restores terminal settings and stops its output readers before
returning.

Reattach to an execution after detaching:

```bash
attune execution watch 5
```

The watch's `--timeout` stops watching without requesting cancellation. Watching
the execution list with `execution watch` still uses Ctrl+C to stop the list
display, without cancelling executions.

The MCP `actions_execute` tool accepts the same API fields directly:

```json
{
  "action_ref": "core.echo",
  "parameters": {"message": "hello"},
  "env_vars": {"LOG_LEVEL": "debug"},
  "permission_set_refs": [],
  "artifact_retention_policy": "hours",
  "artifact_retention_limit": 24,
  "worker_selector": {},
  "worker_tolerations": [],
  "worker_affinity": {},
  "timeout_seconds": 600
}
```

Only `action_ref` is required. Unspecified MCP fields remain omitted. Empty permission and placement collections keep the same clearing semantics as the CLI. Execution creation is asynchronous; use `executions_get` to inspect progress.

Watched commands derive the notifier WebSocket from the API origin. For a
separate notifier origin, pass `--notifier-url` or set
`ATTUNE_NOTIFIER_WS_URL`. Specify the base URL without `/ws`:

```bash
export ATTUNE_NOTIFIER_WS_URL=wss://attune.example.com
attune action execute core.long_task --watch
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

### Inquiry management

Use an access token to browse and answer inquiries:

```bash
attune inquiry list --status pending
attune inquiry show 123
attune inquiry respond 123 --option approve
attune inquiry respond 123 --response-json '{"approved":true}'
```

Creation and cancellation require an execution token. Creation also requires an explicit permission set that grants `inquiries:create`:

```bash
attune inquiry execution create --request-file inquiry.json
attune inquiry execution create --request-file - < inquiry.json
attune inquiry execution cancel 123
```

Use `--offset` and `--limit` to page through list results. JSON and YAML output includes `items` and `pagination`. See the [Inquiry API](../api/api-inquiries.md#client-and-caller-matrix) for visibility and response rules.

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

#### List Queues
```bash
attune queue list
attune queue list --pack core --enabled true
attune queue list --search inbox --is-adhoc false
attune queue list --referencing-pack-ref incident_response --page 1 --per-page 25
attune --output json queue list
```

Queue discovery supports the API's enabled, queue type, text search, referencing
pack, and pagination filters. `--pack` uses the pack-scoped queue endpoint.

#### Show Queue Details
```bash
attune queue show core.inbox
```

#### Enqueue an Item
```bash
# Inline request JSON
attune queue enqueue core.inbox \
  --request-json '{"item_key":"order-123","priority":5,"payload":{"order_id":123},"metadata":{"source":"cli"}}'

# Read the same request shape from a file or stdin
attune queue enqueue core.inbox --request-file item.json
cat item.json | attune --output json queue enqueue core.inbox --request-file -
```

The request requires `payload` and may contain `item_key`, `priority`,
`metadata`, and `trace_tag`. The CLI rejects other fields instead of silently
sending misspelled or unsupported data.

#### Enable/Disable Queue Processing
```bash
attune queue enable core.inbox
attune queue disable core.inbox
```

#### Query and Maintain Pending Queue Items
Queue item selector commands use PostgreSQL SQL/JSONPath and only operate on pending mutable items (`queued` and `retry`).

```bash
# Inspect items in any lifecycle state
attune queue items core.inbox list
attune queue items core.inbox list --status queued --status retry
attune queue items core.inbox list --item-key order-123 --enqueue-source api
attune queue items core.inbox list --page 2 --per-page 50 --output json
attune queue items core.inbox show 42

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

Item list status values are `queued`, `leased`, `retry`, `completed`, `failed`,
`skipped`, and `cancelled`. Table output includes status, priority, attempt
count, payload, and creation time. Item detail also shows lease state, trace
information, errors, acknowledgement data, and request lineage when the API
permits it. JSON and YAML output contain the response object without headings
or success messages; list output retains pagination metadata.

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
secrets. `cache namespace list` lists every namespace that the authenticated
identity can read when you omit the owner flags. All other commands require an
explicit typed owner, including `--owner-type system` for system-owned data.
`--owner-type identity` always selects the authenticated identity and takes no
owner-ref/owner-ID flag.

```bash
# Namespace lifecycle and policy
attune cache namespace create salesforce.users --owner-type pack --owner-pack-ref salesforce
attune cache namespace list
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

### Verbose debugging

Use `--verbose` to log each request's HTTP method and destination URL to stderr:
```bash
attune --verbose pack list
attune --verbose auth whoami
attune --verbose auth sso-login --url https://attune.example.com
```

Request logs include the URL path and omit URL credentials, query strings, and
fragments. Authentication refreshes and retries produce their own request logs.
For example:

```text
DEBUG attune_cli::client: Sending HTTP request method="GET" url=https://attune.example.com/api/v1/packs
DEBUG attune_cli::client: Sending HTTP request method="GET" url=https://attune.example.com/auth/me
DEBUG attune_cli::client: Sending HTTP request method="POST" url=https://attune.example.com/auth/oidc/device/start
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
