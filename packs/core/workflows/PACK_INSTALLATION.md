# Pack Installation Workflow System

**Status**: Schema Complete, Implementation Required  
**Version**: 1.0.0  
**Last Updated**: 2025-02-05

---

## Overview

The pack installation workflow provides a comprehensive, automated system for installing Attune packs from multiple sources with automatic dependency resolution, runtime environment setup, testing, and registration.

This document describes the workflow architecture, supporting actions, and implementation requirements.

---

## Architecture

### Main Workflow: `core.install_packs`

A multi-stage orchestration workflow that handles the complete pack installation lifecycle:

```
┌─────────────────────────────────────────────────────────────┐
│                    Install Packs Workflow                    │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  1. Initialize           → Set up temp directory             │
│  2. Download Packs       → Fetch from git/HTTP/registry     │
│  3. Check Results        → Validate downloads                │
│  4. Get Dependencies     → Parse pack.yaml                   │
│  5. Install Dependencies → Recursive installation            │
│  6. Build Environments   → Python/Node.js setup              │
│  7. Run Tests           → Verify functionality               │
│  8. Register Packs      → Load into database                 │
│  9. Cleanup             → Remove temp files                  │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

### Supporting Actions

The workflow delegates specific tasks to five core actions:

1. **`core.download_packs`** - Download from multiple sources
2. **`core.get_pack_dependencies`** - Parse dependency information
3. **`core.build_pack_envs`** - Create runtime environments
4. **`core.run_pack_tests`** - Execute test suites
5. **`core.register_packs`** - Load components into database

---

## Workflow Details

### Input Parameters

```yaml
parameters:
  packs:
    type: array
    description: "List of packs to install"
    required: true
    examples:
      - ["https://github.com/attune/pack-slack.git"]
      - ["slack@1.0.0", "aws@2.1.0"]
      - ["https://example.com/packs/custom.tar.gz"]
  
  ref_spec:
    type: string
    description: "Git reference (branch/tag/commit)"
    optional: true
  
  skip_dependencies: boolean
  skip_tests: boolean
  skip_env_build: boolean
  force: boolean
  
  registry_url: string (default: https://registry.attune.io)
  packs_base_dir: string (default: /opt/attune/packs)
  api_url: string (default: http://localhost:8080)
  timeout: integer (default: 1800)
```

### Supported Pack Sources

#### 1. Git Repositories

```yaml
packs:
  - "https://github.com/attune/pack-slack.git"
ref_spec: "v1.0.0"  # Optional: branch, tag, or commit
```

**Features:**
- Explicitly approved HTTPS URLs supported
- Shallow clones for efficiency
- Specific ref checkout (branch/tag/commit)
- SSH, `git://`, `file://`, and credential-bearing URLs rejected

#### 2. HTTP Archives

```yaml
packs:
  - "https://example.com/packs/custom-pack.tar.gz"
  - "https://cdn.example.com/slack-pack.zip"
```

**Supported formats:**
- `.tar.gz` / `.tgz`
- `.zip`

#### 3. Pack Registry References

```yaml
packs:
  - "slack@1.0.0"      # Specific version
  - "aws@^2.1.0"       # Semver range
  - "kubernetes"       # Latest version
```

**Features:**
- Automatic URL resolution from registry
- Version constraint support
- Centralized pack metadata

---

## Action Specifications

### 1. Download Packs (`core.download_packs`)

**Purpose**: Download packs from various sources to a temporary directory.

**Responsibilities:**
- Detect source type (git/HTTP/registry)
- Clone git repositories with optional ref checkout
- Download and extract HTTP archives
- Resolve pack registry references to download URLs
- Locate and parse `pack.yaml` files
- Calculate directory checksums
- Return download metadata for downstream tasks

**Input:**
```yaml
packs: ["https://github.com/attune/pack-slack.git"]
destination_dir: "/tmp/attune-pack-install-abc123"
registry_url: "https://registry.attune.io/index.json"
ref_spec: "v1.0.0"
timeout: 300
verify_ssl: true
api_url: "http://localhost:8080"
```

**Output:**
```json
{
  "downloaded_packs": [
    {
      "source": "https://github.com/attune/pack-slack.git",
      "source_type": "git",
      "pack_path": "/tmp/attune-pack-install-abc123/slack",
      "pack_ref": "slack",
      "pack_version": "1.0.0",
      "git_commit": "a1b2c3d4e5",
      "checksum": "sha256:..."
    }
  ],
  "failed_packs": [],
  "total_count": 1,
  "success_count": 1,
  "failure_count": 0
}
```

**Implementation Notes:**
- Should call the API endpoint rather than implement network access directly
- Private sources use authenticated registry archives; server Git is anonymous HTTPS only
- Must validate `pack.yaml` exists and is readable
- Should support both root-level and `pack/` subdirectory structures

---

### 2. Get Pack Dependencies (`core.get_pack_dependencies`)

**Purpose**: Parse `pack.yaml` files to identify pack and runtime dependencies.

**Responsibilities:**
- Read and parse `pack.yaml` files (YAML parsing)
- Extract `dependencies` section (pack dependencies)
- Extract `python` and `nodejs` runtime requirements
- Check which pack dependencies are already installed
- Identify `requirements.txt` and `package.json` files
- Build list of missing dependencies for installation

**Input:**
```yaml
pack_paths: ["/tmp/attune-pack-install-abc123/slack"]
api_url: "http://localhost:8080"
skip_validation: false
```

**Output:**
```json
{
  "dependencies": [
    {
      "pack_ref": "core",
      "version_spec": ">=1.0.0",
      "required_by": "slack",
      "already_installed": true
    }
  ],
  "runtime_requirements": {
    "slack": {
      "pack_ref": "slack",
      "python": {
        "version": ">=3.8",
        "requirements_file": "/tmp/.../slack/requirements.txt"
      }
    }
  },
  "missing_dependencies": [
    {
      "pack_ref": "http",
      "version_spec": "^1.0.0",
      "required_by": "slack"
    }
  ],
  "analyzed_packs": [
    {
      "pack_ref": "slack",
      "pack_path": "/tmp/.../slack",
      "has_dependencies": true,
      "dependency_count": 2
    }
  ],
  "errors": []
}
```

**Implementation Notes:**
- Must parse YAML files (use `yq`, Python, or API call)
- Should call `GET /api/v1/packs` to check installed packs
- Must handle missing or malformed `pack.yaml` files gracefully
- Should validate version specifications (semver)

---

### 3. Build Pack Environments (`core.build_pack_envs`)

**Purpose**: Create runtime environments and install dependencies.

**Responsibilities:**
- Create Python virtualenvs for packs with Python dependencies
- Install packages from `requirements.txt` using pip
- Run `npm install` for packs with Node.js dependencies
- Handle environment creation failures gracefully
- Track installed package counts and build times
- Support force rebuild of existing environments

**Input:**
```yaml
pack_paths: ["/tmp/attune-pack-install-abc123/slack"]
packs_base_dir: "/opt/attune/packs"
python_version: "3.11"
nodejs_version: "20"
skip_python: false
skip_nodejs: false
force_rebuild: false
timeout: 600
```

**Output:**
```json
{
  "built_environments": [
    {
      "pack_ref": "slack",
      "pack_path": "/tmp/.../slack",
      "environments": {
        "python": {
          "virtualenv_path": "/tmp/.../slack/virtualenv",
          "requirements_installed": true,
          "package_count": 15,
          "python_version": "3.11.2"
        }
      },
      "duration_ms": 45000
    }
  ],
  "failed_environments": [],
  "summary": {
    "total_packs": 1,
    "success_count": 1,
    "failure_count": 0,
    "python_envs_built": 1,
    "nodejs_envs_built": 0,
    "total_duration_ms": 45000
  }
}
```

**Implementation Notes:**
- Python virtualenv creation: `python -m venv {pack_path}/virtualenv`
- Pip install: `source virtualenv/bin/activate && pip install -r requirements.txt`
- Node.js install: `npm install --production` in pack directory
- Must handle timeouts and cleanup on failure
- Should use containerized workers for isolation

---

### 4. Run Pack Tests (`core.run_pack_tests`)

**Purpose**: Execute pack test suites to verify functionality.

**Responsibilities:**
- Detect test framework (pytest, unittest, npm test, shell scripts)
- Execute tests in isolated environment
- Capture test output and results
- Return pass/fail status with details
- Support parallel test execution
- Handle test timeouts

**Input:**
```yaml
pack_paths: ["/tmp/attune-pack-install-abc123/slack"]
timeout: 300
fail_on_error: false
```

**Output:**
```json
{
  "test_results": [
    {
      "pack_ref": "slack",
      "status": "passed",
      "total_tests": 25,
      "passed": 25,
      "failed": 0,
      "skipped": 0,
      "duration_ms": 12000,
      "output": "..."
    }
  ],
  "summary": {
    "total_packs": 1,
    "all_passed": true,
    "total_tests": 25,
    "total_passed": 25,
    "total_failed": 0
  }
}
```

**Implementation Notes:**
- Check for `test` section in `pack.yaml`
- Default test discovery: `tests/` directory
- Python: Run pytest or unittest
- Node.js: Run `npm test`
- Shell: Execute `test.sh` scripts
- Should capture stdout/stderr for debugging

---

### 5. Register Packs (`core.register_packs`)

**Purpose**: Validate schemas, load components into database, copy to storage.

**Responsibilities:**
- Validate `pack.yaml` schema
- Scan for component files (actions, sensors, triggers, rules, workflows, policies)
- Validate each component schema
- Call API endpoint to register pack in database
- Copy pack files to permanent storage (`/opt/attune/packs/{pack_ref}/`)
- Record installation metadata
- Handle registration rollback on failure (atomic operation)

**Input:**
```yaml
pack_paths: ["/tmp/attune-pack-install-abc123/slack"]
packs_base_dir: "/opt/attune/packs"
skip_validation: false
skip_tests: false
force: false
api_url: "http://localhost:8080"
api_token: "jwt_token_here"
```

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
        "actions": 15,
        "sensors": 3,
        "triggers": 2,
        "rules": 5,
        "workflows": 2,
        "policies": 0
      },
      "test_result": {
        "status": "passed",
        "total_tests": 25,
        "passed": 25,
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
    "total_components": 27,
    "duration_ms": 8000
  }
}
```

**Implementation Notes:**
- **Primary approach**: Call `POST /api/v1/packs/register` endpoint
- The API already implements:
  - Pack metadata validation
  - Component scanning and registration
  - Database record creation
  - File copying to permanent storage
  - Installation metadata tracking
- This action should be a thin wrapper for API calls
- Must handle authentication (JWT token)
- Must implement proper error handling and retries
- Should validate API response and extract relevant data

**API Endpoint Reference:**
```
POST /api/v1/packs/register
Content-Type: application/json
Authorization: Bearer {token}

