# Pack Installation Workflow API

This document describes the API endpoints for the Pack Installation Workflow system, which enables downloading, analyzing, building environments, and registering packs through a multi-stage process.

## Overview

The pack installation workflow consists of four main stages:

1. **Download** - Fetch pack source code from various sources (Git, registry, local)
2. **Dependencies** - Analyze pack dependencies and runtime requirements
3. **Build Environments** - Prepare Python/Node.js runtime environments
4. **Register** - Register pack components in the Attune database

Each stage is exposed as an API endpoint and can be called independently or orchestrated through a workflow.

## Authentication

All endpoints require authentication via Bearer token:

```http
Authorization: Bearer <access_token>
```

## Endpoints

### 1. Download Packs

Downloads packs from explicit Git or archive URLs to API-managed temporary
storage. Registry references must use `POST /api/v1/packs/install` so identity
and verified provenance remain bound to registration.

Direct downloads require
`pack_registry.allow_unverified_direct_remote_installs: true`; verified
registry installation remains the default.

**Endpoint:** `POST /api/v1/packs/download`

**Request Body:**

```json
{
  "packs": ["https://github.com/attune-io/pack-aws.git"],
  "ref_spec": "v1.0.0"
}
```

**Parameters:**

- `packs` (array, required) - Explicit approved Git or archive URLs
- `ref_spec` (string, optional) - Git ref spec for Git sources (branch/tag/commit)

**Response:**

```json
{
  "data": {
    "downloaded_packs": [
      {
        "source": "https://github.com/attune-io/pack-aws.git",
        "source_type": "git",
        "pack_path": "/tmp/attune-pack-downloads/aws",
        "pack_ref": "aws",
        "pack_version": "1.0.0",
        "git_commit": null,
        "checksum": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
      }
    ],
    "failed_packs": [
      {
        "source": "https://invalid.example/pack.tar.gz",
        "error": "Failed to download archive"
      }
    ],
    "total_count": 2,
    "success_count": 1,
    "failure_count": 1
  }
}
```

**Status Codes:**

- `200 OK` - Request processed (check individual pack results)
- `400 Bad Request` - Invalid request parameters
- `401 Unauthorized` - Missing or invalid authentication
- `500 Internal Server Error` - Server error during download

---

### 2. Get Pack Dependencies

Analyzes pack dependencies and runtime requirements.

**Endpoint:** `POST /api/v1/packs/dependencies`

**Request Body:**

```json
{
  "pack_paths": [
    "/tmp/pack-downloads/core",
    "/tmp/pack-downloads/aws"
  ],
  "skip_validation": false
}
```

**Parameters:**

- `pack_paths` (array, required) - List of pack directory paths to analyze
- `skip_validation` (boolean, optional) - Skip validation checks
  - Default: false

**Response:**

```json
{
  "data": {
    "dependencies": [
      {
        "pack_ref": "core",
        "version_spec": ">=1.0.0",
        "required_by": "aws",
        "already_installed": true
      }
    ],
    "runtime_requirements": {
      "aws": {
        "pack_ref": "aws",
        "python": {
          "version": ">=3.9",
          "requirements_file": "/tmp/pack-downloads/aws/requirements.txt"
        },
        "nodejs": null
      }
    },
    "missing_dependencies": [],
    "analyzed_packs": [
      {
        "pack_ref": "core",
        "pack_path": "/tmp/pack-downloads/core",
        "has_dependencies": false,
        "dependency_count": 0
      },
      {
        "pack_ref": "aws",
        "pack_path": "/tmp/pack-downloads/aws",
        "has_dependencies": true,
        "dependency_count": 1
      }
    ],
    "errors": []
  }
}
```

**Response Fields:**

- `dependencies` - All pack dependencies found
- `runtime_requirements` - Python/Node.js requirements by pack
- `missing_dependencies` - Dependencies not yet installed
- `analyzed_packs` - Summary of analyzed packs
- `errors` - Any errors encountered during analysis

**Status Codes:**

- `200 OK` - Analysis completed (check errors array for issues)
- `400 Bad Request` - Invalid request parameters
- `401 Unauthorized` - Missing or invalid authentication
- `500 Internal Server Error` - Server error during analysis

---

### 3. Build Pack Environments

Detects and validates runtime environments for packs.

**Endpoint:** `POST /api/v1/packs/build-envs`

**Request Body:**

```json
{
  "pack_paths": [
    "/tmp/pack-downloads/aws"
  ],
  "packs_base_dir": "/opt/attune/packs",
  "python_version": "3.11",
  "nodejs_version": "20",
  "skip_python": false,
  "skip_nodejs": false,
  "force_rebuild": false,
  "timeout": 600
}
```

