# Pack Installation from Git Repositories

**Last Updated**: 2025-01-27  
**Status**: Production Ready

---

## Overview

Attune supports installing packs directly from git repositories, enabling teams to:

- **Version Control**: Track pack versions using git tags, branches, and commits
- **Collaboration**: Share packs across teams via git hosting services
- **Automation**: Integrate pack deployment into CI/CD pipelines
- **Development**: Test packs from feature branches before release

---

## Quick Start

### Web UI Installation

1. Navigate to **Packs** page
2. Click **Add Pack** dropdown → **Install from Remote**
3. Select **Git Repository** as source type
4. Enter an approved HTTPS repository URL
5. Optionally specify a git reference (branch, tag, or commit)
6. Configure installation options
7. Click **Install Pack**

### CLI Installation

```bash
# Install from default branch
attune pack install https://github.com/example/pack-slack.git

# Install from specific tag
attune pack install https://github.com/example/pack-slack.git --ref v2.1.0

# Install from branch
attune pack install https://github.com/example/pack-slack.git --ref main

# Install from commit hash
attune pack install https://github.com/example/pack-slack.git --ref a1b2c3d

# Skip tests and dependency validation (use with caution)
attune pack install https://github.com/example/pack-slack.git --skip-tests --skip-deps
```

### API Installation

```bash
curl -X POST http://localhost:8080/api/v1/packs/install \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "source": "https://github.com/example/pack-slack.git",
    "ref_spec": "v2.1.0",
    "force": false,
    "skip_tests": false,
    "skip_deps": false
  }'
```

---

## Supported Git Sources

### HTTPS URLs

**Public repositories:**
```
https://github.com/username/pack-name.git
https://gitlab.com/username/pack-name.git
https://bitbucket.org/username/pack-name.git
```

Only HTTPS Git URLs are accepted. Credential-bearing URLs, SSH, `git://`, and
`file://` sources are rejected. For private distribution, publish an archive
through an authenticated registry index instead of relying on server Git
credentials.

---

## Git References

The `ref_spec` parameter accepts any valid git reference:

### Branches
```bash
# Default branch (usually main or master)
--ref main

# Development branch
--ref develop

# Feature branch
--ref feature/new-action
```

### Tags
```bash
# Semantic version tag
--ref v1.2.3

# Release tag
--ref release-2024-01-27
```

### Commit Hashes
```bash
# Full commit hash
--ref a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0

# Short commit hash (7+ characters)
--ref a1b2c3d
```

### Special References
```bash
# HEAD of default branch
--ref HEAD

# No ref specified = default branch
# (equivalent to --depth 1 clone)
```

---

## Pack Structure Requirements

The git repository must contain a valid pack structure:

### Option 1: Root-Level Pack
```
repository-root/
├── pack.yaml          # Required
├── actions/           # Optional
├── sensors/           # Optional
├── caches/            # Optional declarative Data Cache namespaces
├── triggers/          # Optional
├── rules/             # Optional
├── workflows/         # Optional
└── README.md          # Recommended
```

### Option 2: Pack Subdirectory
```
repository-root/
├── pack/
│   ├── pack.yaml      # Required
│   ├── actions/
│   └── ...
├── docs/
└── README.md
```

> The installer will automatically detect and use the pack directory.

---

## Installation Process

When you install a pack from git, Attune performs these steps:

### 1. Clone Repository
```
git clone [--depth 1] <url> <temp-dir>
```

If a `ref_spec` is provided:
```
git checkout <ref_spec>
```

### 2. Locate Pack Directory
- Search for `pack.yaml` at repository root
- If not found, search in `pack/` subdirectory
- Fail if `pack.yaml` not found in either location

### 3. Validate Dependencies (unless skipped)
- Extract runtime dependencies from `pack.yaml`
- Extract pack dependencies
- Verify all dependencies are satisfied
- Fail if dependencies are missing (unless `force` enabled)

### 4. Register Pack
- Parse `pack.yaml` metadata
- Create database entry
- Auto-sync workflows if present

### 5. Execute Tests (unless skipped)
- Run pack test suite if configured
- Fail if tests don't pass (unless `force` enabled)

