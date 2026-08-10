# Pack Registry and Installation Specification

**Last Updated**: 2024-01-20  
**Status**: Specification (Pre-Implementation)

---

## Overview

This document specifies the pack registry and installation system for Attune, enabling community-driven pack distribution. The system allows packs to be:

- Published to independent registries (no central authority required)
- Installed from git repositories, HTTP/HTTPS URLs, or local sources
- Discovered through configurable registry indices
- Validated and tested during installation

---

## Design Goals

1. **Decentralized**: No single point of failure; anyone can host a registry
2. **CI/CD Friendly**: Integrate with existing build and artifact storage systems
3. **Flexible Sources**: Support multiple installation sources (git, HTTP, local)
4. **Priority-Based Discovery**: Search multiple registries in configured order
5. **Secure**: Validate checksums and signatures (future)
6. **Automated**: Install dependencies, run tests, register components automatically

---

## Pack Index File Format

### Index Structure

Each registry hosts an **index file** (typically `index.json`) that catalogs available packs.

**Format**: JSON  
**Location**: Configurable URL (HTTPS recommended)  
**Filename Convention**: `index.json` or `registry.json`

### Index Schema

```json
{
  "registry_name": "Attune Community Registry",
  "registry_url": "https://registry.attune.io",
  "version": "1.0",
  "last_updated": "2024-01-20T12:00:00Z",
  "packs": [
    {
      "ref": "slack",
      "label": "Slack Integration",
      "description": "Send messages, upload files, and monitor Slack channels",
      "use_case": "Slack ChatOps actions, notifications, and message-triggered automation",
      "version": "2.1.0",
      "author": "Attune Team",
      "email": "team@attune.io",
      "homepage": "https://github.com/attune-io/pack-slack",
      "repository": "https://github.com/attune-io/pack-slack",
      "license": "Apache-2.0",
      "keywords": ["slack", "messaging", "notifications"],
      "runtime_deps": ["python3"],
      
      "install_sources": [
        {
          "type": "git",
          "url": "https://github.com/attune-io/pack-slack.git",
          "ref": "v2.1.0",
          "checksum": "sha256:abc123..."
        },
        {
          "type": "archive",
          "url": "https://github.com/attune-io/pack-slack/archive/refs/tags/v2.1.0.zip",
          "checksum": "sha256:def456..."
        }
      ],
      
      "contents": {
        "actions": [
          {
            "name": "send_message",
            "description": "Send a message to a Slack channel"
          },
          {
            "name": "upload_file",
            "description": "Upload a file to Slack"
          }
        ],
        "sensors": [
          {
            "name": "message_sensor",
            "description": "Monitor Slack messages"
          }
        ],
        "triggers": [
          {
            "name": "message_received",
            "description": "Fires when a message is received"
          }
        ],
        "rules": [],
        "workflows": []
      },
      
      "dependencies": {
        "attune_version": ">=0.1.0",
        "python_version": ">=3.9",
        "packs": []
      },
      
      "meta": {
        "downloads": 1543,
        "stars": 87,
        "tested_attune_versions": ["0.1.0", "0.2.0"]
      }
    }
  ]
}
```

### Field Definitions

#### Registry Metadata

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `registry_name` | string | Yes | Human-readable registry name |
| `registry_url` | string | Yes | Registry homepage URL |
| `version` | string | Yes | Index format version (semantic versioning) |
| `last_updated` | string | Yes | ISO 8601 timestamp of last update |
| `packs` | array | Yes | Array of pack entries |

#### Pack Entry

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `ref` | string | Yes | Unique pack identifier (matches pack.yaml) |
| `label` | string | Yes | Human-readable pack name |
| `description` | string | Yes | Brief pack description |
| `use_case` | string | No | Short browse/install summary describing what the pack is useful for |
| `version` | string | Yes | Semantic version (latest available) |
| `author` | string | Yes | Pack author/maintainer name |
| `email` | string | No | Contact email |
| `homepage` | string | No | Pack homepage URL |
| `repository` | string | No | Source repository URL |
| `license` | string | Yes | SPDX license identifier |
| `keywords` | array[string] | No | Searchable keywords/tags |
| `runtime_deps` | array[string] | Yes | Required runtimes (python3, nodejs, shell) |
| `install_sources` | array[object] | Yes | Available installation sources (see below) |
| `contents` | object | Yes | Pack components summary |
| `dependencies` | object | No | Pack dependencies |
| `meta` | object | No | Additional metadata |