**Parameters:**

- `pack_paths` (array, required) - List of pack directory paths
- `packs_base_dir` (string, optional) - Base directory for pack installations
  - Default: `/opt/attune/packs`
- `python_version` (string, optional) - Preferred Python version
  - Default: `3.11`
- `nodejs_version` (string, optional) - Preferred Node.js version
  - Default: `20`
- `skip_python` (boolean, optional) - Skip Python environment checks
  - Default: false
- `skip_nodejs` (boolean, optional) - Skip Node.js environment checks
  - Default: false
- `force_rebuild` (boolean, optional) - Force rebuild existing environments
  - Default: false
- `timeout` (integer, optional) - Build timeout in seconds
  - Default: 600

**Response:**

```json
{
  "data": {
    "built_environments": [
      {
        "pack_ref": "aws",
        "pack_path": "/tmp/pack-downloads/aws",
        "environments": {
          "python": {
            "virtualenv_path": "/tmp/pack-downloads/aws/venv",
            "requirements_installed": true,
            "package_count": 15,
            "python_version": "Python 3.11.4"
          },
          "nodejs": null
        },
        "duration_ms": 2500
      }
    ],
    "failed_environments": [],
    "summary": {
      "total_packs": 1,
      "success_count": 1,
      "failure_count": 0,
      "python_envs_built": 1,
      "nodejs_envs_built": 0,
      "total_duration_ms": 2500
    }
  }
}
```

**Note:** In the current implementation, this endpoint detects and validates runtime availability but does not perform actual environment building. It reports existing environment status. Full environment building (creating virtualenvs, installing dependencies) is planned for future containerized worker implementation.

**Status Codes:**

- `200 OK` - Environment detection completed
- `400 Bad Request` - Invalid request parameters
- `401 Unauthorized` - Missing or invalid authentication
- `500 Internal Server Error` - Server error during detection

---

### 4. Register Packs (Batch)

Registers multiple packs and their components in the database.

**Endpoint:** `POST /api/v1/packs/register-batch`

**Request Body:**

```json
{
  "pack_paths": [
    "/opt/attune/packs/core",
    "/opt/attune/packs/aws"
  ],
  "packs_base_dir": "/opt/attune/packs",
  "skip_validation": false,
  "skip_tests": false,
  "force": false
}
```

**Parameters:**

- `pack_paths` (array, required) - List of pack directory paths to register
- `packs_base_dir` (string, optional) - Base directory for packs
  - Default: `/opt/attune/packs`
- `skip_validation` (boolean, optional) - Skip pack validation
  - Default: false
- `skip_tests` (boolean, optional) - Skip running pack tests
  - Default: false
- `force` (boolean, optional) - Force re-registration if pack exists
  - Default: false

**Response:**

```json
{
  "data": {
    "registered_packs": [
      {
        "pack_ref": "core",
        "pack_id": 1,
        "pack_version": "1.0.0",
        "storage_path": "/opt/attune/packs/core",
        "components_registered": {
          "actions": 25,
          "sensors": 5,
          "triggers": 10,
          "rules": 3,
          "workflows": 2,
          "policies": 1
        },
        "test_result": {
          "status": "passed",
          "total_tests": 27,
          "passed": 27,
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
      "total_packs": 2,
      "success_count": 2,
      "failure_count": 0,
      "total_components": 46,
      "duration_ms": 1500
    }
  }
}
```

**Response Fields:**

- `registered_packs` - Successfully registered packs with details
- `failed_packs` - Packs that failed registration with error details
- `summary` - Overall registration statistics

**Status Codes:**

- `200 OK` - Registration completed (check individual pack results)
- `400 Bad Request` - Invalid request parameters
- `401 Unauthorized` - Missing or invalid authentication
- `500 Internal Server Error` - Server error during registration

---

## Action Wrappers

These API endpoints are wrapped by shell actions in the `core` pack for workflow orchestration:

### Actions

1. **`core.download_packs`** - Wraps `/api/v1/packs/download`
2. **`core.get_pack_dependencies`** - Wraps `/api/v1/packs/dependencies`
3. **`core.build_pack_envs`** - Wraps `/api/v1/packs/build-envs`
4. **`core.register_packs`** - Wraps `/api/v1/packs/register-batch`

### Action Parameters

Each action accepts parameters that map directly to the API request body, plus:

- `api_url` (string, optional) - API base URL
  - Default: `http://localhost:8080`
- `api_token` (string, optional) - Authentication token
  - If not provided, uses system authentication

### Example Action Execution

```bash
attune action execute core.download_packs \
  --param packs='["https://github.com/attune-io/pack-aws.git"]' \
  --param destination_dir=/tmp/packs
```

