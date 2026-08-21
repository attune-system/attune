# Pack Structure Documentation

**Last Updated**: 2024-01-20  
**Status**: Reference Documentation

---

## Overview

Attune packs are bundles of automation components (actions, sensors, triggers, rules, workflows) organized in a standardized directory structure. This document defines the canonical pack structure and file formats.

---

## Pack Directory Structure

```
packs/<pack_name>/
├── pack.yaml                    # Pack manifest (required)
├── README.md                    # Pack documentation (recommended)
├── CHANGELOG.md                 # Version history (recommended)
├── LICENSE                      # License file (recommended)
├── requirements.txt             # Python dependencies (optional)
├── package.json                 # Node.js dependencies (optional)
├── actions/                     # Action definitions
│   ├── <action_name>.yaml      # Action metadata
│   ├── <action_name>.sh        # Shell action implementation
│   ├── <action_name>.py        # Python action implementation
│   └── <action_name>.js        # Node.js action implementation
├── sensors/                     # Sensor definitions
│   ├── <sensor_name>.yaml      # Sensor metadata
│   ├── <sensor_name>.py        # Python sensor implementation
│   └── <sensor_name>.js        # Node.js sensor implementation
├── caches/                      # Declarative Data Cache namespaces (optional)
│   └── <cache_name>.yaml       # Namespace owner and mutable policy
├── triggers/                    # Trigger type definitions
│   └── <trigger_name>.yaml     # Trigger metadata
├── rules/                       # Rule definitions (optional)
│   └── <rule_name>.yaml        # Rule metadata
├── workflows/                   # Workflow definitions (optional)
│   └── <workflow_name>.yaml    # Workflow metadata
├── policies/                    # Policy definitions (optional)
│   └── <policy_name>.yaml      # Policy metadata
├── config.schema.yaml           # Pack configuration schema (optional)
├── icon.png                     # Pack icon (optional)
└── tests/                       # Tests (optional)
    ├── test_actions.py
    └── test_sensors.py
```

---

## File Formats

### Pack Manifest (`pack.yaml`)

The pack manifest is the main metadata file for a pack. It defines the pack's identity, version, dependencies, and configuration.

**Required Fields:**
- `ref` (string): Unique pack reference/identifier. It must start with a lowercase ASCII letter and contain only lowercase ASCII letters, digits, hyphens, or underscores. Dots are not allowed.
- `label` (string): Human-readable pack name
- `description` (string): Brief description of the pack
- `version` (string): Semantic version (e.g., "1.0.0")

**Optional Fields:**
- `author` (string): Pack author name
- `email` (string): Author email
- `system` (boolean): Whether this is a system pack (default: false)
- `enabled` (boolean): Whether pack is enabled by default (default: true)
- `conf_schema` (object): JSON Schema for pack configuration
- `config` (object): Default pack configuration
- `meta` (object): Additional metadata
- `tags` (array): Tags for categorization
- `runtime_deps` (array): Runtime dependencies (e.g., "python3", "nodejs", "shell")
- `worker_selector` (object): Mandatory exact labels for workers that run this pack.
- `worker_tolerations` (array): Mandatory tolerations for worker taints.
- `worker_affinity` (object): Mandatory required, preferred, and anti-affinity rules.

Pack-level placement is inherited by every action, sensor, execution, and pack
test. Child placement can narrow worker eligibility but cannot clear or weaken
the pack's requirements. A pack Dockerfile or deployment recipe is operator
documentation only. Pack registration never builds or deploys it.

Pack content must contain regular files and directories only. Symlinks, hard
links, devices, and FIFOs are rejected during upload, archive installation,
checksum calculation, and storage copy. Component refs must be qualified by the
manifest pack ref, and installation cannot replace components owned by another
pack or reserved permission sets such as `standard`.

### Custom workers

Operators run custom workers with labels and taints that satisfy the pack's
placement. An action worker that can run a GPU-bound pack, for example, needs
matching worker configuration:

```yaml
worker:
  labels:
    accelerator: nvidia
  taints:
    - key: accelerator
      value: nvidia
      effect: no_schedule
```

The pack must select `accelerator: nvidia` and include a matching toleration.
Start the worker normally with this configuration. Registration only records
pack metadata and components. It never builds images, starts workers, or runs a
pack Dockerfile or deployment recipe.

