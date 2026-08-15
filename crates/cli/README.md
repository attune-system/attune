# Attune CLI

The Attune CLI is a command-line interface for interacting with the Attune automation platform. It provides an intuitive and flexible interface for managing packs, actions, rules, sensors, triggers, and executions.

## Installation

### Homebrew (macOS)

After the first stable release, install the CLI and MCP server from the Attune
tap:

```bash
brew install --cask attune-system/attune-client-tap/attune
```

This installs both `attune` and `attune-mcp`.

### From Source

```bash
cargo install --path crates/cli
```

The binary will be named `attune`.

### Development Build

```bash
cargo build -p attune-cli
./target/debug/attune --help
```

### Release Build

```bash
cargo build -p attune-cli --release
./target/release/attune --help
```

## Configuration

The CLI stores configuration in `~/.config/attune/config.yaml` (or `$XDG_CONFIG_HOME/attune/config.yaml`).

Default configuration:
```yaml
api_url: http://localhost:8080
auth_token: null
refresh_token: null
output_format: table
```

### Environment Variables

- `ATTUNE_API_URL`: Override the API endpoint URL
- Standard XDG environment variables for config directory location

### Global Flags

All commands support these global flags:

- `--api-url <URL>`: Override the API endpoint (also via `ATTUNE_API_URL`)
- `--output <FORMAT>`: Output format (`table`, `json`, `yaml`)
- `-j, --json`: Output as JSON (shorthand for `--output json`)
- `-y, --yaml`: Output as YAML (shorthand for `--output yaml`)
- `-v, --verbose`: Enable verbose logging

## Shell Completion

Bash, Fish, and Zsh completion cover the CLI's execution options and dynamically complete
action references and `--param name=value` entries for `attune run` and
`attune action execute`. It uses the configured profile to query actions that
the current user may access.

Enable it for the current shell:

```bash
source <(attune completion bash)
```

For Fish, install the generated completion file:

```fish
attune completion fish > ~/.config/fish/completions/attune.fish
```

For Zsh, install the generated completion file in a directory in your `fpath`:

```zsh
mkdir -p ~/.zsh/completions
attune completion zsh > ~/.zsh/completions/_attune
fpath=(~/.zsh/completions $fpath)
autoload -Uz compinit && compinit
```

Add the Bash command to your Bash startup file to enable it permanently. Dynamic
completion is best effort: an unavailable API or expired login produces no
dynamic candidates and does not interrupt the command line.

## Authentication

### Login

```bash
# Interactive password prompt
attune auth login --username admin

# With password (not recommended for interactive use)
attune auth login --username admin --password secret

# With custom API URL
attune auth login --username admin --api-url https://attune.example.com
```

### Logout

```bash
attune auth logout
```

### Check Current User

```bash
attune auth whoami
```

## Pack Management

### List Packs

```bash
# List all packs
attune pack list

# Filter by name
attune pack list --name core

# JSON output (long form)
attune pack list --output json

# JSON output (shorthand)
attune pack list -j

# YAML output (shorthand)
attune pack list -y
```

### Show Pack Details

```bash
# By name
attune pack show core

# By ID
attune pack show 1
```

### Install Pack

```bash
# From git repository
attune pack install https://github.com/example/attune-pack-example

# From git with specific branch/tag
attune pack install https://github.com/example/attune-pack-example --ref v1.0.0

# Force reinstall
attune pack install https://github.com/example/attune-pack-example --force
```

### Register Local Pack

```bash
# Register from local directory
attune pack register /path/to/pack
```

### Uninstall Pack

```bash
# Interactive confirmation
attune pack uninstall core

# Skip confirmation
attune pack uninstall core --yes
```

## Action Management

### List Actions

```bash
# List all actions
attune action list

# Filter by pack
attune action list --pack core

# Filter by name
attune action list --name execute
```

### Show Action Details

```bash
# By pack.action reference
attune action show core.echo

# By ID
attune action show 1
```

### Execute Action

```bash
# With key=value parameters
attune action execute core.echo --param message="Hello World" --param count=3

# With JSON parameters
attune action execute core.echo --params-json '{"message": "Hello", "count": 5}'

# Watch until completion
attune action execute core.long_task --watch

# Watch with custom timeout (default 300 seconds)
attune action execute core.long_task --watch --timeout 600
```

## Rule Management

### List Rules

```bash
# List all rules
attune rule list

# Filter by pack
attune rule list --pack core

# Filter by enabled status
attune rule list --enabled true
```

### Show Rule Details

```bash
# By pack.rule reference
attune rule show core.on_webhook

# By ID
attune rule show 1
```

### Enable/Disable Rules

```bash
# Enable a rule
attune rule enable core.on_webhook

# Disable a rule
attune rule disable core.on_webhook
```

### Create Rule

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

### Delete Rule

```bash
# Interactive confirmation
attune rule delete core.my_rule

# Skip confirmation
attune rule delete core.my_rule --yes
```

## Execution Monitoring

### List Executions

```bash
# List recent executions (default: last 50)
attune execution list

# Filter by pack
attune execution list --pack core

# Filter by action
attune execution list --action core.echo

# Filter by status
attune execution list --status succeeded

# Search in execution results
attune execution list --result "error"

# Combine filters
attune execution list --pack monitoring --status failed --result "timeout"

# Limit results
attune execution list --limit 100
```