{
  "path": "/tmp/attune-pack-install-abc123/slack",
  "force": false,
  "skip_tests": false
}

Response:
{
  "data": {
    "pack_id": 42,
    "pack": { ... },
    "test_result": { ... }
  }
}
```

---

## Workflow Execution Flow

### Success Path

```
1. Initialize
   ↓
2. Download Packs
   ↓ (if any downloads succeeded)
3. Check Results
   ↓ (if not skip_dependencies)
4. Get Dependencies
   ↓ (if missing dependencies found)
5. Install Dependencies (recursive call)
   ↓
6. Build Environments
   ↓ (if not skip_tests)
7. Run Tests
   ↓
8. Register Packs
   ↓
9. Cleanup Success
   ✓ Complete
```

### Failure Handling

Each stage can fail and trigger cleanup:

- **Download fails**: Go to cleanup_on_failure
- **Dependency installation fails**: 
  - If `force=true`: Continue to build_environments
  - If `force=false`: Go to cleanup_on_failure
- **Environment build fails**:
  - If `force=true` or `skip_env_build=true`: Continue
  - If `force=false`: Go to cleanup_on_failure
- **Tests fail**:
  - If `force=true`: Continue to register_packs
  - If `force=false`: Go to cleanup_on_failure
- **Registration fails**: Go to cleanup_on_failure

### Force Mode Behavior

When `force: true`:

- ✓ Continue even if downloads fail
- ✓ Skip dependency validation failures
- ✓ Skip environment build failures
- ✓ Skip test failures
- ✓ Override existing pack installations

**Use Cases:**
- Development and testing
- Emergency deployments
- Pack upgrades
- Recovery from partial installations

**Warning:** Force mode bypasses safety checks. Use cautiously in production.

---

## Recursive Dependency Resolution

The workflow supports recursive dependency installation:

```
install_packs(["slack"])
  ↓
  Depends on: ["core@>=1.0.0", "http@^1.0.0"]
  ↓
  install_packs(["http"])  # Recursive call
    ↓
    Depends on: ["core@>=1.0.0"]
    ↓
    core already installed ✓
  ✓
  http installed ✓
  ↓
