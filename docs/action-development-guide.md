# Action Development Guide

**Complete guide for developing actions in Attune**

## Table of Contents

1. [Introduction](#introduction)
2. [Action Anatomy](#action-anatomy)
3. [Parameter Configuration](#parameter-configuration)
4. [Output Configuration](#output-configuration)
5. [Standard Environment Variables](#standard-environment-variables)
6. [Runtime Configuration](#runtime-configuration)
7. [Complete Examples](#complete-examples)
8. [Best Practices](#best-practices)
9. [Troubleshooting](#troubleshooting)

---

## Introduction

Actions are the fundamental building blocks of automation in Attune. Each action is a script or program that performs a specific task, receives parameters, and returns results. This guide covers everything you need to know to write effective actions.

### What You'll Learn

- How to configure parameter delivery methods (stdin, file)
- How to format parameters (JSON, YAML, dotenv)
- How to specify output formats for structured data
- What environment variables are available to your actions
- How to write actions for different runtimes (Shell, Python, Node.js)
- Best practices for security and reliability

---

## Action Anatomy

Every action consists of two files:

1. **Metadata file** (`actions/<action_name>.yaml`) - Describes the action
2. **Implementation file** (`actions/<action_name>.<ext>`) - Executes the logic

### Metadata File Structure

```yaml
ref: mypack.my_action
label: "My Action"
description: "Action description"
enabled: true

# Runtime configuration
runner_type: shell              # Runtime to use (shell, python, nodejs, etc.)
entry_point: my_action.sh       # Script to execute

# Optional worker placement constraints
worker_selector:                # exact worker label matches
  zone: us-east-1a
worker_tolerations:             # tolerate matching worker taints
  - key: gpu
    operator: exists
    effect: no_schedule
worker_affinity:                # required/preferred label affinity
  preferred:
    - weight: 50
      preference:
        match_labels:
          disk: ssd

# Parameter configuration (how parameters are delivered)
parameter_delivery: stdin       # Options: stdin (default), file
parameter_format: json          # Options: json (default), yaml, dotenv

# Output configuration (how output is parsed)
output_format: json             # Options: text (default), json, yaml, jsonl

# Parameter schema (JSON Schema format)
# Note: 'required' is an array of required property names
# If no properties are required, omit the 'required' field entirely
parameters:
  type: object
  properties:
    message:
      type: string
      description: "Message to process"
    count:
      type: integer
      description: "Number of times to repeat"
      default: 1
  required:
    - message

# Output schema (documents expected JSON structure)
output_schema:
  type: object
  properties:
    result:
      type: string
      description: "Processed result"
    success:
      type: boolean
      description: "Whether operation succeeded"

tags:
  - utility
```

Action-level worker placement is the default for executions of that action. Manual execution requests can override any of `worker_selector`, `worker_tolerations`, or `worker_affinity` with the same JSON shape; omitted override fields inherit the action default, while explicit `{}` (`worker_selector` / `worker_affinity`) or `[]` (`worker_tolerations`) clears that field for the execution. Workflow tasks support the same fields and render them as workflow templates before creating the child execution.

---

## Parameter Configuration

Parameters are the inputs to your action. Attune provides flexible configuration for how parameters are delivered and formatted.

### Parameter Delivery Methods

#### 1. **Stdin Delivery** (Recommended, Default)

Parameters are passed via standard input. This is the **most secure method** as parameters don't appear in process listings.

```yaml
parameter_delivery: stdin
parameter_format: json
```

**Reading stdin parameters:**

The worker writes a single document to stdin containing all parameters (including secrets merged in), followed by a newline, then closes stdin:

```
<formatted_parameters>\n
```

- Parameters and secrets are merged into a single document
- Secrets are included as top-level keys in the parameters object
- The action reads until EOF (stdin is closed after delivery)

#### 2. **File Delivery**

Parameters are written to a temporary file with restrictive permissions (owner read-only, 0400).

```yaml
parameter_delivery: file
parameter_format: yaml
```

**Reading file parameters:**

The file path is provided in the `ATTUNE_PARAMETER_FILE` environment variable:

```bash
# Shell example
PARAM_FILE="$ATTUNE_PARAMETER_FILE"
params=$(cat "$PARAM_FILE")
```

### Parameter Formats

#### 1. **JSON Format** (Default)

Standard JSON object format.

```yaml
parameter_format: json
```

**Example output:**
```json
{
  "message": "Hello, World!",
  "count": 42,
  "enabled": true
}
```

**Reading JSON (Shell with jq):**
```bash
#!/bin/bash
set -e

# Read JSON from stdin
read -r -d '' PARAMS_JSON || true
MESSAGE=$(echo "$PARAMS_JSON" | jq -r '.message')
COUNT=$(echo "$PARAMS_JSON" | jq -r '.count // 1')
```

**Reading JSON (Python):**
```python
#!/usr/bin/env python3
import json
import sys

# Read all parameters from stdin (secrets are merged in)
content = sys.stdin.read().strip()
params = json.loads(content) if content else {}

message = params.get('message', '')
count = params.get('count', 1)
```

#### 2. **YAML Format**

YAML format, useful for complex nested structures.

```yaml
parameter_format: yaml
```

**Example output:**
```yaml
message: Hello, World!
count: 42
enabled: true
nested:
  key: value
```

**Reading YAML (Python):**
```python
#!/usr/bin/env python3
import sys
import yaml

# Read all parameters from stdin (secrets are merged in)
content = sys.stdin.read().strip()
params = yaml.safe_load(content) if content else {}

message = params.get('message', '')
```

#### 3. **Dotenv Format**

Simple key-value pairs, one per line. Best for shell scripts with simple parameters.

```yaml
parameter_format: dotenv
```

**Example output:**
```bash
message='Hello, World!'
count='42'
enabled='true'
```

**Reading dotenv (Shell):**
```bash
#!/bin/sh
set -e

# Initialize variables
message=""
count=""

# Read until delimiter
while IFS= read -r line; do
    case "$line" in
        message=*) 
            message="${line#message=}"
            # Remove quotes
            message="${message#[\"']}"
            message="${message%[\"']}"
            ;;
        count=*)
            count="${line#count=}"
            count="${count#[\"']}"
            count="${count%[\"']}"
            ;;
    esac
done

echo "Message: $message"
echo "Count: $count"
```

**Example with no required parameters:**
```yaml
# All parameters are optional
parameters:
  type: object
  properties:
    message:
      type: string
      description: "Optional message to log"
      default: "Hello"
    verbose:
      type: boolean
      description: "Enable verbose logging"
      default: false
  # Note: No 'required' field - all parameters are optional
```

### Security Considerations

- **Never use environment variables for secrets** - Environment variables are visible in process listings (`ps aux`)
- **Always use stdin or file delivery for sensitive data** - These methods are not visible to other processes
- **Secrets are always passed via stdin** - Even if parameters use file delivery, secrets come through stdin after the delimiter
- **Parameter files have restrictive permissions** - 0400 (owner read-only)

---

## Output Configuration

Configure how your action's output is parsed and stored in the execution result.

### Output Formats

#### 1. **Text Format** (Default)

No parsing - output is captured as plain text in `execution.stdout`.

```yaml
output_format: text
```

**Use case:** Simple actions that output messages, logs, or unstructured text.

**Example:**
```bash
#!/bin/bash
echo "Task completed successfully"
echo "Processed 42 items"
exit 0
```

**Result:**
- `execution.stdout`: Full text output
- `execution.result`: `null` (no parsing)

#### 2. **JSON Format**

Parses the **last line** of stdout as JSON and stores it in `execution.result`.

```yaml
output_format: json
```

**Use case:** Actions that return structured data, API responses, or computed results.

**Example:**
```bash
#!/bin/bash
# Your action logic here
curl -s https://api.example.com/data

# Output JSON as last line (curl already outputs JSON)
# The worker will parse this into execution.result
exit 0
```

**Example (manual JSON):**
```python
#!/usr/bin/env python3
import json

result = {
    "status": "success",
    "items_processed": 42,
    "duration_ms": 1234
}

# Output JSON - will be parsed into execution.result
print(json.dumps(result, indent=2))
```

**Result:**
- `execution.stdout`: Full output including JSON
- `execution.result`: Parsed JSON object from last line

#### 3. **YAML Format**

Parses the entire stdout as YAML.

```yaml
output_format: yaml
```

**Use case:** Actions that generate YAML configuration or reports.

**Example:**
```python
#!/usr/bin/env python3
import yaml

result = {
    "status": "success",
    "config": {
        "enabled": True,
        "timeout": 30
    }
}

print(yaml.dump(result))
```

#### 4. **JSONL Format** (JSON Lines)

Parses each line of stdout as a separate JSON object and collects them into an array.

```yaml
output_format: jsonl
```

**Use case:** Streaming results, processing multiple items, progress updates.

**Example:**
```python
#!/usr/bin/env python3
import json

# Process items and output each as JSON
for i in range(5):
    item = {"id": i, "status": "processed"}
    print(json.dumps(item))  # Each line is valid JSON
```

**Result:**
- `execution.result`: Array of parsed JSON objects

---

## Standard Environment Variables

The worker provides these environment variables to **all** action executions:

### Core Variables (Always Present)

| Variable | Description | Example |
|----------|-------------|---------|
| `ATTUNE_EXEC_ID` | Execution database ID | `12345` |
| `ATTUNE_ACTION` | Action reference (pack.action) | `core.echo` |
| `ATTUNE_API_URL` | Attune API base URL | `http://api:8080` |
| `ATTUNE_API_TOKEN` | Execution-scoped API token | `eyJ0eXAi...` |

### Contextual Variables (When Applicable)

| Variable | Description | Present When |
|----------|-------------|--------------|
| `ATTUNE_RULE` | Rule reference that triggered execution | Execution triggered by rule |
| `ATTUNE_TRIGGER` | Trigger reference that fired | Execution triggered by event |
| `ATTUNE_CONTEXT_*` | Custom context data | Context provided in execution config |

### Parameter Delivery Variables

| Variable | Description | Present When |
|----------|-------------|--------------|
| `ATTUNE_PARAMETER_DELIVERY` | Delivery method used | Always (`stdin` or `file`) |
| `ATTUNE_PARAMETER_FORMAT` | Format used | Always (`json`, `yaml`, or `dotenv`) |
| `ATTUNE_PARAMETER_FILE` | Path to parameter file | `parameter_delivery: file` |

### Using Environment Variables

**Shell:**
```bash
#!/bin/bash
set -e

echo "Execution ID: $ATTUNE_EXEC_ID"
echo "Action: $ATTUNE_ACTION"
echo "API URL: $ATTUNE_API_URL"

# Check if triggered by rule
if [ -n "$ATTUNE_RULE" ]; then
    echo "Triggered by rule: $ATTUNE_RULE"
    echo "Trigger: $ATTUNE_TRIGGER"
fi

# Use API token for authenticated requests
curl -H "Authorization: Bearer $ATTUNE_API_TOKEN" \
     "$ATTUNE_API_URL/api/executions/$ATTUNE_EXEC_ID"
```

**Python:**
```python
#!/usr/bin/env python3
import os
import requests

exec_id = os.environ['ATTUNE_EXEC_ID']
action_ref = os.environ['ATTUNE_ACTION']
api_url = os.environ['ATTUNE_API_URL']
api_token = os.environ['ATTUNE_API_TOKEN']

print(f"Execution {exec_id} running action {action_ref}")

# Make authenticated API request
headers = {'Authorization': f'Bearer {api_token}'}
response = requests.get(f"{api_url}/api/executions/{exec_id}", headers=headers)
print(response.json())
```

### Custom Environment Variables

You can also set custom environment variables per execution via the `env_vars` field:

```json
{
  "action_ref": "core.my_action",
  "parameters": {
    "message": "Hello"
  },
  "env_vars": {
    "DEBUG": "true",
    "LOG_LEVEL": "verbose"
  }
}
```

**Note:** These are separate from parameters and passed as actual environment variables.

---

## Runtime Configuration

Configure which runtime executes your action.

### Available Runtimes

| Runtime | `runner_type` | Description | Entry Point |
|---------|---------------|-------------|-------------|
| Shell | `shell` | POSIX shell scripts | Script file (`.sh`) |
| Python | `python` | Python 3.x scripts | Script file (`.py`) |
| Node.js | `nodejs` | JavaScript/Node.js | Script file (`.js`) |
| Native | `native` | Compiled binaries | Binary file |
| Local | `local` | Local system commands | Command name |

### Shell Runtime

Execute shell scripts using `/bin/sh` (or configurable shell).

```yaml
runner_type: shell
entry_point: my_script.sh
```

**Script requirements:**
- Must be executable or have a shebang (`#!/bin/sh`)
- Exit code 0 indicates success
- Output to stdout for results
- Errors to stderr

**Example:**
```bash
#!/bin/sh
set -e  # Exit on error

# Read parameters from stdin
content=$(cat)
params=$(echo "$content" | head -n 1)

# Process
echo "Processing: $params"

# Output result
echo '{"status": "success"}'
exit 0
```

### Python Runtime

Execute Python scripts with automatic virtual environment management.

```yaml
runner_type: python
entry_point: my_script.py
```

**Features:**
- Automatic virtual environment creation
- Dependency installation from `requirements.txt`
- Python 3.x support
- Access to all standard libraries

**Example:**
```python
#!/usr/bin/env python3
import json
import sys

def main():
    # Read parameters (secrets are merged into the same document)
    content = sys.stdin.read().strip()
    params = json.loads(content) if content else {}
    
    # Process
    message = params.get('message', '')
    result = message.upper()
    
    # Output
    print(json.dumps({
        'result': result,
        'success': True
    }))
    return 0

if __name__ == '__main__':
    sys.exit(main())
```

**Dependencies (optional `requirements.txt` in action directory):**
```txt
requests>=2.28.0
pyyaml>=6.0
```

### Node.js Runtime

Execute JavaScript with Node.js.

```yaml
runner_type: nodejs
entry_point: my_script.js
```

**Example:**
```javascript
#!/usr/bin/env node
const readline = require('readline');

async function main() {
    // Read stdin
    const rl = readline.createInterface({
        input: process.stdin,
        output: process.stdout,
        terminal: false
    });

    let input = '';
    for await (const line of rl) {
        input += line;
    }

    const params = JSON.parse(input || '{}');
    
    // Process
    const result = {
        message: params.message.toUpperCase(),
        success: true
    };

    // Output
    console.log(JSON.stringify(result, null, 2));
}

main().catch(err => {
    console.error(err);
    process.exit(1);
});
```

### Native Runtime

Execute compiled binaries (sensors, performance-critical actions).

```yaml
runner_type: native
entry_point: my_binary
```

**Use case:** Compiled sensors, performance-critical operations.

**Requirements:**
- Binary must be executable
- Built for target architecture (see `scripts/build-pack-binaries.sh`)
- Follow same stdin/stdout conventions

---

## Complete Examples

### Example 1: Simple Text Action

**Metadata (`actions/greet.yaml`):**
```yaml
ref: mypack.greet
label: "Greet User"
description: "Greet a user by name"
runner_type: shell
entry_point: greet.sh
parameter_delivery: stdin
parameter_format: json
output_format: text

parameters:
  type: object
  properties:
    name:
      type: string
      description: "Name to greet"
    formal:
      type: boolean
      description: "Use formal greeting"
      default: false
  required:
    - name
```

**Implementation (`actions/greet.sh`):**
```bash
#!/bin/bash
set -e

# Read JSON parameters
read -r -d '' PARAMS_JSON || true
NAME=$(echo "$PARAMS_JSON" | jq -r '.name')
FORMAL=$(echo "$PARAMS_JSON" | jq -r '.formal // false')

# Generate greeting
if [ "$FORMAL" = "true" ]; then
    echo "Good day, $NAME. It is a pleasure to meet you."
else
    echo "Hey $NAME! What's up?"
fi

exit 0
```

### Example 2: HTTP API Action with JSON Output

**Metadata (`actions/fetch_user.yaml`):**
```yaml
ref: mypack.fetch_user
label: "Fetch User"
description: "Fetch user data from API"
runner_type: shell
entry_point: fetch_user.sh
parameter_delivery: stdin
parameter_format: json
output_format: json

parameters:
  type: object
  properties:
    user_id:
      type: integer
      description: "User ID to fetch"
    include_posts:
      type: boolean
      description: "Include user posts"
      default: false
  required:
    - user_id

output_schema:
  type: object
  properties:
    user:
      type: object
      description: "User data"
    posts:
      type: array
      description: "User posts (if requested)"
    success:
      type: boolean
```

**Implementation (`actions/fetch_user.sh`):**
```bash
#!/bin/bash
set -e

# Read parameters
read -r -d '' PARAMS_JSON || true
USER_ID=$(echo "$PARAMS_JSON" | jq -r '.user_id')
INCLUDE_POSTS=$(echo "$PARAMS_JSON" | jq -r '.include_posts // false')

# Fetch user
USER_DATA=$(curl -s "https://jsonplaceholder.typicode.com/users/$USER_ID")

# Build result
RESULT="{\"user\": $USER_DATA"

# Optionally fetch posts
if [ "$INCLUDE_POSTS" = "true" ]; then
    POSTS=$(curl -s "https://jsonplaceholder.typicode.com/users/$USER_ID/posts")
    RESULT="$RESULT, \"posts\": $POSTS"
fi

RESULT="$RESULT, \"success\": true}"

# Output JSON (will be parsed into execution.result)
echo "$RESULT"
exit 0
```

### Example 3: Python Action with Secrets

**Metadata (`actions/send_email.yaml`):**
```yaml
ref: mypack.send_email
label: "Send Email"
description: "Send email via SMTP"
runner_type: python
entry_point: send_email.py
parameter_delivery: stdin
parameter_format: json
output_format: json

parameters:
  type: object
  properties:
    to:
      type: string
      description: "Recipient email"
    subject:
      type: string
      description: "Email subject"
    body:
      type: string
      description: "Email body"
    smtp_password:
      type: string
      description: "SMTP password"
      secret: true
  required:
    - to
    - subject
    - body
```

**Implementation (`actions/send_email.py`):**
```python
#!/usr/bin/env python3
import json
import sys
import smtplib
from email.mime.text import MIMEText

def read_stdin_params():
    """Read parameters from stdin. Secrets are already merged into the parameters."""
    content = sys.stdin.read().strip()
    return json.loads(content) if content else {}

def main():
    try:
        params = read_stdin_params()
        
        # Extract parameters
        to = params['to']
        subject = params['subject']
        body = params['body']
        smtp_password = params.get('smtp_password', '')
        
        # Create message
        msg = MIMEText(body)
        msg['Subject'] = subject
        msg['From'] = 'noreply@example.com'
        msg['To'] = to
        
        # Send (example - configure for your SMTP server)
        # with smtplib.SMTP('smtp.example.com', 587) as server:
        #     server.starttls()
        #     server.login('user', smtp_password)
        #     server.send_message(msg)
        
        # Simulate success
        result = {
            'success': True,
            'to': to,
            'subject': subject,
            'message': 'Email sent successfully'
        }
        
        print(json.dumps(result, indent=2))
        return 0
        
    except Exception as e:
        result = {
            'success': False,
            'error': str(e)
        }
        print(json.dumps(result, indent=2))
        return 1

if __name__ == '__main__':
    sys.exit(main())
```

### Example 4: Multi-Item Processing with JSONL

**Metadata (`actions/process_items.yaml`):**
```yaml
ref: mypack.process_items
label: "Process Items"
description: "Process multiple items and stream results"
runner_type: python
entry_point: process_items.py
parameter_delivery: stdin
parameter_format: json
output_format: jsonl

parameters:
  type: object
  properties:
    items:
      type: array
      items:
        type: string
      description: "Items to process"
  required:
    - items
```

**Implementation (`actions/process_items.py`):**
```python
#!/usr/bin/env python3
import json
import sys
import time

def main():
    # Read parameters (secrets are merged into the same document)
    content = sys.stdin.read().strip()
    params = json.loads(content) if content else {}
    
    items = params.get('items', [])
    
    # Process each item and output immediately (streaming)
    for idx, item in enumerate(items):
        # Simulate processing
        time.sleep(0.1)
        
        # Output one JSON object per line
        result = {
            'index': idx,
            'item': item,
            'processed': item.upper(),
            'timestamp': time.time()
        }
        print(json.dumps(result))  # Each line is valid JSON
        sys.stdout.flush()  # Ensure immediate output
    
    return 0

if __name__ == '__main__':
    sys.exit(main())
```

### Example 5: Shell Action with Dotenv Parameters

**Metadata (`actions/backup.yaml`):**
```yaml
ref: mypack.backup
label: "Backup Files"
description: "Backup files to destination"
runner_type: shell
entry_point: backup.sh
parameter_delivery: stdin
parameter_format: dotenv
output_format: text

parameters:
  type: object
  properties:
    source:
      type: string
      description: "Source directory"
    destination:
      type: string
      description: "Backup destination"
    compress:
      type: boolean
      description: "Compress backup"
      default: true
  required:
    - source
    - destination
```

**Implementation (`actions/backup.sh`):**
```bash
#!/bin/sh
set -e

# Initialize variables
source=""
destination=""
compress="true"

# Read dotenv format from stdin
while IFS= read -r line; do
    case "$line" in
        source=*)
            source="${line#source=}"
            source="${source#[\"\']}"
            source="${source%[\"\']}"
            ;;
        destination=*)
            destination="${line#destination=}"
            destination="${destination#[\"\']}"
            destination="${destination%[\"\']}"
            ;;
        compress=*)
            compress="${line#compress=}"
            compress="${compress#[\"\']}"
            compress="${compress%[\"\']}"
            ;;
    esac
done

echo "Backing up $source to $destination"

# Perform backup
if [ "$compress" = "true" ]; then
    tar -czf "$destination/backup.tar.gz" -C "$source" .
    echo "Compressed backup created at $destination/backup.tar.gz"
else
    cp -r "$source" "$destination"
    echo "Backup copied to $destination"
fi

exit 0
```

---

## Best Practices

### Security

1. **Never log secrets** - Don't echo parameters that might contain secrets
2. **Use stdin for sensitive data** - Avoid file delivery for credentials
3. **Validate inputs** - Check parameters before using them
4. **Use API token** - Authenticate API requests with `$ATTUNE_API_TOKEN`
5. **Restrict file permissions** - Keep temporary files secure

### Reliability

1. **Exit codes matter** - Return 0 for success, non-zero for failure
2. **Handle errors gracefully** - Use `set -e` in shell scripts
3. **Provide meaningful errors** - Write error details to stderr
4. **Set timeouts** - Avoid infinite loops or hangs
5. **Clean up resources** - Remove temporary files

### Output

1. **Structure output properly** - Match your `output_format` setting
2. **Validate JSON** - Ensure JSON output is valid
3. **Use stderr for logs** - Reserve stdout for results
4. **Keep output concise** - Large outputs are truncated (10MB default)
5. **Document output schema** - Define `output_schema` in metadata

### Performance

1. **Minimize dependencies** - Fewer dependencies = faster execution
2. **Use appropriate runtime** - Shell for simple tasks, Python for complex logic
3. **Stream large results** - Use JSONL for incremental output
4. **Avoid unnecessary work** - Check prerequisites early

### Maintainability

1. **Document parameters** - Provide clear descriptions
2. **Provide examples** - Include usage examples in metadata
3. **Version your actions** - Use pack versioning
4. **Test thoroughly** - Test with various parameter combinations
5. **Handle edge cases** - Empty inputs, missing optional parameters

---

## Troubleshooting

### My action isn't receiving parameters

**Check:**
- Is `parameter_delivery` set correctly?
- Are you reading from stdin or checking `$ATTUNE_PARAMETER_FILE`?
- Are you reading stdin until EOF?

**Debug:**
```bash
# Dump stdin to stderr for debugging
cat > /tmp/debug_stdin.txt
cat /tmp/debug_stdin.txt >&2
```

### My JSON output isn't being parsed

**Check:**
- Is `output_format: json` set in metadata?
- Is the last line of stdout valid JSON?
- Are you outputting anything else to stdout?

**Debug:**
```python
import json
result = {"test": "value"}
print(json.dumps(result))  # Ensure this is last line
```

### My action times out

**Check:**
- Default timeout is 5 minutes (300 seconds)
- Is your action hanging or waiting for input?
- Are you flushing output buffers?

**Fix:**
```python
print(result)
sys.stdout.flush()  # Ensure output is written immediately
```

### Secrets aren't available

**Check:**
- Are secrets configured for the action?
- Secrets are merged into the parameters document — access them by key name just like regular parameters

**Example:**
```python
content = sys.stdin.read().strip()
params = json.loads(content) if content else {}
api_key = params.get('api_key', '')  # Secrets are regular keys
```

### Environment variables are missing

**Check:**
- Standard variables (`ATTUNE_EXEC_ID`, etc.) are always present
- Context variables (`ATTUNE_RULE`, etc.) are conditional
- Custom `env_vars` must be set in execution config

**Debug:**
```bash
env | grep ATTUNE >&2  # Print all ATTUNE_* variables to stderr
```

### Output is truncated

**Cause:** Default limit is 10MB for stdout/stderr

**Solution:**
- Reduce output size
- Use artifacts for large data
- Stream results using JSONL format

---

## Additional Resources

- [Pack Structure Documentation](pack-structure.md)
- [Worker Service Architecture](architecture/worker-service.md)
- [Secrets Management](authentication/secrets-management.md)
- [Testing Actions](packs/PACK_TESTING.md)

---

## Quick Reference Card

| Configuration | Options | Default | Description |
|--------------|---------|---------|-------------|
| `runner_type` | `shell`, `python`, `nodejs`, `native`, `local` | Required | Runtime to execute action |
| `parameter_delivery` | `stdin`, `file` | `stdin` | How parameters are delivered |
| `parameter_format` | `json`, `yaml`, `dotenv` | `json` | Format for parameter serialization |
| `output_format` | `text`, `json`, `yaml`, `jsonl` | `text` | How to parse stdout output |

### Standard Environment Variables

- `ATTUNE_EXEC_ID` - Execution database ID
- `ATTUNE_ACTION` - Action reference (pack.action)
- `ATTUNE_API_URL` - API base URL
- `ATTUNE_API_TOKEN` - Execution-scoped token
- `ATTUNE_RULE` - Rule ref (if triggered by rule)
- `ATTUNE_TRIGGER` - Trigger ref (if triggered by event)

### Exit Codes

- `0` - Success
- Non-zero - Failure (error message from stderr)

### Output Parsing

- **Text**: No parsing, captured in `execution.stdout`
- **JSON**: Last line parsed into `execution.result`
- **YAML**: Full output parsed into `execution.result`
- **JSONL**: Each line parsed, collected into array in `execution.result`

## Calling an installed binary with arguments

A native action can call a worker-installed program directly. Define fixed
arguments under `launch.arguments`, and dynamic bindings on their parameter
schema fields. One global `order` determines the sequence.

```yaml
ref: tools.print_message
label: Print message
runner_type: native
launch:
  type: command
  program: printf
  arguments:
    - literal: "%s\n"
      order: 0
parameter_delivery: none
output_format: text
parameters:
  message:
    type: string
    default: hello
    cli:
      kind: positional
      order: 10
```

`attune action execute tools.print_message --param 'message=hello world'` passes
`hello world` as one argument. Use `cli: {kind: option, name: --output, order: 20}`
for an option/value pair, or `kind: flag` for a Boolean switch. Nested bindings
belong on explicit `properties` definitions; author-facing paths use dot/bracket
notation. Static subcommands, flags and `--` separators are literal entries.

For a binary distributed in the pack, use `launch: {type: native, artifact: tool}`
and publish the verified native artifact with the pack builder. Both launch
forms require a direct native runtime. Existing `entry_point` actions continue
working without `launch`.

Commands default to no input document. Use `parameter_delivery: stdin` to send
the complete flat JSON map as well as declared arguments. Secrets cannot bind to
argv. With `none`, any concrete protected input fails before launch, including
unbound fields. Optional secrets can use stdin JSON. CLI invocations support
JSON input only and never expand shell expressions. Constrain values that the
target program interprets as paths, code or response files. Deployments must
protect installed programs and search directories against untrusted writes.

Action detail responses include a definition-only `argument_preview` in derived
order. The web configuration dialog exposes launch/input schema JSON and this
saved preview. CLI JSON/YAML output preserves the same metadata. Replace sources
atomically with:

```bash
attune action update tools.print_message --clear-entrypoint \
  --launch-file launch.yaml --parameters-file parameters.yaml \
  --parameter-delivery none --parameter-format json
```

Use `--clear-launch --entrypoint run` with a compatible schema and explicit
`--parameter-delivery stdin` when switching back. Omitted patch fields preserve
stored settings. Supporting Linux workers advertise `action_cli_arguments_v1`;
use placement constraints for tools installed only on selected workers.
See the [full metadata specification](plans/action-command-line-metadata.md)
for conversion rules, limits, diagnostics and security responsibilities.