#### Install Source

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | string | Yes | Source type: "git" or "archive" |
| `url` | string | Yes | Source URL |
| `ref` | string | No | Git ref (tag, branch, commit) for git type |
| `checksum` | string | Yes | Format: "algorithm:hash" (e.g., "sha256:abc...") |

### Configured Index Ordering

Attune stores configured index URLs in the `pack_registry_index` table. Indices
have an integer `position` internally; lower positions are searched first. New
indices append to the end by default, and administrators reorder them through
the web client's drag handle or by updating positions through the API/CLI. When
the same pack `ref` appears in multiple enabled indices, the first enabled index
containing that ref wins for browse/detail/install resolution.

The static `pack_registry.indices` YAML configuration remains supported as a
bootstrap fallback. API-managed indices take precedence once any rows exist in
`pack_registry_index`.

### Management API

The API exposes the configured index and browse surfaces under:

| Endpoint | Description |
|----------|-------------|
| `GET /api/v1/pack-indices` | List API-managed indices in search order |
| `POST /api/v1/pack-indices` | Add an index (`url`, optional `name`, `position`, `enabled`, `headers`); omitted `position` appends to the end |
| `PUT /api/v1/pack-indices/{id}` | Update an index |
| `DELETE /api/v1/pack-indices/{id}` | Delete an index |
| `GET /api/v1/pack-indices/packs?q=...` | Browse de-duplicated indexed packs using first-index-wins order |
| `GET /api/v1/pack-indices/packs/{ref}` | Show the pack entry selected by configured index order |

`POST /api/v1/packs/install` accepts a pack ref (for example `slack` or
`slack@2.1.0`) and resolves it through the same ordered indices before
selecting the preferred install source.

#### Contents Object

| Field | Type | Description |
|-------|------|-------------|
| `actions` | array[object] | List of actions with name and description |
| `sensors` | array[object] | List of sensors with name and description |
| `triggers` | array[object] | List of triggers with name and description |
| `rules` | array[object] | List of bundled rules |
| `workflows` | array[object] | List of bundled workflows |

#### Dependencies Object

| Field | Type | Description |
|-------|------|-------------|
| `attune_version` | string | Semver requirement (e.g., ">=0.1.0", "^1.0.0") |
| `python_version` | string | Python version requirement |
| `nodejs_version` | string | Node.js version requirement |
| `packs` | array[string] | Pack dependencies (format: "ref@version") |

---

## Pack Sources

Packs can be installed from multiple source types:

### 1. Git Repository

Install directly from a git repository:

```bash
attune pack install https://github.com/example/pack-slack.git
attune pack install https://github.com/example/pack-slack.git --ref v2.1.0
attune pack install https://github.com/example/pack-slack.git --ref main
```

**Requirements**:
- Repository must contain valid pack structure at root or in `pack/` subdirectory
- `pack.yaml` must be present
- Git client must be installed on system
- The HTTPS host must be explicitly approved; SSH and credential-bearing URLs are rejected

### 2. Archive URL

Install from a zip or tar.gz archive:

```bash
attune pack install https://example.com/packs/slack-2.1.0.zip
attune pack install https://example.com/packs/slack-2.1.0.tar.gz
```

**Requirements**:
- Archive must contain pack directory structure
- Archive root or single top-level directory must contain `pack.yaml`
- Supported formats: `.zip`, `.tar.gz`, `.tgz`

### 3. Local Directory

Install from a local filesystem path:

```bash
attune pack install /path/to/pack-slack
attune pack install ./packs/my-pack
```

**Requirements**:
- Directory must contain valid pack structure
- `pack.yaml` must be present
- Used for development and testing

### 4. Local Archive

Upload and install from a local archive file:

```bash
attune pack install /path/to/pack-slack-2.1.0.zip
attune pack install ./my-pack.tar.gz
```

**Requirements**:
- Archive must contain valid pack structure
- Archive is uploaded to Attune API before installation
- Used for air-gapped or offline installations

### 5. Registry Reference

Install by pack reference, searching configured registries:

```bash
attune pack install slack
attune pack install slack@2.1.0
attune pack install slack@latest
```

**Requirements**:
- At least one registry must be configured
- Pack reference must exist in one of the registries
- Registries searched in configured priority order

---

## Configuration

### Registry Configuration

Add registry URLs to service configuration files:

**YAML Configuration** (`config.yaml`):

```yaml
pack_registry:
  enabled: true
  indices:
    - url: https://registry.attune.io/index.json
      priority: 1
      enabled: true
      name: "Official Attune Registry"
    
    - url: https://company-internal.example.com/attune-registry.json
      priority: 2
      enabled: true
      name: "Company Internal Registry"
      headers:
        Authorization: "Bearer ${REGISTRY_TOKEN}"
    
  approved_public_hosts:
    - registry.attune.io
    - github.com
    - codeload.github.com
    - objects.githubusercontent.com
  approved_private_hosts:
    - company-internal.example.com
  approved_private_cidrs:
    - 10.20.0.0/16

  # Cache settings
  cache_ttl: 3600  # Cache index for 1 hour
  cache_enabled: true
  
  # Download settings
  timeout: 120
  connect_timeout: 10
  index_max_bytes: 10485760
  archive_max_bytes: 104857600
  verify_checksums: true
  allow_http: false  # Only allow HTTPS
```

**Environment Variables**:

```bash
# Enable/disable registry
export ATTUNE__PACK_REGISTRY__ENABLED=true

# Set registry URLs (comma-separated, in priority order)
export ATTUNE__PACK_REGISTRY__INDICES="https://registry.attune.io/index.json,https://internal.example.com/registry.json"

# Cache settings
export ATTUNE__PACK_REGISTRY__CACHE_TTL=3600
export ATTUNE__PACK_REGISTRY__VERIFY_CHECKSUMS=true
```

### Priority-Based Search

Registries are searched in **priority order** (lowest priority number first):

1. **Priority 1**: Official Attune Registry (public packs)
2. **Priority 2**: Company Internal Registry (private packs)

When installing by reference (e.g., `attune pack install slack`):
- Search priority 1 registry first
- If not found, search priority 2
- If not found in any registry, return error

**Use Cases**:
- **Override public packs**: Company registry can provide custom version of "slack" pack
- **Private packs**: Internal registry can host proprietary packs
- **Development**: An explicitly approved HTTPS development registry can provide development versions

### Registry Headers

For authenticated registries, configure custom HTTP headers:

```yaml
pack_registry:
  indices:
    - url: https://private-registry.example.com/index.json
      headers:
        Authorization: "Bearer ${PRIVATE_REGISTRY_TOKEN}"
        X-Custom-Header: "value"
```

---

## CLI Commands

### Install Pack

```bash
# From registry (by reference)
attune pack install <pack-ref>[@version]

# From git repository
attune pack install <git-url> [--ref <branch|tag|commit>]

# From archive URL
attune pack install <https-url>

# From local directory
attune pack install <local-path>

# From local archive
attune pack install <local-archive-path>

# Options
--force                 # Force reinstall if already exists
--skip-tests            # Skip running pack tests
--skip-deps             # Skip installing dependencies
--registry <name>       # Use specific registry (skip priority search)
--no-registry           # Don't search registries (direct install only)
```

### Examples

```bash
# Install latest version from registry
attune pack install slack

# Install specific version from registry
attune pack install slack@2.1.0

# Install from git repository (latest tag)
attune pack install https://github.com/example/pack-slack.git

# Install from git repository (specific tag)
attune pack install https://github.com/example/pack-slack.git --ref v2.1.0

# Install from git repository (branch)
attune pack install https://github.com/example/pack-slack.git --ref main

# Install from archive URL
attune pack install https://example.com/packs/slack-2.1.0.zip

# Install from local directory (development)
attune pack install ./packs/my-pack

# Install from local archive
attune pack install ./slack-2.1.0.zip

# Force reinstall
attune pack install slack --force

# Skip tests (faster, but not recommended)
attune pack install slack --skip-tests
```

### Generate Index Entry

For pack maintainers, generate an index entry from a pack:

```bash
attune pack index-entry \
  --pack-dir <path-to-pack> \
  --version <version> \
  --git-url <git-repo-url> \
  --git-ref <tag-or-branch> \
  --archive-url <archive-url>

# Output to stdout (JSON)
attune pack index-entry --pack-dir ./pack-slack --version 2.1.0 \
  --git-url https://github.com/example/pack-slack.git \
  --git-ref v2.1.0 \
  --archive-url https://example.com/packs/slack-2.1.0.zip

# Append to existing index file
attune pack index-entry --pack-dir ./pack-slack --version 2.1.0 \
  --git-url https://github.com/example/pack-slack.git \
  --git-ref v2.1.0 \
  --archive-url https://example.com/packs/slack-2.1.0.zip \
  --index-file registry.json \
  --output registry.json
```

**Output Example**:

```json
{
  "ref": "slack",
  "label": "Slack Integration",
  "description": "Send messages, upload files, and monitor Slack channels",
  "version": "2.1.0",
  "author": "Example Team",
  "email": "team@example.com",
  "license": "Apache-2.0",
  "runtime_deps": ["python3"],
  "install_sources": [
    {
      "type": "git",
      "url": "https://github.com/example/pack-slack.git",
      "ref": "v2.1.0",
      "checksum": "sha256:abc123..."
    },
    {
      "type": "archive",
      "url": "https://example.com/packs/slack-2.1.0.zip",
      "checksum": "sha256:def456..."
    }
  ],
  "contents": {
    "actions": [...],
    "sensors": [...],
    "triggers": [...]
  }
}
```

### Update Index File

Merge multiple index entries or update an existing index:

```bash
# Add entry to index
attune pack index-update --index registry.json --entry entry.json

# Merge multiple indices
attune pack index-merge --output combined.json registry1.json registry2.json

# Update pack version in index
attune pack index-update --index registry.json --pack slack --version 2.1.1 \
  --git-ref v2.1.1 --archive-url https://example.com/packs/slack-2.1.1.zip
```

### List Registries

```bash
attune pack registries

# Output:
# Priority | Name                    | URL                                      | Status
# ---------|-------------------------|------------------------------------------|--------
# 1        | Official Attune Registry| https://registry.attune.io/index.json    | Online
# 2        | Company Internal        | https://internal.example.com/registry.json| Online
# 3        | GitHub Releases         | https://example.github.io/registry.json  | Online
```

### Search Registry

```bash
# Search all registries
attune pack search <keyword>

# Search specific registry
attune pack search <keyword> --registry "Official Attune Registry"

# Example
attune pack search slack

# Output:
# Ref    | Version | Description                              | Registry
# -------|---------|------------------------------------------|-------------------------
# slack  | 2.1.0   | Send messages and monitor Slack channels | Official Attune Registry
```

---

## Installation Process

### Installation Workflow

```
┌─────────────────────────────────────────────────────────────────────┐
│ 1. Source Resolution                                                │
│    - Registry reference → Search indices → Resolve install source   │
│    - Direct URL → Use provided source                               │
│    - Local path → Use local filesystem                              │
└────────────────┬────────────────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 2. Download/Copy Pack                                               │
│    - Git: Clone repository to temp directory                        │
│    - Archive: Download and extract to temp directory                │
│    - Local: Copy to temp directory                                  │
└────────────────┬────────────────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 3. Validate Pack Structure                                          │
│    - Verify pack.yaml exists and is valid                           │
│    - Verify pack ref matches (if installing from registry)          │
│    - Verify version matches (if specified)                          │
│    - Validate pack structure (actions, sensors, triggers)           │
└────────────────┬────────────────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 4. Check Dependencies                                               │
│    - Verify Attune version compatibility                            │
│    - Check runtime dependencies (Python, Node.js, etc.)             │
│    - Verify dependent packs are installed                           │
│    - Check Python/Node.js version requirements                      │
└────────────────┬────────────────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 5. Setup Worker Environment                                         │
│    - Python: Create virtualenv, install requirements.txt            │
│    - Node.js: Create node_modules, run npm install                  │
│    - Shell: Verify scripts are executable                           │
└────────────────┬────────────────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 6. Run Pack Tests (if present)                                      │
│    - Execute test suite defined in pack                             │
│    - Verify all tests pass                                          │
│    - Skip if --skip-tests flag provided                             │
└────────────────┬────────────────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 7. Register Pack Components                                         │
│    - Insert pack metadata into database                             │
│    - Register actions, sensors, triggers                            │
│    - Register bundled rules and workflows (if any)                  │
│    - Copy pack files to permanent location                          │
└────────────────┬────────────────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 8. Cleanup                                                          │
│    - Remove temporary directory                                     │
│    - Log installation success                                       │
│    - Return pack ID and metadata                                    │
└─────────────────────────────────────────────────────────────────────┘
```