### 6. Copy to Permanent Storage
- Move pack files to `{packs_base_dir}/{pack_ref}/`
- Calculate directory checksum
- Store installation metadata

### 7. Record Installation
- Store installation record in `pack_installation` table
- Record source URL, git ref, timestamp, checksum
- Link to installing user

---

## Installation Options

### Force Installation
**Flag**: `--force` (CLI) or `force: true` (API)

Enables:
- Reinstall pack even if it already exists (replaces existing)
- Proceed even if dependencies are missing
- Proceed even if tests fail

**Use Cases**:
- Upgrading pack to new version
- Recovering from failed installation
- Development/testing workflows

**Warning**: Force mode bypasses safety checks. Use cautiously in production.

### Skip Tests
**Flag**: `--skip-tests` (CLI) or `skip_tests: true` (API)

- Skip executing pack test suite
- Faster installation
- Useful when tests are slow or not available

**Use Cases**:
- Installing trusted packs
- Tests not yet implemented
- Development environments

### Skip Dependencies
**Flag**: `--skip-deps` (CLI) or `skip_deps: true` (API)

- Skip validation of runtime dependencies
- Skip validation of pack dependencies
- May result in runtime failures if dependencies truly missing

**Use Cases**:
- Dependencies will be installed separately
- Custom runtime environment
- Development/testing

---

## Example Workflows

### Development Workflow

```bash
# 1. Install pack from feature branch for testing
attune pack install https://github.com/myorg/pack-custom.git \
  --ref feature/new-sensor \
  --skip-tests \
  --force

# 2. Test the pack
attune pack test custom

# 3. When satisfied, install from main/release tag
attune pack install https://github.com/myorg/pack-custom.git \
  --ref v1.0.0 \
  --force
```

### Production Deployment

```bash
# Install specific version with full validation
attune pack install https://github.com/myorg/pack-slack.git \
  --ref v2.1.0

# Verify installation
attune pack list | grep slack
attune pack test slack
```

### CI/CD Pipeline

```yaml
# GitHub Actions example
- name: Install pack on staging
  run: |
    attune pack install https://github.com/${{ github.repository }}.git \
      --ref ${{ github.sha }} \
      --force
  env:
    ATTUNE_API_TOKEN: ${{ secrets.ATTUNE_TOKEN }}

- name: Run tests
  run: attune pack test $(basename ${{ github.repository }})
```

## Troubleshooting

### Git Clone Fails

**Error**: `Git clone failed: Permission denied`

**Solutions**:
- Check repository access permissions
- Verify the repository is anonymously readable over HTTPS
- For private packs, use an authenticated registry archive

### Ref Not Found

**Error**: `Git checkout failed: pathspec 'v1.0.0' did not match any file(s) known to git`

**Solutions**:
- Verify tag/branch exists in repository
- Check spelling and case sensitivity
- Ensure ref is pushed to remote
- Try with full commit hash

### Pack.yaml Not Found

**Error**: `pack.yaml not found in directory`

**Solutions**:
- Ensure `pack.yaml` exists at repository root or in `pack/` subdirectory
- Check file name spelling (case-sensitive on Linux)
- Verify correct branch/tag is checked out

### Dependency Validation Failed

**Error**: `Pack dependency validation failed: pack 'core' version '^1.0.0' not found`

**Solutions**:
- Install missing dependencies first
- Use `--skip-deps` to bypass validation (not recommended)
- Use `--force` to install anyway

### Test Failures

**Error**: `Pack registration failed: tests did not pass`

**Solutions**:
- Review test output for specific failures
- Fix issues in pack code
- Use `--skip-tests` to install without testing
- Use `--force` to install despite test failures

---

## Security Considerations

### HTTPS Sources
- Never embed credentials directly in URLs
- Explicitly approve every public Git host
- Use authenticated registry archives for private packs

### Code Review
- Review pack code before installation
- Install from tagged releases, not branches
- Verify pack author/source
- Check for malicious code in actions/sensors

