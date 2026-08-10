# Pack Installation Actions

This document describes the pack installation actions that automate the process of downloading, analyzing, building environments, and registering packs in Attune.

## Overview

The pack installation system consists of four core actions that work together to automate pack installation:

1. **`core.download_packs`** - Downloads packs from git, HTTP, or registry sources
2. **`core.get_pack_dependencies`** - Analyzes pack dependencies and runtime requirements
3. **`core.build_pack_envs`** - Creates Python virtualenvs and Node.js environments
4. **`core.register_packs`** - Registers packs with the Attune API and database

These actions are designed to be used in workflows (like `core.install_packs`) or independently via the CLI/API.

## Actions

### 1. core.download_packs

Downloads packs from various sources to a local directory.

**Source Types:**
- **Git repositories**: explicitly approved HTTPS URLs ending in `.git`
- **HTTP archives**: approved HTTPS URLs (tar.gz, zip); HTTP requires explicit development configuration
- **Registry references**: Pack name with optional version (e.g., `slack@1.0.0`)

**Parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `packs` | array[string] | Yes | - | List of pack sources to download |
| `destination_dir` | string | Yes | - | Directory where packs will be downloaded |
| `registry_url` | string | No | `https://registry.attune.io/index.json` | Pack registry URL |
| `ref_spec` | string | No | - | Git reference (branch/tag/commit) for git sources |
| `timeout` | integer | No | 300 | Download timeout in seconds per pack |
| `verify_ssl` | boolean | No | true | Verify SSL certificates for HTTPS |
| `api_url` | string | No | `http://localhost:8080` | Attune API URL |

**Output:**

```json
{
  "downloaded_packs": [
    {
      "source": "https://github.com/attune/pack-slack.git",
      "source_type": "git",
      "pack_path": "/tmp/downloads/pack-0-1234567890",
      "pack_ref": "slack",
      "pack_version": "1.0.0",
      "git_commit": "abc123def456",
      "checksum": "d41d8cd98f00b204e9800998ecf8427e"
    }
  ],
  "failed_packs": [],
  "total_count": 1,
  "success_count": 1,
  "failure_count": 0
}
```

**Example Usage:**

```bash
# CLI
attune action execute core.download_packs \
  --param packs='["https://github.com/attune/pack-slack.git"]' \
  --param destination_dir=/tmp/attune-packs \
  --param ref_spec=v1.0.0

# Via API
curl -X POST http://localhost:8080/api/v1/executions \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "action": "core.download_packs",
    "parameters": {
      "packs": ["slack@1.0.0"],
      "destination_dir": "/tmp/attune-packs"
    }
  }'
```

### 2. core.get_pack_dependencies

Parses pack.yaml files to extract dependencies and runtime requirements.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `pack_paths` | array[string] | Yes | - | List of pack directory paths to analyze |
| `skip_validation` | boolean | No | false | Skip pack.yaml schema validation |
| `api_url` | string | No | `http://localhost:8080` | Attune API URL for checking installed packs |

**Output:**

```json
{
  "dependencies": [
    {
      "pack_ref": "core",
      "version_spec": "*",
      "required_by": "slack",
      "already_installed": true
    }
  ],
  "runtime_requirements": {
    "slack": {
      "pack_ref": "slack",
      "python": {
        "version": "3.11",
        "requirements_file": "/tmp/slack/requirements.txt"
      }
    }
  },
  "missing_dependencies": [],
  "analyzed_packs": [
    {
      "pack_ref": "slack",
      "pack_path": "/tmp/slack",
      "has_dependencies": true,
      "dependency_count": 1
    }
  ],
  "errors": []
}
```

**Example Usage:**

```bash
# CLI
attune action execute core.get_pack_dependencies \
  --param pack_paths='["/tmp/attune-packs/slack"]'

# Check for missing dependencies
result=$(attune action execute core.get_pack_dependencies \
  --param pack_paths='["/tmp/attune-packs/slack"]' \
  --json)

missing=$(echo "$result" | jq '.output.missing_dependencies | length')
if [[ $missing -gt 0 ]]; then
  echo "Missing dependencies detected"
fi
```

### 3. core.build_pack_envs