### Pack Storage Location

Installed packs are stored in the configured packs directory:

```
/var/lib/attune/packs/
├── slack/
│   ├── pack.yaml
│   ├── actions/
│   ├── sensors/
│   ├── triggers/
│   ├── requirements.txt
│   ├── .venv/              # Python virtualenv (if applicable)
│   └── metadata.json       # Installation metadata
├── aws/
└── github/
```

Installation metadata includes:

```json
{
  "pack_ref": "slack",
  "version": "2.1.0",
  "installed_at": "2024-01-20T12:00:00Z",
  "installed_from": {
    "type": "git",
    "url": "https://github.com/example/pack-slack.git",
    "ref": "v2.1.0"
  },
  "checksum": "sha256:abc123...",
  "registry": "Official Attune Registry"
}
```

---

## Checksum Verification

To ensure pack integrity, checksums are verified during installation:

### Supported Algorithms

- `sha256` (recommended)
- `sha512` (recommended)
- `sha1` (legacy, not recommended)
- `md5` (legacy, not recommended)

### Checksum Format

```
algorithm:hash
```

Examples:
- `sha256:abc123def456...`
- `sha512:789xyz...`

### Generating Checksums

For pack maintainers:

```bash
# Git repository (tar.gz snapshot)
sha256sum pack-slack-2.1.0.tar.gz

# Zip archive
sha256sum pack-slack-2.1.0.zip

# Using attune CLI
attune pack checksum ./pack-slack-2.1.0.zip
```

### Verification Process

1. Download/extract pack to temporary location
2. Calculate checksum of downloaded content
3. Compare with checksum in index file
4. If mismatch, abort installation and report error
5. If `verify_checksums: false` in config, skip verification (not recommended)

---

## CI/CD Integration

### GitHub Actions Example

Automate pack building and registry updates:

```yaml
name: Build and Publish Pack

on:
  push:
    tags:
      - 'v*'

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      
      - name: Create pack archive
        run: |
          VERSION=${GITHUB_REF#refs/tags/v}
          zip -r pack-slack-${VERSION}.zip . -x ".git/*" ".github/*"
      
      - name: Calculate checksum
        id: checksum
        run: |
          CHECKSUM=$(sha256sum pack-slack-*.zip | awk '{print $1}')
          echo "checksum=sha256:${CHECKSUM}" >> $GITHUB_OUTPUT
      
      - name: Upload to artifact storage
        run: |
          VERSION=${GITHUB_REF#refs/tags/v}
          aws s3 cp pack-slack-${VERSION}.zip s3://my-bucket/packs/
      
      - name: Generate registry entry
        run: |
          VERSION=${GITHUB_REF#refs/tags/v}
          attune pack index-entry \
            --pack-dir . \
            --version ${VERSION} \
            --git-url https://github.com/example/pack-slack.git \
            --git-ref ${GITHUB_REF#refs/tags/} \
            --archive-url https://my-bucket.s3.amazonaws.com/packs/pack-slack-${VERSION}.zip \
            --checksum ${{ steps.checksum.outputs.checksum }} \
            > entry.json
      
      - name: Update registry index
        run: |
          # Download current index
          wget https://registry.example.com/index.json
          
          # Add new entry
          attune pack index-update \
            --index index.json \
            --entry entry.json \
            --output index.json
          
          # Upload updated index
          aws s3 cp index.json s3://registry.example.com/
```

---

## Error Handling

### Installation Errors