### Git References
- **Production**: Use specific tags (e.g., `v1.2.3`)
- **Staging**: Use release branches (e.g., `release-*`)
- **Development**: Feature branches acceptable
- **Avoid**: `main`/`master` in production (may change unexpectedly)

---

## Advanced Topics

### Submodules

If pack repository uses git submodules:

```bash
# Clone with submodules
git clone --recurse-submodules <url>
```

> **Note**: Current implementation does not automatically clone submodules. Manual configuration required.

### Large Repositories

For large repositories, use shallow clones:

```bash
# Default behavior when no ref_spec
git clone --depth 1 <url>
```

When specific ref is needed:
```bash
git clone <url>
git checkout <ref>
```

### Monorepos

For repositories containing multiple packs:

```
monorepo/
├── pack-a/
│   └── pack.yaml
├── pack-b/
│   └── pack.yaml
└── pack-c/
    └── pack.yaml
```

**Limitation**: Current implementation expects one pack per repository. For monorepos, use filesystem registration or create separate repositories.

---

## Database Schema

Installation metadata is stored in the `pack_installation` table:

```sql
CREATE TABLE pack_installation (
  id BIGSERIAL PRIMARY KEY,
  pack_id BIGINT NOT NULL REFERENCES pack(id),
  source_type VARCHAR(50) NOT NULL,        -- 'git'
  source_url TEXT,                          -- Git repository URL
  source_ref TEXT,                          -- Branch/tag/commit
  checksum TEXT,                            -- Directory checksum
  checksum_verified BOOLEAN DEFAULT FALSE,
  installed_by BIGINT REFERENCES identity(id),
  installation_method VARCHAR(50),          -- 'api', 'cli', 'web'
  storage_path TEXT NOT NULL,               -- File system path
  meta JSONB,                               -- Additional metadata
  created TIMESTAMP DEFAULT NOW(),
  updated TIMESTAMP DEFAULT NOW()
);
```

---

## API Reference

### Install Pack Endpoint

**POST** `/api/v1/packs/install`

**Request Body**:
```json
{
  "source": "https://github.com/example/pack-slack.git",
  "ref_spec": "v2.1.0",
  "force": false,
  "skip_tests": false,
  "skip_deps": false
}
```

**Response** (201 Created):
```json
{
  "data": {
    "pack": {
      "id": 42,
      "ref": "slack",
      "label": "Slack Pack",
      "version": "2.1.0",
      "description": "Slack integration pack",
      "is_standard": false,
      ...
    },
    "test_result": {
      "status": "passed",
      "total_tests": 10,
      "passed": 10,
      "failed": 0,
      ...
    },
    "tests_skipped": false
  }
}
```

**Error Responses**:
- `400`: Invalid request, dependency validation failed, or tests failed
- `409`: Pack already exists (use `force: true` to override)
- `500`: Internal error (git clone failed, filesystem error, etc.)

---

## Future Enhancements

### Planned Features
- Git submodule support
- Monorepo support (install specific subdirectory)
- Pack version upgrade workflow
- Automatic version detection from tags
- Git LFS support
- Signature verification for signed commits

### Registry Integration
When pack registry system is implemented:
- Registry can reference git repositories
- Automatic version discovery from git tags
- Centralized pack metadata
- See `pack-registry-spec.md` for details

---

## Related Documentation

- [Pack Structure](pack-structure.md) - Pack directory and file format
- [Pack Registry Specification](pack-registry-spec.md) - Registry-based installation
- [Pack Testing Framework](pack-testing-framework.md) - Testing packs
- [Configuration](../configuration/configuration.md) - Server configuration
- [Production Deployment](../deployment/production-deployment.md) - Deployment guide

---

## Examples Repository

Example packs demonstrating git-based installation:

```bash
# Simple action pack
attune pack install https://github.com/attune-examples/pack-hello-world.git

# Complex pack with dependencies
attune pack install https://github.com/attune-examples/pack-kubernetes.git --ref v1.0.0

```

---

## Support

For issues or questions:
- GitHub Issues: https://github.com/attune-io/attune/issues
- Documentation: https://docs.attune.io/packs/installation
- Community: https://community.attune.io