**Example:**

```yaml
ref: core
label: "Core Pack"
description: "Built-in core functionality including timer triggers and basic actions"
version: "1.0.0"
author: "Attune Team"
email: "core@attune.io"
system: true
enabled: true

conf_schema:
  type: object
  properties:
    max_action_timeout:
      type: integer
      description: "Maximum timeout for action execution in seconds"
      default: 300
      minimum: 1
      maximum: 3600

config:
  max_action_timeout: 300

meta:
  category: "system"
  keywords:
    - "core"
    - "utilities"
  python_dependencies:
    - "requests>=2.28.0"
  documentation_url: "https://docs.attune.io/packs/core"
  repository_url: "https://github.com/attune-io/attune"

tags:
  - core
  - system
  - utilities

runtime_deps:
  - shell
  - python3
```

---

### Action Metadata (`actions/<action_name>.yaml`)

Action metadata files define the parameters, output schema, and execution details for actions.

**Required Fields:**
- `ref` (string): Full action reference (e.g., "core.echo")
- `label` (string): Human-readable action name
- `description` (string): Action description
- `runner_type` (string): Execution runtime (shell, python, nodejs, docker)
- `entry_point` (string): Script filename to execute

**Optional Fields:**
- `enabled` (boolean): Whether action is enabled (default: true)
- `parameters` (object): Parameter definitions (JSON Schema style)
- `output_schema` (object): Output schema definition
- `parameter_delivery` (string): How parameters are delivered - `env` (environment variables), `stdin` (standard input), or `file` (temporary file). Default: `env`. **Security Note**: Use `stdin` or `file` for actions with sensitive parameters.
- `parameter_format` (string): Parameter serialization format - `dotenv` (KEY='VALUE'), `json` (JSON object), or `yaml` (YAML format). Default: `dotenv`
- `tags` (array): Tags for categorization
- `timeout` (integer): Default timeout in seconds
- `log_retention_policy` (string): Optional per-action stdout/stderr artifact retention override (`versions`, `days`, `hours`, or `minutes`)
- `log_retention_limit` (integer): Optional per-action stdout/stderr artifact retention limit override
- `examples` (array): Usage examples

**Example:**

```yaml
ref: core.echo
label: "Echo"
description: "Echo a message to stdout"
enabled: true
runner_type: shell
entry_point: echo.sh

# Optional worker placement constraints
worker_selector:
  zone: us-east-1a
worker_tolerations:
  - key: gpu
    operator: exists
    effect: no_schedule
worker_affinity:
  preferred:
    - weight: 50
      preference:
        match_labels:
          disk: ssd

# Parameter delivery (optional, defaults to stdin/json)
parameter_delivery: stdin
parameter_format: json

# Optional action log artifact retention override
log_retention_policy: versions
log_retention_limit: 4

parameters:
  type: object
  properties:
    message:
      type: string
      description: "Message to echo"
      default: "Hello, World!"
    uppercase:
      type: boolean
      description: "Convert message to uppercase"
      default: false

output_schema:
  type: object
  properties:
    stdout:
      type: string
      description: "Standard output from the command"
    exit_code:
      type: integer
      description: "Exit code (0 = success)"

tags:
  - utility
  - testing
```

Worker placement in action YAML adds requirements to the pack's placement. Manual
executions and workflow tasks may add their own constraints, but empty values
only clear that child layer. They never clear the pack or action requirements.

---

### Action Implementation

Actions receive parameters according to the `parameter_delivery` method specified in their metadata:

- **`env`** (default): Parameters as environment variables prefixed with `ATTUNE_ACTION_`
- **`stdin`**: Parameters via standard input in the specified format
- **`file`**: Parameters in a temporary file (path in `ATTUNE_PARAMETER_FILE` env var)

**Security Warning**: Environment variables are visible in process listings. Use `stdin` or `file` for sensitive data.

**Shell Example with Environment Variables** (`actions/echo.sh`):

```bash
#!/bin/bash
set -e

# Parse parameters from environment variables
MESSAGE="${ATTUNE_ACTION_MESSAGE:-Hello, World!}"
UPPERCASE="${ATTUNE_ACTION_UPPERCASE:-false}"

# Convert to uppercase if requested
if [ "$UPPERCASE" = "true" ]; then
    MESSAGE=$(echo "$MESSAGE" | tr '[:lower:]' '[:upper:]')
fi

# Echo the message
echo "$MESSAGE"

# Exit successfully
exit 0
```