slack installed ✓
```

**Features:**
- Automatically detects and installs missing dependencies
- Prevents circular dependencies (each pack registered once)
- Respects version constraints (semver)
- Installs dependencies depth-first
- Tracks installed packs to avoid duplicates

---

## Error Handling

### Atomic Registration

Pack registration is atomic - all components are registered or none:

- ✓ Validates all component schemas first
- ✓ Creates database transaction for registration
- ✓ Rolls back on any component failure
- ✓ Prevents partial pack installations

### Cleanup Strategy

Temporary directories are always cleaned up:

- **On success**: Remove temp directory after registration
- **On failure**: Remove temp directory and report errors
- **On timeout**: Cleanup triggered by workflow timeout handler

### Error Reporting

Comprehensive error information returned:

```json
{
  "failed_packs": [
    {
      "pack_path": "/tmp/.../custom-pack",
      "pack_ref": "custom",
      "error": "Schema validation failed: action 'do_thing' missing required field 'runner_type'",
      "error_stage": "validation"
    }
  ]
}
```

Error stages:
- `validation` - Schema validation failed
- `testing` - Pack tests failed
- `database_registration` - Database operation failed
- `file_copy` - File system operation failed
- `api_call` - API request failed

---

## Implementation Status

### ✅ Complete

- Workflow YAML schema (`install_packs.yaml`)
- Action YAML schemas (5 actions)
- Action placeholder scripts (.sh files)
- Documentation
- Error handling structure
- Output schemas

### 🔄 Requires Implementation

All action scripts currently return placeholder responses. Each needs proper implementation:

#### 1. `download_packs.sh`

**Implementation Options:**

**Option A: API-based** (Recommended)
- Create API endpoint: `POST /api/v1/packs/download`
- Action calls API with pack list
- API handles git/HTTP/registry logic
- Returns download results to action

**Option B: Direct implementation**
- Implement git cloning logic in script
- Implement HTTP download and extraction
- Implement registry lookup and resolution
- Handle all error cases

**Recommendation**: Option A (API-based) keeps action scripts lean and centralizes pack handling logic in the API service.

#### 2. `get_pack_dependencies.sh`

**Implementation approach:**
- Parse YAML files (use `yq` tool or Python script)
- Extract dependencies from `pack.yaml`
- Call `GET /api/v1/packs` to get installed packs
- Compare and build missing dependencies list

#### 3. `build_pack_envs.sh`

**Implementation approach:**
- For each pack with `requirements.txt`:
  ```bash
  python -m venv {pack_path}/virtualenv
  source {pack_path}/virtualenv/bin/activate
  pip install -r {pack_path}/requirements.txt
  ```
- For each pack with `package.json`:
  ```bash
  cd {pack_path}
  npm install --production
  ```
- Handle timeouts and errors
- Use containerized workers for isolation

#### 4. `run_pack_tests.sh`

**Implementation approach:**
- Already exists in core pack: `core.run_pack_tests`
- May need minor updates for integration
- Supports pytest, unittest, npm test

#### 5. `register_packs.sh`

**Implementation approach:**
- Call existing API endpoint: `POST /api/v1/packs/register`
- Send pack path and options
- Parse API response
- Handle authentication (JWT token from workflow context)

**API Integration:**
```bash
curl -X POST "$API_URL/api/v1/packs/register" \
  -H "Authorization: Bearer $API_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{
    \"path\": \"$pack_path\",
    \"force\": $FORCE,
    \"skip_tests\": $SKIP_TESTS
  }"