### Show Execution Details

```bash
attune execution show 123
```

### View Execution Logs

```bash
# Show logs
attune execution logs 123

# Follow logs (real-time)
attune execution logs 123 --follow
```

### Cancel Execution

```bash
# Interactive confirmation
attune execution cancel 123

# Skip confirmation
attune execution cancel 123 --yes
```

### Get Raw Execution Result

Get just the result data from a completed execution, useful for piping to other tools.

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

## Trigger Management

### List Triggers

```bash
# List all triggers
attune trigger list

# Filter by pack
attune trigger list --pack core
```

### Show Trigger Details

```bash
attune trigger show core.webhook
```

## Sensor Management

### List Sensors

```bash
# List all sensors
attune sensor list

# Filter by pack
attune sensor list --pack core
```

### Show Sensor Details

```bash
attune sensor show core.file_watcher
```

## CLI Configuration

### List Configuration

```bash
attune config list
```

### Get Configuration Value

```bash
attune config get api_url
```

### Set Configuration Value

```bash
# Set API URL
attune config set api_url https://attune.example.com

# Set output format
attune config set output_format json
```

### Show Configuration File Path

```bash
attune config path
```

## Output Formats

### Table (Default)

Human-readable table format with colored output:

```bash
attune pack list
```

### JSON

Machine-readable JSON for scripting:

```bash
# Long form
attune pack list --output json

# Shorthand
attune pack list -j
```

### YAML

YAML format:

```bash
# Long form
attune pack list --output yaml

# Shorthand
attune pack list -y
```

## Examples

### Complete Workflow Example

```bash
# 1. Login
attune auth login --username admin

# 2. Install a pack
attune pack install https://github.com/example/monitoring-pack

# 3. List available actions
attune action list --pack monitoring

# 4. Execute an action
attune action execute monitoring.check_health --param endpoint=https://api.example.com

# 5. Enable a rule
attune rule enable monitoring.alert_on_failure

# 6. Monitor executions
attune execution list --action monitoring.check_health
```

### Scripting Example

```bash
#!/bin/bash
# Deploy and test a pack

set -e

PACK_URL="https://github.com/example/my-pack"
PACK_NAME="my-pack"

# Install pack
echo "Installing pack..."
attune pack install "$PACK_URL" -j | jq -r '.id'

# Verify installation
echo "Verifying pack..."
PACK_ID=$(attune pack list --name "$PACK_NAME" -j | jq -r '.[0].id')

if [ -z "$PACK_ID" ]; then
  echo "Pack installation failed"
  exit 1
fi

echo "Pack installed successfully with ID: $PACK_ID"

# List actions in the pack
echo "Actions in pack:"
attune action list --pack "$PACK_NAME"

# Enable all rules in the pack
attune rule list --pack "$PACK_NAME" -j | \
  jq -r '.[].id' | \
  xargs -I {} attune rule enable {}

echo "All rules enabled"
```

### Process Execution Results

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

## Troubleshooting

### Authentication Issues

If you get authentication errors:

1. Check you're logged in: `attune auth whoami`
2. Try logging in again: `attune auth login --username <user>`
3. Verify API URL: `attune config get api_url`

### Connection Issues

If you can't connect to the API:

1. Verify the API is running: `curl http://localhost:8080/health`
2. Check the configured URL: `attune config get api_url`
3. Override the URL: `attune --api-url http://localhost:8080 auth whoami`

### Verbose Logging

Enable verbose logging for debugging:

```bash
attune --verbose pack list
```

## Development

### Building

```bash
cargo build -p attune-cli
```

### Testing

```bash
cargo test -p attune-cli
```

### Code Structure

```
crates/cli/
├── src/
│   ├── main.rs           # Entry point and CLI structure
│   ├── client.rs         # HTTP client for API calls
│   ├── config.rs         # Configuration management
│   ├── output.rs         # Output formatting (table, JSON, YAML)
│   └── commands/         # Command implementations
│       ├── auth.rs       # Authentication commands
│       ├── pack.rs       # Pack management commands
│       ├── action.rs     # Action commands
│       ├── rule.rs       # Rule commands
│       ├── execution.rs  # Execution commands
│       ├── trigger.rs    # Trigger commands
│       ├── sensor.rs     # Sensor commands
│       └── config.rs     # Config commands
└── Cargo.toml
```

## Features

- ✅ JWT authentication with token storage
- ✅ Multiple output formats (table, JSON, YAML)
- ✅ Colored and formatted table output
- ✅ Interactive prompts for sensitive operations
- ✅ Configuration management
- ✅ Advanced execution search (by pack, action, status, result content)
- ✅ Comprehensive pack management
- ✅ Action execution with parameter support
- ✅ Rule creation and management
- ✅ Execution monitoring and logs with advanced filtering
- ✅ Raw result extraction for piping to other tools
- ✅ Shorthand output flags (`-j`, `-y`) for CLI convenience
- ✅ Environment variable overrides

## Dependencies

Key dependencies:
- `clap`: CLI argument parsing
- `reqwest`: HTTP client
- `serde_json` / `serde_yaml`: Serialization
- `colored`: Terminal colors
- `comfy-table`: Table formatting
- `dialoguer`: Interactive prompts
- `indicatif`: Progress indicators (for future use)
