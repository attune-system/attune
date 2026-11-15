> Core 1.0.7 ships only native echo, sleep, and noop actions. References below to
> HTTP, SSH, agent-command, or pack-installation core actions describe withdrawn
> examples. Use application-owned packs or the normal pack CLI/API instead.

# Development Packs Directory

## Overview

The `packs.dev/` directory provides a development environment for creating and testing custom packs without rebuilding Docker images. Files in this directory are mounted directly into Docker containers at `/opt/attune/packs.dev`, allowing immediate access to changes.

## Quick Start

### 1. Create a New Pack

```bash
./scripts/dev-pack.sh create my-pack
```

This creates a complete pack structure:
```
packs.dev/my-pack/
├── pack.yaml
├── actions/
│   ├── example.yaml
│   └── example.sh
├── triggers/
├── sensors/
├── workflows/
└── README.md
```

### 2. Validate the Pack

```bash
./scripts/dev-pack.sh validate my-pack
```

### 3. Start Docker Environment

```bash
docker compose up -d
```

The pack is automatically available at `/opt/attune/packs.dev/my-pack` in all containers.

### 4. Register the Pack

Get an authentication token:
```bash
# Login via web UI or CLI
attune auth login test@attune.local
```

Register the pack via API:
```bash
curl -X POST http://localhost:8080/api/v1/packs \
  -H "Authorization: Bearer $ATTUNE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "my-pack",
    "label": "My Custom Pack",
    "description": "My custom automation pack",
    "version": "1.0.0",
    "system": false,
    "enabled": true
  }'
```

### 5. Test the Pack

Create a rule that uses your pack's actions, or execute directly:
```bash
curl -X POST http://localhost:8080/api/v1/executions \
  -H "Authorization: Bearer $ATTUNE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "action": "my-pack.example",
    "parameters": {
      "message": "Hello from dev pack!"
    }
  }'
```

## Directory Structure

```
packs.dev/
├── README.md                   # Usage guide
├── .gitignore                  # Ignore custom packs, keep examples
├── examples/                   # Example packs
│   ├── basic-pack/            # Minimal shell action example
│   └── python-pack/           # Python action example
└── my-pack/                   # Your custom pack (not in git)
    ├── pack.yaml              # Pack metadata
    ├── actions/               # Action definitions and scripts
    ├── triggers/              # Trigger definitions
    ├── sensors/               # Sensor definitions
    └── workflows/             # Workflow definitions
```

## Volume Mounts

The `packs.dev/` directory is mounted in Docker Compose:

```yaml
volumes:
  - ./packs.dev:/opt/attune/packs.dev:rw
```

This mount is added to all relevant services:
- **api** - Pack registration and metadata
- **executor** - Workflow execution
- **worker-*** - Action execution
- **sensor** - Sensor execution

### Core vs Dev Packs

| Location | Mount Type | Purpose |
|----------|------------|---------|
| `/opt/attune/packs` | Volume (ro) | Production core pack |
| `/opt/attune/packs.dev` | Bind mount (rw) | Development packs |

The core pack is read-only in containers, while dev packs are read-write for active development.

## Development Workflow

### Typical Development Cycle

1. **Create pack structure**
   ```bash
   ./scripts/dev-pack.sh create my-integration
   ```

2. **Edit pack files**
   - Edit `packs.dev/my-integration/pack.yaml`
   - Add actions in `actions/`
   - Add workflows in `workflows/`

3. **Validate**
   ```bash
   ./scripts/dev-pack.sh validate my-integration
   ```

4. **Test immediately** - Changes are live in containers!
   - No rebuild needed
   - No restart needed
   - Actions are available instantly

5. **Iterate** - Make changes and test again

6. **Export for production** - When ready, package the pack properly

### Live Reloading

Changes to pack files are immediately visible in containers because they're bind-mounted:

- **Action scripts**: Available immediately for execution
- **Action/Trigger YAML**: Requires pack re-registration to update DB
- **Workflows**: Use workflow sync endpoint to reload

```bash
# Sync workflows after changes
curl -X POST http://localhost:8080/api/v1/packs/my-pack/workflows/sync \
  -H "Authorization: Bearer $ATTUNE_TOKEN"
```

## Helper Script Reference

### Commands

#### `create <pack-ref>`
Creates a new pack structure with example files.

```bash
./scripts/dev-pack.sh create my-awesome-pack
```

Creates:
- `packs.dev/my-awesome-pack/`
- Basic pack.yaml
- Example shell action
- README with instructions

#### `list`
Lists all development packs.

```bash
./scripts/dev-pack.sh list
```

Output:
```
Development Packs:

  my-pack
    Label: My Pack
    Version: 1.0.0

  integration-pack
    Label: Integration Pack
    Version: 2.1.0

Total: 2 pack(s)
```

#### `validate <pack-ref>`
Validates pack structure and files.

```bash
./scripts/dev-pack.sh validate my-pack
```

Checks:
- `pack.yaml` exists and is valid YAML
- Action definitions reference existing scripts
- Scripts are executable
- Required directories exist

#### `register <pack-ref>`
Shows the API command to register the pack.

```bash
./scripts/dev-pack.sh register my-pack
```

Outputs the `curl` command needed to register via API.

#### `clean`
Removes all non-example packs (interactive confirmation).

```bash
./scripts/dev-pack.sh clean
```

**Warning**: This permanently deletes custom packs!

## Example Packs

### Basic Pack (Shell Actions)

Location: `packs.dev/examples/basic-pack/`

Simple shell-based action that echoes a message.