```

---

## Testing Strategy

### Unit Tests

Test each action independently:

```bash
# Test download_packs with mock git repo
./actions/download_packs.sh \
  ATTUNE_ACTION_PACKS='["https://github.com/test/pack-test.git"]' \
  ATTUNE_ACTION_DESTINATION_DIR=/tmp/test

# Verify output structure
jq '.downloaded_packs | length' output.json
```

### Integration Tests

Test complete workflow:

```bash
# Execute workflow via API
curl -X POST "$API_URL/api/v1/workflows/execute" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "workflow": "core.install_packs",
    "input": {
      "packs": ["https://github.com/attune/pack-test.git"],
      "skip_tests": false,
      "force": false
    }
  }'

# Check execution status
curl "$API_URL/api/v1/executions/$EXECUTION_ID"

# Verify pack registered
curl "$API_URL/api/v1/packs/test-pack"
```

### End-to-End Tests

Test with real packs:

1. Install core pack (already installed)
2. Install pack with dependencies
3. Install pack from HTTP archive
4. Install pack from registry reference
5. Test force mode reinstallation
6. Test error handling (invalid pack)

---

## Usage Examples

### Example 1: Install Single Pack from Git

```yaml
workflow: core.install_packs
input:
  packs:
    - "https://github.com/attune/pack-slack.git"
  ref_spec: "v1.0.0"
  skip_dependencies: false
  skip_tests: false
  force: false