---

## Workflow Example

Complete pack installation workflow using the API:

```yaml
# workflows/install_pack.yaml
name: install_pack
description: Complete pack installation workflow
version: 1.0.0

input:
  - pack_source
  - destination_dir

tasks:
  # Stage 1: Download
  download:
    action: core.download_packs
    input:
      packs:
        - <% ctx().pack_source %>
      destination_dir: <% ctx().destination_dir %>
    next:
      - when: <% succeeded() %>
        publish:
          - pack_paths: <% result().downloaded_packs.select($.pack_path) %>
        do: analyze_deps

  # Stage 2: Analyze Dependencies
  analyze_deps:
    action: core.get_pack_dependencies
    input:
      pack_paths: <% ctx().pack_paths %>
    next:
      - when: <% succeeded() and result().missing_dependencies.len() = 0 %>
        do: build_envs
      - when: <% succeeded() and result().missing_dependencies.len() > 0 %>
        do: fail
        publish:
          - error: "Missing dependencies: <% result().missing_dependencies %>"

  # Stage 3: Build Environments
  build_envs:
    action: core.build_pack_envs
    input:
      pack_paths: <% ctx().pack_paths %>
    next:
      - when: <% succeeded() %>
        do: register

  # Stage 4: Register Packs
  register:
    action: core.register_packs
    input:
      pack_paths: <% ctx().pack_paths %>
      skip_tests: false

output:
  - registered_packs: <% task(register).result.registered_packs %>
```

---

## Error Handling

All endpoints return consistent error responses:

```json
{
  "error": "Error message",
  "message": "Detailed error description",
  "status": 400
}
```

### Common Error Scenarios

1. **Missing Authentication**
   - Status: 401
   - Solution: Provide valid Bearer token

2. **Invalid Pack Path**
   - Reported in `errors` array within 200 response
   - Solution: Verify pack paths exist and are readable

3. **Missing Dependencies**
   - Reported in `missing_dependencies` array
   - Solution: Install dependencies first or use `skip_deps: true`

4. **Runtime Not Available**
   - Reported in `failed_environments` array
   - Solution: Install required Python/Node.js version

5. **Pack Already Registered**
   - Status: 400 (or in `failed_packs` for batch)
   - Solution: Use `force: true` to re-register

---

## Best Practices

### 1. Download Strategy

- **Registry packs**: Use atomic `attune pack install PACK_REF`, not staged download
- **Git repos**: Use full URLs with version tags
- **Local packs**: Use absolute paths

### 2. Dependency Management

- Always run dependency analysis after download
- Install missing dependencies before registration
- Use pack registry to resolve dependency versions

### 3. Environment Building

- Check for existing environments before rebuilding
- Use `force_rebuild: true` sparingly (time-consuming)
- Verify Python/Node.js availability before starting

### 4. Registration

- Run tests unless in development (`skip_tests: false` in production)
- Use validation to catch configuration errors early
- Enable `force: true` only when intentionally updating

### 5. Error Recovery

- Check individual pack results in batch operations
- Retry failed downloads with exponential backoff
- Log all errors for troubleshooting

---

## CLI Integration

Use the Attune CLI to execute pack installation actions:

```bash
# Download packs
attune action execute core.download_packs \
  --param packs='["https://github.com/attune-io/pack-aws.git"]' \
  --param destination_dir=/tmp/packs

# Analyze dependencies
attune action execute core.get_pack_dependencies \
  --param pack_paths='["/tmp/packs/core"]'

# Build environments
attune action execute core.build_pack_envs \
  --param pack_paths='["/tmp/packs/core"]'

# Register packs
attune action execute core.register_packs \
  --param pack_paths='["/tmp/packs/core"]'
```

---

## Future Enhancements

### Planned Features

1. **Actual Environment Building**
   - Create Python virtualenvs
   - Install requirements.txt dependencies
   - Run npm/yarn install for Node.js packs

2. **Progress Streaming**
   - WebSocket updates during long operations
   - Real-time download/build progress

3. **Pack Validation**
   - Schema validation before registration
   - Dependency conflict detection
   - Version compatibility checks

4. **Rollback Support**
   - Snapshot packs before updates
   - Rollback to previous versions
   - Automatic cleanup on failure

5. **Cache Management**
   - Cache downloaded packs
   - Reuse existing environments
   - Clean up stale installations

---

## Related Documentation

- [Pack Structure](../packs/pack-structure.md)
- [Pack Registry Specification](../packs/pack-registry-spec.md)
- [Pack Testing Framework](../packs/pack-testing-framework.md)
- [CLI Documentation](../cli/cli.md)
- [Workflow System](../workflows/workflow-summary.md)