**Shell Example with Stdin/JSON** (more secure):

```bash
#!/bin/bash
set -e

# Read parameters from stdin (JSON format)
read -r PARAMS_JSON
MESSAGE=$(echo "$PARAMS_JSON" | jq -r '.message // "Hello, World!"')
UPPERCASE=$(echo "$PARAMS_JSON" | jq -r '.uppercase // "false"')

# Convert to uppercase if requested
if [ "$UPPERCASE" = "true" ]; then
    MESSAGE=$(echo "$MESSAGE" | tr '[:lower:]' '[:upper:]')
fi

echo "$MESSAGE"
exit 0
```

**Python Example with Stdin/JSON** (recommended for security):

```python
#!/usr/bin/env python3
import json
import sys

def read_stdin_params():
    """Read parameters from stdin. Secrets are already merged into parameters."""
    content = sys.stdin.read().strip()
    return json.loads(content) if content else {}

def main():
    params = read_stdin_params()
    url = params.get("url")
    method = params.get("method", "GET")
    
    if not url:
        print(json.dumps({"error": "url parameter required"}))
        sys.exit(1)
    
    # Perform action logic
    result = {
        "url": url,
        "method": method,
        "success": True
    }
    
    # Output result as JSON
    print(json.dumps(result, indent=2))
    sys.exit(0)

if __name__ == "__main__":
    main()
```

**Python Example with Environment Variables** (legacy, less secure):

```python
#!/usr/bin/env python3
import json
import os
import sys

def get_env_param(name: str, default=None):
    """Get action parameter from environment variable."""
    env_key = f"ATTUNE_ACTION_{name.upper()}"
    return os.environ.get(env_key, default)

def main():
    url = get_env_param("url")
    method = get_env_param("method", "GET")
    
    if not url:
        print(json.dumps({"error": "url parameter required"}))
        sys.exit(1)
    
    # Perform action logic
    result = {
        "url": url,
        "method": method,
        "success": True
    }
    
    # Output result as JSON
    print(json.dumps(result, indent=2))
    sys.exit(0)

if __name__ == "__main__":
    main()
```

---

### Sensor Metadata (`sensors/<sensor_name>.yaml`)

Sensor metadata files define sensors that monitor for events and fire triggers.

**Required Fields:**
- `ref` (string): Full sensor reference (e.g., "core.interval_timer_sensor")
- `label` (string): Human-readable sensor name
- `description` (string): Sensor description
- `runner_type` (string): Execution runtime (python, nodejs)
- `entry_point` (string): Script filename to execute
- `trigger_types` (array): List of trigger types this sensor monitors

**Optional Fields:**
- `enabled` (boolean): Whether sensor is enabled (default: true)
- `parameters` (object): Sensor configuration parameters
- `poll_interval` (integer): Poll interval in seconds
- `log_retention_policy` (string): Optional per-sensor stdout/stderr log artifact retention override (`versions`, `days`, `hours`, or `minutes`)
- `log_retention_limit` (integer): Optional per-sensor stdout/stderr log artifact retention limit override
- `tags` (array): Tags for categorization
- `meta` (object): Additional metadata

**Example:**

```yaml
ref: core.interval_timer_sensor
label: "Interval Timer Sensor"
description: "Monitors time and fires interval timer triggers"
enabled: true
runner_type: python
entry_point: interval_timer_sensor.py

# Optional sensor log artifact retention override
log_retention_policy: versions
log_retention_limit: 4

trigger_types:
  - core.intervaltimer

parameters:
  check_interval_seconds:
    type: integer
    description: "How often to check if triggers should fire"
    default: 1
    minimum: 1
    maximum: 60

poll_interval: 1

tags:
  - timer
  - system
  - builtin

meta:
  builtin: true
  system: true
```

---

### Sensor Implementation

Sensors run continuously and emit events to stdout as JSON, one per line.

**Python Example (`sensors/interval_timer_sensor.py`):**

```python
#!/usr/bin/env python3
import json
import time
from datetime import datetime

def check_triggers():
    """Check configured triggers and return events to fire."""
    # Load trigger instances from environment
    # Check if any should fire
    # Return list of events
    return []

def main():
    while True:
        events = check_triggers()
        
        # Output events as JSON (one per line)
        for event in events:
            print(json.dumps(event))
            sys.stdout.flush()
        
        # Sleep until next check
        time.sleep(1)

if __name__ == "__main__":
    main()
```