```

### Example 2: Install Multiple Packs from Registry

```yaml
workflow: core.install_packs
input:
  packs:
    - "slack@1.0.0"
    - "aws@^2.1.0"
    - "kubernetes@>=3.0.0"
  skip_dependencies: false
  skip_tests: false
```

### Example 3: Force Reinstall with Skip Tests

```yaml
workflow: core.install_packs
input:
  packs:
    - "https://github.com/myorg/pack-custom.git"
  ref_spec: "main"
  skip_dependencies: true
  skip_tests: true
  force: true
```

### Example 4: Install from HTTP Archive

```yaml
workflow: core.install_packs
input:
  packs:
    - "https://example.com/packs/custom-pack-1.0.0.tar.gz"
  skip_dependencies: false
  skip_tests: false
```

---

## Future Enhancements

### Phase 2 Features

1. **Pack Upgrade Workflow**
   - Detect installed version
   - Download new version
   - Run migration scripts
   - Update in-place or side-by-side

2. **Pack Uninstall Workflow**
   - Check for dependent packs
   - Remove from database
   - Remove from filesystem
   - Optional backup before removal

3. **Pack Validation Workflow**
   - Validate without installing
   - Check dependencies
   - Run tests in isolated environment
   - Report validation results

4. **Batch Operations**
   - Install all packs from registry
   - Upgrade all installed packs
   - Validate all installed packs

### Phase 3 Features

1. **Registry Integration**
   - Automatic version discovery
   - Dependency resolution from registry
   - Pack popularity metrics
   - Security vulnerability scanning

2. **Advanced Dependency Management**
   - Conflict detection
   - Version constraint solving
   - Dependency graphs
   - Optional dependencies

3. **Rollback Support**
   - Snapshot before installation
   - Rollback on failure
   - Version history
   - Migration scripts

4. **Performance Optimizations**
   - Parallel downloads
   - Cached dependencies
   - Incremental updates
   - Build caching

---

## Related Documentation

- [Pack Structure](../../../docs/packs/pack-structure.md) - Pack directory format
- [Pack Installation from Git](../../../docs/packs/pack-installation-git.md) - Git installation guide
- [Pack Registry Specification](../../../docs/packs/pack-registry-spec.md) - Registry format
- [Pack Testing Framework](../../../docs/packs/pack-testing-framework.md) - Testing packs
- [API Documentation](../../../docs/api/api-packs.md) - Pack API endpoints

---

## Support

For questions or issues:

- GitHub Issues: https://github.com/attune-io/attune/issues
- Documentation: https://docs.attune.io/workflows/pack-installation
- Community: https://community.attune.io

---

## Changelog

### v1.0.0 (2025-02-05)

- Initial workflow schema design
- Five supporting action schemas
- Comprehensive documentation
- Placeholder implementation scripts
- Error handling structure
- Output schemas defined

### Next Steps

1. Implement `download_packs.sh` (or create API endpoint)
2. Implement `get_pack_dependencies.sh`
3. Implement `build_pack_envs.sh`
4. Update `run_pack_tests.sh` if needed
5. Implement `register_packs.sh` (API wrapper)
6. End-to-end testing
7. Documentation updates based on testing