Creates runtime environments and installs dependencies for packs.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `pack_paths` | array[string] | Yes | - | List of pack directory paths |
| `packs_base_dir` | string | No | `/opt/attune/packs` | Base directory for permanent pack storage |
| `python_version` | string | No | `3.11` | Python version for virtualenvs |
| `nodejs_version` | string | No | `20` | Node.js version |
| `skip_python` | boolean | No | false | Skip building Python environments |
| `skip_nodejs` | boolean | No | false | Skip building Node.js environments |
| `force_rebuild` | boolean | No | false | Force rebuild of existing environments |
| `timeout` | integer | No | 600 | Timeout in seconds per environment build |

**Output:**

```json
{
  "built_environments": [
    {
      "pack_ref": "slack",
      "pack_path": "/tmp/slack",
      "environments": {
        "python": {
          "virtualenv_path": "/tmp/slack/virtualenv",
          "requirements_installed": true,
          "package_count": 15,
          "python_version": "3.11.5"
        }
      },
      "duration_ms": 12500
    }
  ],
  "failed_environments": [],
  "summary": {
    "total_packs": 1,
    "success_count": 1,
    "failure_count": 0,
    "python_envs_built": 1,
    "nodejs_envs_built": 0,
    "total_duration_ms": 12500
  }
}
```

**Example Usage:**

```bash
# CLI - Build Python environment only
attune action execute core.build_pack_envs \
  --param pack_paths='["/tmp/attune-packs/slack"]' \
  --param skip_nodejs=true

# Force rebuild
attune action execute core.build_pack_envs \
  --param pack_paths='["/tmp/attune-packs/slack"]' \
  --param force_rebuild=true
```

### 4. core.register_packs

Validates pack structure and registers packs with the Attune API.

**Parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `pack_paths` | array[string] | Yes | - | List of pack directory paths to register |
| `packs_base_dir` | string | No | `/opt/attune/packs` | Base directory for permanent storage |
| `skip_validation` | boolean | No | false | Skip schema validation |
| `skip_tests` | boolean | No | false | Skip running pack tests |
| `force` | boolean | No | false | Force registration (replace if exists) |
| `api_url` | string | No | `http://localhost:8080` | Attune API URL |
| `api_token` | string | No | - | API authentication token (secret) |

**Output:**

```json
{
  "registered_packs": [
    {
      "pack_ref": "slack",
      "pack_id": 42,
      "pack_version": "1.0.0",
      "storage_path": "/opt/attune/packs/slack",
      "components_registered": {
        "actions": 10,
        "sensors": 2,
        "triggers": 3,
        "rules": 1,
        "workflows": 0,
        "policies": 0
      },
      "test_result": {
        "status": "passed",
        "total_tests": 5,
        "passed": 5,
        "failed": 0
      },
      "validation_results": {
        "valid": true,
        "errors": []
      }
    }
  ],
  "failed_packs": [],
  "summary": {
    "total_packs": 1,
    "success_count": 1,
    "failure_count": 0,
    "total_components": 16,
    "duration_ms": 2500
  }
}
```

**Example Usage:**

```bash
# CLI - Register pack with authentication
attune action execute core.register_packs \
  --param pack_paths='["/tmp/attune-packs/slack"]' \
  --param api_token="$ATTUNE_API_TOKEN"

# Force registration (replace existing)
attune action execute core.register_packs \
  --param pack_paths='["/tmp/attune-packs/slack"]' \
  --param force=true \
  --param skip_tests=true
```

## Workflow Integration

These actions are designed to work together in the `core.install_packs` workflow:

```yaml
# Simplified workflow structure
workflow:
  - download_packs:
      action: core.download_packs
      input:
        packs: "{{ parameters.packs }}"
        destination_dir: "{{ vars.temp_dir }}"
  
  - get_dependencies:
      action: core.get_pack_dependencies
      input:
        pack_paths: "{{ download_packs.output.downloaded_packs | map('pack_path') }}"
  
  - build_environments:
      action: core.build_pack_envs
      input:
        pack_paths: "{{ download_packs.output.downloaded_packs | map('pack_path') }}"
  
  - register_packs:
      action: core.register_packs
      input:
        pack_paths: "{{ download_packs.output.downloaded_packs | map('pack_path') }}"
```

## Error Handling

All actions follow consistent error handling patterns:

1. **Validation Errors**: Return errors in the `errors` or `failed_*` arrays
2. **Partial Failures**: Process continues for other packs; failures are reported
3. **Fatal Errors**: Exit with non-zero code and minimal JSON output
4. **Timeouts**: Commands respect timeout parameters; failures are recorded

Example error output:

```json
{
  "downloaded_packs": [],
  "failed_packs": [
    {
      "source": "https://github.com/invalid/repo.git",
      "error": "Git clone failed or timed out"
    }
  ],
  "total_count": 1,
  "success_count": 0,
  "failure_count": 1
}
```

## Testing

Comprehensive test suite available at:
```
packs/core/tests/test_pack_installation_actions.sh
```

Run tests:
```bash
cd packs/core/tests
./test_pack_installation_actions.sh
```

Test coverage includes:
- Input validation
- JSON output format validation
- Error handling (invalid paths, missing files)
- Edge cases (spaces in paths, missing version fields)
- Timeout handling
- API integration (with mocked endpoints)

## Implementation Details

### Directory Structure

```
packs/core/actions/
├── download_packs.sh       # Implementation
├── download_packs.yaml     # Schema
├── get_pack_dependencies.sh
├── get_pack_dependencies.yaml
├── build_pack_envs.sh
├── build_pack_envs.yaml
├── register_packs.sh
└── register_packs.yaml
```

### Dependencies

**System Requirements:**
- `bash` 4.0+
- `jq` (JSON processing)
- `curl` (HTTP requests)
- `git` (for git sources)
- Archive extraction is implemented in-process by Attune; system `tar` and
  `unzip` executables are not required.
- `python3`, `pip3` (for Python environments)
- `node`, `npm` (for Node.js environments)

**Optional:**
- `md5sum` or `shasum` (checksums)

### Environment Variables

Actions receive parameters via environment variables with prefix `ATTUNE_ACTION_`:

```bash
export ATTUNE_ACTION_PACKS='["slack@1.0.0"]'
export ATTUNE_ACTION_DESTINATION_DIR=/tmp/packs
export ATTUNE_ACTION_API_TOKEN="secret-token"
```

### Output Format

All actions output JSON to stdout. Stderr is used for logging/debugging.

```bash
# Redirect stderr to see debug logs
./download_packs.sh 2>&1 | tee debug.log

# Parse output
output=$(./download_packs.sh 2>/dev/null)
success_count=$(echo "$output" | jq '.success_count')
```

## Best Practices

1. **Use Workflows**: Prefer `core.install_packs` workflow over individual actions
2. **Check Dependencies**: Always run `get_pack_dependencies` before installation
3. **Handle Timeouts**: Set appropriate timeout values for large packs
4. **Validate Output**: Check JSON validity and error fields after execution
5. **Clean Temp Directories**: Remove downloaded packs after successful registration
6. **Use API Tokens**: Always provide authentication for production environments
7. **Enable SSL Verification**: Only disable for testing/development

## Troubleshooting

### Issue: Git clone fails with authentication error

Server-side Git accepts anonymously readable, approved HTTPS URLs only. For a
private pack, publish an archive through an authenticated registry index;
SSH and credential-bearing Git URLs are rejected.

### Issue: Python virtualenv creation fails

**Solution**: Ensure Python 3 and venv module are installed:
```bash
sudo apt-get install python3 python3-venv python3-pip
```

### Issue: Registry lookup fails

**Solution**: Check registry URL and network connectivity:
```bash
curl -I https://registry.attune.io/index.json
```

### Issue: API registration fails with 401 Unauthorized

**Solution**: Provide valid API token:
```bash
export ATTUNE_ACTION_API_TOKEN="$(attune auth token)"
```

### Issue: Timeout during npm install

**Solution**: Increase timeout parameter:
```bash
--param timeout=1200  # 20 minutes
```

## See Also

- [Pack Structure](pack-structure.md)
- [Pack Registry](pack-registry-spec.md)
- [Pack Testing Framework](../packs/PACK_TESTING.md)
- [Workflow System](workflow-orchestration.md)
- [Pack Installation Workflow](../packs/core/workflows/install_packs.yaml)

## Future Enhancements

Planned improvements:
- Parallel pack downloads
- Resume incomplete downloads
- Dependency graph visualization
- Pack signature verification
- Rollback on installation failure
- Delta updates for pack upgrades