**Try it:**
```bash
# View the pack
ls -la packs.dev/examples/basic-pack/

# Register it (after starting Docker)
curl -X POST http://localhost:8080/api/v1/packs \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d @packs.dev/examples/basic-pack/pack.json
```

### Python Pack

Location: `packs.dev/examples/python-pack/`

Python action with parameters and structured output.

Features:
- Parameter validation
- JSON output
- Array handling
- Environment variable access

## Best Practices

### Pack Structure

1. **Use descriptive refs**: `my-company-integration`, not `pack1`
2. **Version properly**: Follow semantic versioning (1.0.0)
3. **Document actions**: Clear descriptions and parameter docs
4. **Test parameters**: Validate edge cases and defaults
5. **Handle errors**: Always return valid JSON, even on error

### Action Scripts

```bash
#!/bin/bash
set -e  # Exit on error

# Get parameters (with defaults)
PARAM="${ATTUNE_ACTION_param:-default_value}"

# Validate inputs
if [ -z "$PARAM" ]; then
    echo '{"error": "param is required"}' >&2
    exit 1
fi

# Do work
RESULT=$(do_something "$PARAM")

# Return JSON
echo "{\"result\": \"$RESULT\"}"
```

### Security Considerations

1. **No secrets in code**: Use Attune's secret management
2. **Validate inputs**: Never trust action parameters directly
3. **Sandbox scripts**: Be aware workers execute with privileges
4. **Review dependencies**: Check Python/Node packages carefully

### Version Control

The `.gitignore` in `packs.dev/` excludes custom packs:

```gitignore
*
!.gitignore
!README.md
!examples/
!examples/**
```

This means:
- ✅ Example packs are committed
- ✅ Documentation is committed
- ❌ Your custom packs are NOT committed

To version control a custom pack:
1. Move it to a separate repository
2. Or explicitly add it: `git add -f packs.dev/my-pack/`

## Troubleshooting

### Pack Not Found

**Symptom**: "Pack not found" when executing action

**Solutions**:
1. Verify pack is registered in database:
   ```bash
   curl http://localhost:8080/api/v1/packs/$PACK_REF \
     -H "Authorization: Bearer $TOKEN"
   ```

2. Check pack directory exists:
   ```bash
   docker exec attune-api ls -la /opt/attune/packs.dev/
   ```

3. Verify mount in docker-compose.yaml:
   ```bash
   grep -A 2 "packs.dev" docker-compose.yaml
   ```

### Action Not Executing

**Symptom**: Action fails with "entry point not found"

**Solutions**:
1. Check script exists and is executable:
   ```bash
   ls -la packs.dev/my-pack/actions/
   ```

2. Verify entry_point in action YAML matches filename:
   ```bash
   grep entry_point packs.dev/my-pack/actions/*.yaml
   ```

3. Check script has shebang and is executable:
   ```bash
   head -1 packs.dev/my-pack/actions/script.sh
   chmod +x packs.dev/my-pack/actions/script.sh
   ```

### Permission Errors

**Symptom**: "Permission denied" when accessing pack files

**Solutions**:
1. Check file ownership (should be readable by UID 1000):
   ```bash
   ls -ln packs.dev/my-pack/
   ```

2. Fix permissions:
   ```bash
   chmod -R 755 packs.dev/my-pack/
   ```

3. Ensure scripts are executable:
   ```bash
   find packs.dev/my-pack/ -name "*.sh" -exec chmod +x {} \;
   ```

### Changes Not Reflected

**Symptom**: Code changes don't appear in execution

**Solutions**:
1. For **action scripts**: Changes are immediate, but verify mount:
   ```bash
   docker exec attune-worker-shell cat /opt/attune/packs.dev/my-pack/actions/script.sh
   ```

2. For **action YAML**: Re-register pack or update action in DB

3. For **workflows**: Run sync endpoint:
   ```bash
   curl -X POST http://localhost:8080/api/v1/packs/my-pack/workflows/sync \
     -H "Authorization: Bearer $TOKEN"
   ```

## Advanced Usage

### Multiple Environment Packs

Use different pack refs for different environments:

```
packs.dev/
├── my-pack-dev/      # Development version
├── my-pack-staging/  # Staging version
└── my-pack/          # Production-ready version
```

### Pack Dependencies

Reference other packs in workflows:

```yaml
# In packs.dev/my-pack/workflows/example.yaml
tasks:
  - name: use_core_action
    action: core.http_request
    input:
      url: https://api.example.com
  
  - name: use_my_action
    action: my-pack.process
    input:
      data: "{{ use_core_action.output.body }}"
```

### Testing Workflows

Create test workflows in `packs.dev/`:

```yaml
# packs.dev/my-pack/workflows/test_integration.yaml
name: test_integration
ref: my-pack.test_integration
description: "Integration test workflow"

tasks:
  - name: test_action
    action: my-pack.my_action
    input:
      test: true
```

## Production Migration

When ready to deploy a dev pack to production:

1. **Clean up**: Remove test files and documentation
2. **Version**: Tag with proper version number
3. **Test**: Run full test suite
4. **Package**: Create proper pack archive
5. **Install**: Use pack installation API
6. **Deploy**: Install on production Attune instance

See [Pack Registry Documentation](../packs/pack-registry-spec.md) for production deployment.

## See Also

- [Pack Structure Documentation](../packs/pack-structure.md)
- [Action Development Guide](../packs/PACK_TESTING.md)
- [Workflow Development](../workflows/workflow-summary.md)
- [Pack Registry](../packs/pack-registry-spec.md)
- [Docker Deployment](../deployment/docker-deployment.md)