| Error | Cause | Resolution |
|-------|-------|------------|
| Pack not found in registry | Pack ref doesn't exist in any configured registry | Check pack name, verify registry is online |
| Checksum mismatch | Downloaded pack doesn't match expected checksum | Pack may be corrupted or tampered with; contact pack maintainer |
| Pack already installed | Pack with same ref already exists | Use `--force` to reinstall |
| Dependency not met | Required Attune version, runtime, or pack not available | Update Attune, install runtime, or install dependency pack |
| Invalid pack structure | pack.yaml missing or invalid | Fix pack structure |
| Tests failed | Pack tests did not pass | Fix pack code or use `--skip-tests` (not recommended) |

### Registry Errors

| Error | Cause | Resolution |
|-------|-------|------------|
| Registry unreachable | Network error, DNS failure | Check network, verify URL |
| Invalid index format | Index JSON is malformed | Contact registry maintainer |
| Authentication failed | Registry requires authentication but token is invalid | Update registry token in configuration |

---

## Security Considerations

### 1. HTTPS And Explicit Host Approval

Configure `allow_http: false` to reject non-HTTPS registries:

```yaml
pack_registry:
  allow_http: false  # Only allow HTTPS
  approved_public_hosts:
    - registry.example.com
```

Index URLs and registry-provided archive/Git URLs do not grant trust to their
own hosts. Public destinations must have their hostname in
`approved_public_hosts`. Private/internal destinations may be approved by
hostname in `approved_private_hosts` or independently by ensuring every
resolved address is within `approved_private_cidrs`. DNS answers containing
both public and private/special addresses are rejected.

HTTP clients disable environment proxies and automatic redirects, pin the DNS
answers checked by policy, and enforce connect/total timeouts and response-size
limits. API-managed indices must use HTTPS and cannot use `file://` URLs.

Git sources support approved HTTPS URLs only. Git runs with system/global Git
configuration disabled, redirects and proxies disabled, TLS hostname
verification enabled, and libcurl resolution pinned to one address from the
validated DNS result. SSH, `git://`, credential-bearing, and `file://` Git URLs
are rejected.

ZIP, TAR, and compressed TAR pack archives are extracted in-process with fixed
entry-count, per-entry, and total extracted-byte limits. Extraction rejects
absolute or parent paths, symbolic and hard links, devices/FIFOs, and any path
whose existing parent is a symlink. Pack directories are also rejected if they
contain links during checksum or installation. Activation keeps the previous
pack directory available for rollback until database registration succeeds.

### 2. Checksum Verification

Always enable checksum verification in production:

```yaml
pack_registry:
  verify_checksums: true
```

### 3. Registry Authentication

API-managed registry header values are encrypted at rest with
`security.encryption_key` and are returned as `[REDACTED]`. Sending that marker
back for an existing header preserves its value; it is never stored literally.
Static registry headers should obtain tokens from deployment secret injection:

```bash
export REGISTRY_TOKEN=$(cat /run/secrets/registry_token)
export ATTUNE__PACK_REGISTRY__INDICES="https://registry.example.com/index.json"
```

### 4. Code Review

- Review pack code before installation
- Use `--skip-tests` cautiously
- Test packs in non-production environment first

### 5. Signature Verification (Future)

Future enhancement: GPG signature verification for pack archives:

```json
{
  "type": "archive",
  "url": "https://example.com/packs/slack-2.1.0.zip",
  "checksum": "sha256:abc123...",
  "signature": "https://example.com/packs/slack-2.1.0.zip.sig",
  "signing_key": "0x1234567890ABCDEF"
}
```

---

## Future Enhancements

### Version 1.1

- **Semantic version matching**: `slack@^2.0.0`, `slack@~2.1.0`
- **Pack updates**: `attune pack update <ref>` to upgrade to latest version
- **Dependency resolution**: Automatic installation of pack dependencies

### Version 1.2

- **GPG signature verification**: Cryptographic verification of pack authenticity
- **Pack ratings and reviews**: Community feedback in registry
- **Usage statistics**: Download counts, popularity metrics

### Version 1.3

- **Private pack authentication**: Token-based authentication for private packs
- **Pack mirroring**: Automatic mirroring of registry indices for redundancy
- **Delta updates**: Only download changed files when updating packs

---

## Related Documentation

- [Pack Structure](./pack-structure.md)
- [Pack Management Architecture](./pack-management-architecture.md)
- [CLI Documentation](./cli.md)
- [Configuration Guide](./configuration.md)
- [Pack Testing Framework](./pack-testing-framework.md)
