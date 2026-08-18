# Quick Reference: Pack Management API

**Last Updated:** 2026-02-05

## Overview

Four API endpoints for pack installation workflow:
1. **Download** - Fetch packs from sources
2. **Dependencies** - Analyze requirements
3. **Build Envs** - Prepare runtimes (detection mode)
4. **Register** - Import to database

All endpoints require Bearer token authentication.

---

## 1. Download Packs

```bash
POST /api/v1/packs/download  # explicit Git/archive URLs only
```

**Minimal Request:**
```json
{
  "packs": ["https://github.com/attune-io/pack-aws.git"]
}
```

**Full Request:**
```json
{
  "packs": ["https://github.com/attune-io/pack-aws.git"],
  "ref_spec": "v1.0.0"
}
```

**Response:**
```json
{
  "data": {
    "downloaded_packs": [...],
    "failed_packs": [...],
    "total_count": 2,
    "success_count": 1,
    "failure_count": 1
  }
}
```

**cURL Example:**
```bash
curl -X POST http://localhost:8080/api/v1/packs/download \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"packs":["https://github.com/attune-io/pack-aws.git"]}'
```

---

## 2. Get Dependencies

```bash
POST /api/v1/packs/dependencies
```

**Request:**
```json
{
  "pack_paths": ["/tmp/packs/core"],
  "skip_validation": false
}
```

**Response:**
```json
{
  "data": {
    "dependencies": [...],
    "runtime_requirements": {...},
    "missing_dependencies": [...],
    "analyzed_packs": [...],
    "errors": []
  }
}
```

**cURL Example:**
```bash
curl -X POST http://localhost:8080/api/v1/packs/dependencies \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"pack_paths":["/tmp/packs/core"]}'
```

---

## 3. Build Environments

```bash
POST /api/v1/packs/build-envs
```

**Minimal Request:**
```json
{
  "pack_paths": ["/tmp/packs/aws"],
  "packs_base_dir": "/opt/attune/packs"
}
```

**Full Request:**
```json
{
  "pack_paths": ["/tmp/packs/aws"],
  "packs_base_dir": "/opt/attune/packs",
  "python_version": "3.11",
  "nodejs_version": "20",
  "skip_python": false,
  "skip_nodejs": false,
  "force_rebuild": false,
  "timeout": 600
}
```

**Response:**
```json
{
  "data": {
    "built_environments": [...],
    "failed_environments": [...],
    "summary": {
      "total_packs": 1,
      "success_count": 1,
      "python_envs_built": 1,
      "nodejs_envs_built": 0
    }
  }
}
```

**Note:** Currently in detection mode - checks runtime availability but doesn't build full environments.

**cURL Example:**
```bash
curl -X POST http://localhost:8080/api/v1/packs/build-envs \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"pack_paths":["/tmp/packs/core"],"packs_base_dir":"/opt/attune/packs"}'
```

---

## 4. Register Packs (Batch)

```bash
POST /api/v1/packs/register-batch
```

**Minimal Request:**
```json
{
  "pack_paths": ["/opt/attune/packs/core"],
  "packs_base_dir": "/opt/attune/packs"
}
```

**Full Request:**
```json
{
  "pack_paths": ["/opt/attune/packs/core"],
  "packs_base_dir": "/opt/attune/packs",
  "skip_validation": false,
  "skip_tests": false,
  "force": false
}
```

**Response:**
```json
{
  "data": {
    "registered_packs": [...],
    "failed_packs": [...],
    "summary": {
      "total_packs": 1,
      "success_count": 1,
      "failure_count": 0,
      "total_components": 46,
      "duration_ms": 1500
    }
  }
}
```

**cURL Example:**
```bash
curl -X POST http://localhost:8080/api/v1/packs/register-batch \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"pack_paths":["/opt/attune/packs/core"],"packs_base_dir":"/opt/attune/packs","skip_tests":true}'
```

---

## Action Wrappers

Execute via CLI or workflows:

```bash
# Download
attune action execute core.download_packs \
  --param packs='["https://github.com/attune-io/pack-aws.git"]' \
  --param destination_dir=/tmp/packs

# Analyze dependencies
attune action execute core.get_pack_dependencies \
  --param pack_paths='["/tmp/packs/core"]'

# Build environments
attune action execute core.build_pack_envs \
  --param pack_paths='["/tmp/packs/core"]'

# Register
attune action execute core.register_packs \
  --param pack_paths='["/opt/attune/packs/core"]' \
  --param skip_tests=true
```

---

## Complete Workflow Example

```bash
#!/bin/bash
TOKEN=$(attune auth token)

# 1. Download
DOWNLOAD=$(curl -s -X POST http://localhost:8080/api/v1/packs/download \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"packs":["https://github.com/attune-io/pack-aws.git"]}')

PACK_PATH=$(echo "$DOWNLOAD" | jq -r '.data.downloaded_packs[0].pack_path')

# 2. Check dependencies
curl -X POST http://localhost:8080/api/v1/packs/dependencies \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"pack_paths\":[\"$PACK_PATH\"]}"

# 3. Build/check environments
curl -X POST http://localhost:8080/api/v1/packs/build-envs \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"pack_paths\":[\"$PACK_PATH\"]}"

# 4. Register
curl -X POST http://localhost:8080/api/v1/packs/register-batch \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"pack_paths\":[\"$PACK_PATH\"],\"skip_tests\":true}"
```

---

## Common Parameters

### Source Formats (download)
- **Git URL:** `"https://github.com/org/repo.git"`
- **Archive URL:** `"https://example.com/pack.tar.gz"`
- **API-visible local path:** `"/path/to/pack"`

Registry names use `POST /api/v1/packs/install` or `attune pack install`.

### Auth Token
```bash
# Get token via CLI
TOKEN=$(attune auth token)

# Or login directly
LOGIN=$(curl -X POST http://localhost:8080/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email":"user@example.com","password":"pass"}')
TOKEN=$(echo "$LOGIN" | jq -r '.data.access_token')
```

---

## Error Handling

All endpoints return 200 with per-pack results:

```json
{
  "data": {
    "successful_items": [...],
    "failed_items": [
      {
        "pack_ref": "unknown",
        "error": "pack.yaml not found"
      }
    ]
  }
}
```

Check `success_count` vs `failure_count` in summary.

---

## Best Practices

1. **Check authentication first** - Verify token works
2. **Process downloads** - Check `downloaded_packs` array
3. **Validate dependencies** - Ensure `missing_dependencies` is empty
4. **Skip tests in dev** - Use `skip_tests: true` for faster iteration
5. **Use force carefully** - Only re-register when needed

---

## Testing Quick Start

```bash
# 1. Start API
make run-api

# 2. Get token
TOKEN=$(curl -s -X POST http://localhost:8080/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email":"test@attune.local","password":"TestPass123!"}' \
  | jq -r '.data.access_token')

# 3. Test endpoint
curl -X POST http://localhost:8080/api/v1/packs/dependencies \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"pack_paths":[]}' | jq
```

---

## Related Docs

- **Full API Docs:** [api-pack-installation.md](api/api-pack-installation.md)
- **Pack Structure:** [pack-structure.md](packs/pack-structure.md)
- **Registry Spec:** [pack-registry-spec.md](packs/pack-registry-spec.md)
- **CLI Guide:** [cli.md](cli/cli.md)