---

### Data Cache Namespace (`caches/<cache_name>.yaml`)

Cache files declaratively create owner-scoped Data Cache namespaces after
actions and sensors are loaded. They declare cache policy and ownership, not
authoritative data. Every populated namespace must be a reconstructable
snapshot of an independent source; credentials for that source remain in Keys
and Secrets. The schema is flat:

```yaml
ref: salesforce.users
namespace: users
owner_type: action
owner_ref: salesforce.refresh_users
freshness_target_seconds: 3600
max_records_per_generation: 200000
max_generation_bytes: 536870912
max_retained_bytes: 2147483648
max_retained_generations: 5
max_staging_generations: 2
```

Required fields:

- `ref`: stable, pack-qualified definition ref. This identifies the
  declarative definition across pack updates.
- `namespace`: lowercase namespace matching
  `[a-z0-9][a-z0-9._-]{0,127}`.
- `owner_type`: `pack`, `action`, or `sensor`. Pack files cannot declare
  `system`- or `identity`-owned namespaces.
- `owner_ref`: the installing pack ref or a full action/sensor ref belonging
  to that pack. Cross-pack owners are rejected.

All policy fields shown above are optional and use those values as defaults.
The definition ref, namespace, and owner are immutable. Policy-only updates
preserve the live namespace ID, active generation, and retained data.

Actions, workflow tasks, and managed sensors fetch records deliberately through
the authenticated cache API. Pack installation does not inject cache values
into action parameters, sensor configuration, or secret stdin, and cache files
must not be used to embed records or secrets.

Removing a cache file tombstones only the namespace managed by that definition;
API-created namespaces are not treated as pack definitions. Tombstoning makes
the namespace unreadable immediately and leaves generation/entry reclamation
to the supervisor. Re-adding the same definition before the old row drains
creates a new live namespace rather than resurrecting the tombstoned row.
Deleting an owning action, sensor, or pack tombstones its namespaces before
the component is removed.

---

### Trigger Metadata (`triggers/<trigger_name>.yaml`)

Trigger metadata files define event types that sensors can fire.

**Required Fields:**
- `ref` (string): Full trigger reference (e.g., "core.intervaltimer")
- `label` (string): Human-readable trigger name
- `description` (string): Trigger description
- `type` (string): Trigger type (interval, cron, one_shot, webhook, custom)

**Optional Fields:**
- `enabled` (boolean): Whether trigger is enabled (default: true)
- `reference_visibility` (string): Which packs may create rules that subscribe to this trigger: `public` (default), `private`, or `restricted`
- `reference_allowed_pack_refs` (array): Additional pack refs allowed to subscribe when `reference_visibility: restricted`; the trigger's own pack is always allowed
- `parameters_schema` (object): Schema for trigger instance configuration
- `payload_schema` (object): Schema for event payload
- `tags` (array): Tags for categorization
- `examples` (array): Configuration examples

**Example:**

```yaml
ref: core.intervaltimer
label: "Interval Timer"
description: "Fires at regular intervals"
enabled: true
type: interval
reference_visibility: restricted
reference_allowed_pack_refs:
  - automation_examples
  - incident_response

parameters_schema:
  type: object
  properties:
    unit:
      type: string
      enum: [seconds, minutes, hours]
      description: "Time unit for the interval"
    interval:
      type: integer
      minimum: 1
      description: "Number of time units between triggers"
  required: [unit, interval]

payload_schema:
  type: object
  properties:
    type:
      type: string
      const: interval
    interval_seconds:
      type: integer
    fired_at:
      type: string
      format: date-time
  required: [type, interval_seconds, fired_at]

tags:
  - timer
  - interval

examples:
  - description: "Fire every 10 seconds"
    parameters:
      unit: "seconds"
      interval: 10
```

---

## Pack Loading Process

When a pack is loaded, Attune performs the following steps:

1. **Parse Pack Manifest**: Read and validate `pack.yaml`
2. **Register Pack**: Insert pack metadata into database
3. **Load Permission Sets**: Parse all `permission_sets/*.yaml` files
4. **Load Runtimes**: Parse all `runtimes/*.yaml` files
5. **Load Triggers**: Parse all `triggers/*.yaml` files
6. **Load Actions and Workflow Definitions**: Parse `actions/*.yaml` files and any `workflow_file` graphs they reference
7. **Load Dashboards**: Parse all `dashboards/*.yaml` files
8. **Load Work Queues**: Parse all `queues/*.yaml` files
9. **Load Policies**: Parse all `policies/*.yaml` files
10. **Load Rules**: Parse all `rules/*.yaml` files after actions and triggers are available
11. **Load Sensors**: Parse all `sensors/*.yaml` files
12. **Load Data Caches**: Parse all `caches/*.yaml` files after action/sensor owners exist
13. **Cleanup Removed Components**: Tombstone removed cache definitions before deleting stale owners

---

## Pack Types

### System Packs

System packs are built-in packs that ship with Attune.

- `system: true` in pack manifest
- Installed automatically
- Cannot be uninstalled
- Examples: `core`, `system`, `utils`

### Community Packs

Community packs are third-party packs installed from repositories.

- `system: false` in pack manifest
- Installed via CLI or API
- Can be updated and uninstalled
- Examples: `slack`, `aws`, `github`, `datadog`

### Ad-Hoc Packs

Ad-hoc packs are user-created packs without code-based components.

- `system: false` in pack manifest
- Created via Web UI
- May only contain triggers (no actions/sensors)
- Used for custom webhook integrations

---

## Best Practices

### Naming Conventions

- **Pack refs**: lowercase, alphanumeric, hyphens/underscores (e.g., `my-pack`, `aws_ec2`)
- **Component refs**: `<pack_ref>.<component_name>` (e.g., `core.echo`, `slack.send_message`)
- **File names**: Match component names (e.g., `echo.yaml`, `echo.sh`)

Before upgrading an existing installation, run `attune pack check` against each
unpacked pack. Administrators can preflight persisted refs with the read-only
query `SELECT ref FROM pack WHERE ref LIKE '%.%';`. Re-register any returned
pack under a simple ref and update its component references before upgrading;
new registrations reject dotted pack refs.

### Versioning

- Use semantic versioning (MAJOR.MINOR.PATCH)
- Update `CHANGELOG.md` with each release
- Increment version in `pack.yaml` when releasing

### Documentation

- Include comprehensive `README.md` with usage examples
- Document all parameters and output schemas
- Provide example configurations

### Testing

- Include unit tests in `tests/` directory
- Test actions/sensors independently
- Validate parameter schemas

### Dependencies

- Specify all runtime dependencies in pack manifest
- Pin dependency versions in `requirements.txt` or `package.json`
- Test with minimum required versions

### Security

- **Use `stdin` or `file` parameter delivery for actions with sensitive data** (not `env`)
- Use `secret: true` for sensitive parameters (passwords, tokens, API keys)
- Mark actions with credentials using `parameter_delivery: stdin` and `parameter_format: json`
- Validate all user inputs
- Sanitize command-line arguments to prevent injection
- Use HTTPS for API calls with SSL verification enabled
- Never log sensitive parameters in action output

---

## Example Packs

### Minimal Pack

```
my-pack/
├── pack.yaml
├── README.md
└── actions/
    ├── hello.yaml
    └── hello.sh
```

### Full-Featured Pack

```
slack-pack/
├── pack.yaml
├── README.md
├── CHANGELOG.md
├── LICENSE
├── requirements.txt
├── icon.png
├── actions/
│   ├── send_message.yaml
│   ├── send_message.py
│   ├── upload_file.yaml
│   └── upload_file.py
├── sensors/
│   ├── message_sensor.yaml
│   └── message_sensor.py
├── triggers/
│   ├── message_received.yaml
│   └── reaction_added.yaml
├── config.schema.yaml
└── tests/
    ├── test_actions.py
    └── test_sensors.py
```

---

## Related Documentation

- [Pack Management Architecture](./pack-management-architecture.md)
- [Pack Management API](./api-packs.md)
- [Trigger and Sensor Architecture](./trigger-sensor-architecture.md)
- [Parameter Delivery Methods](../actions/parameter-delivery.md)
- [Action Development Guide](./action-development-guide.md) (future)
- [Sensor Development Guide](./sensor-development-guide.md) (future)
