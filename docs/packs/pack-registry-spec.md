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
          "ref": "0123456789abcdef0123456789abcdef01234567",
          "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        },
        {
          "type": "archive",
          "url": "https://github.com/attune-io/pack-slack/archive/refs/tags/v2.1.0.zip",
          "checksum": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
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
| `keywords` | array[string] | Yes | Searchable keywords/tags |
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
| `ref` | string | Yes for Git | Explicit Git ref; production catalogs require an immutable lowercase 40-character commit SHA |
| `checksum` | string | Yes | SHA-256 in `sha256:<64 lowercase hex characters>` format |

### Configured Index Ordering

Fresh and upgraded databases receive an immutable **Attune Standard Pack
Index** snapshot as a one-time API-managed row:

```text
https://raw.githubusercontent.com/attune-system/index/793aabcc0eb537af7681a386b591de6c4fafd7a1/index.json
```

On a fresh database it starts at position `0`. On upgrade it is appended after
existing managed indices so established resolution order is not changed. The
row is otherwise ordinary managed configuration: administrators can reorder,
disable, or permanently delete it. Database migrations do not recreate a
deleted row. Administrators who explicitly want catalog changes independent of
Attune releases can add the live `main` index as a separate managed index.

Attune stores configured index URLs in the `pack_registry_index` table. Indices
have an integer `position` internally; lower positions are searched first. New
indices append to the end by default, and administrators reorder them through
the web client's drag handle or by updating positions through the API/CLI. When
the same pack `ref` appears in multiple enabled indices, the first enabled index
containing that ref wins for browse/detail/install resolution.

Install resolution is fail-closed: if any higher-priority index cannot be
fetched or validated, Attune returns that error instead of continuing to a
lower-priority index that might contain a different pack with the same ref.
Use `registry_id`/`--registry-id` to select one enabled managed index when the
origin must be explicit.

Static `pack_registry.indices` YAML entries remain bootstrap configuration. The
seeded standard snapshot alone does not suppress them, but any non-standard
managed row restores managed-only resolution as before. A managed row shadows
a canonical-equivalent static entry even while disabled, so disabling a
managed index cannot reactivate a duplicate static entry.

The API blocks deletion of the last non-standard managed index while any
static index is enabled. Disable or remove those static entries first so a
managed-index deletion cannot unexpectedly reactivate bootstrap configuration.

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
selecting the preferred install source. Optional `registry_id` pins resolution
to one enabled managed index; `no_registry: true` requires an explicit URL or
path already visible to the API server and performs no registry lookup. The CLI
does not upload workstation-local files. The two options are
mutually exclusive.

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
attune pack install https://github.com/example/pack-slack.git --ref-spec v2.1.0
attune pack install https://github.com/example/pack-slack.git --ref-spec main
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

Install from a filesystem path already visible to the API server:

```bash
attune pack install /path/to/pack-slack
attune pack install ./packs/my-pack
```

**Requirements**:
- Directory must contain valid pack structure
- `pack.yaml` must be present
- Used for development and testing
- The CLI sends the path string; it does not upload workstation files

### 4. Local Archive

Install from an archive path already visible to the API server:

```bash
attune pack install /path/to/pack-slack-2.1.0.zip
attune pack install ./my-pack.tar.gz
```

**Requirements**:
- Archive must contain valid pack structure
- The CLI sends the path string; use `pack upload` for workstation-local content
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
  approved_public_hosts:
    - raw.githubusercontent.com
    - registry.attune.io
    - github.com
    - codeload.github.com
    - objects.githubusercontent.com
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
        Authorization: "Bearer replace-with-literal-token"
    
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
  allow_unverified_direct_remote_installs: false
  allow_http: false  # Only allow HTTPS
```

The `raw.githubusercontent.com`, `github.com`, and `codeload.github.com` hosts
are the application defaults required by the pinned standard snapshot and its
install sources. `codeload.github.com` serves the independently checksummed
archive fallback. Set `approved_public_hosts: []` explicitly to opt out of public
registry and pack-source traffic. The pinned and live standard-index URLs are
distinct managed indices; canonical URL variants of either are deduplicated.
Managed entries are searched first and shadow canonical-equivalent static
entries.

Setting `pack_registry.enabled: false` disables index resolution and remote
Git/archive pack traffic entirely; local-directory installation remains
available.

Direct remote Git/archive requests do not carry registry-supplied integrity
metadata and are rejected by default even when their hosts are approved. Prefer
registry references. Operators who explicitly accept this risk can set
`allow_unverified_direct_remote_installs: true`; host and HTTPS policy still
apply.

**Environment Variables**:

```bash
# Enable/disable registry
export ATTUNE__PACK_REGISTRY__ENABLED=true

# Static index objects are configured in YAML. API-managed indices can be
# added with `attune pack index add` or the management API.

# Cache settings
export ATTUNE__PACK_REGISTRY__CACHE_TTL=3600
export ATTUNE__PACK_REGISTRY__VERIFY_CHECKSUMS=true
export ATTUNE__PACK_REGISTRY__ALLOW_UNVERIFIED_DIRECT_REMOTE_INSTALLS=false
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

Index and pack-source URLs must not contain query parameters. Put index
credentials in validated headers so they can be encrypted and redacted rather
than logged or persisted as part of a URL. Authenticated pack-source downloads
do not currently support custom headers and must use another supported delivery
model. Migration 26 strips query strings from managed-index and pack-provenance
URLs, disables affected indices for review, and limits audit redaction to the
`source` field of `pack.installed` events. Operators upgrading from versions
that accepted query credentials must still review stored URLs and rotate those
credentials because external logs and backups cannot be rewritten
automatically.

```yaml
pack_registry:
  indices:
    - url: https://private-registry.example.com/index.json
      headers:
        Authorization: "Bearer replace-with-literal-token"
        X-Custom-Header: "value"
```

Attune YAML values are literal: `${TOKEN}` placeholders are not interpolated.
Prefer API-managed headers supplied from a secret store; they are encrypted at
rest and redacted on read.

---

## CLI Commands

### Install Pack

```bash
# From registry (by reference), optionally pinned to a managed index ID
attune pack install <pack-ref>[@version]
attune pack install <pack-ref>[@version] --registry-id <id>

# From git repository
attune pack install <git-url> [--ref-spec <branch|tag|commit>]

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
--registry-id <id>      # Resolve only through this enabled managed index
--no-registry           # Require an explicit URL or existing local path
```

`--registry-id` and `--no-registry` cannot be combined. `--no-registry` does
not reinterpret an unresolved name as a registry ref.

Installation always requires pack `install` permission. If `--force` would
replace an existing pack, the caller must also have `configure` permission for
that pack. Replacement preserves the existing pack owner rather than
transferring ownership to the installer.

### Examples

```bash
# Install latest version from registry
attune pack install slack

# Install specific version from registry
attune pack install slack@2.1.0

# Install from git repository (latest tag)
attune pack install https://github.com/example/pack-slack.git

# Install from git repository (specific tag)
attune pack install https://github.com/example/pack-slack.git --ref-spec v2.1.0

# Install from git repository (branch)
attune pack install https://github.com/example/pack-slack.git --ref-spec main

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
  <path-to-pack> \
  --git-url <git-repo-url> \
  --git-ref <40-character-lowercase-commit-sha> \
  --archive-url <archive-url> \
  --archive-checksum sha256:<64-lowercase-hex>

# Output to stdout (JSON)
attune pack index-entry ./pack-slack \
  --git-url https://github.com/example/pack-slack.git \
  --git-ref 0123456789abcdef0123456789abcdef01234567 \
  --archive-url https://example.com/packs/slack-2.1.0.zip \
  --archive-checksum sha256:1111111111111111111111111111111111111111111111111111111111111111
```

The Git source receives Attune's framed checksum of the local pack directory.
When `--archive-url` is present, `--archive-checksum` is required and must be
the SHA-256 of the exact archive bytes; the CLI does not download the URL or
reuse the directory checksum for it. Generated Git and archive URLs must use
HTTPS and cannot contain credentials, query parameters, or fragments.
At least one of `--git-url` or `--archive-url` is required; the CLI never emits
a template or placeholder install source. `--git-url` also requires an explicit
`--git-ref`; the CLI does not fabricate a ref. Production indices must use an
immutable lowercase 40-character commit SHA. The CLI continues to accept a
branch or tag only for development-only local indices; do not publish those
mutable refs.

Manifest metadata uses the same normalization as the maintained index builder:
`label` precedes `name`; canonical `tags` precedes top-level `keywords`, which
precedes `meta.keywords`; top-level `license`, `homepage`, and `use_case`
precede `meta.license`, `meta.documentation_url`, and `meta.use_case`.
List-form dependencies become `{"packs": [...]}`, object-form dependencies
map to `PackDependencies`, and JSON-compatible manifest `meta` is preserved.
The local CLI does not invent GitHub-only branch, commit, or star metadata.
Canonical scalar metadata must be strings. Discovery, runtime, and dependency
lists accept strings and finite numbers; nulls, booleans, objects, and
non-finite values are rejected rather than silently dropped or coerced.

`index-entry` emits JSON without progress text by default, so redirecting it to
a file is safe; `--format json` explicitly selects the same behavior. Component
summaries come from top-level `.yaml`/`.yml` files in `actions/`, `sensors/`,
`triggers/`, `rules/`, and `workflows/`. An action file declaring
`workflow_file` is listed under workflows. Summary names use `ref`, then
`name`, then the filename, remove the `<pack-ref>.` prefix, and are sorted.
Descriptions use `description`, then `label`.

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
      "ref": "0123456789abcdef0123456789abcdef01234567",
      "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
    },
    {
      "type": "archive",
      "url": "https://example.com/packs/slack-2.1.0.zip",
      "checksum": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
    }
  ],
  "contents": {
    "actions": [
      {"name": "send_message", "description": "Send a message to a Slack channel"}
    ],
    "sensors": [
      {"name": "message_sensor", "description": "Monitor Slack messages"}
    ],
    "triggers": [
      {"name": "message_received", "description": "Fires when a message is received"}
    ],
    "rules": [],
    "workflows": []
  }
}
```

### Update Index File

Merge multiple index entries or update an existing index:

```bash
# Add a generated entry to an index
attune pack index-update --index registry.json ./pack-slack \
  --git-url https://github.com/example/pack-slack.git \
  --git-ref 0123456789abcdef0123456789abcdef01234567

# Merge multiple indices
attune pack index-merge --file combined.json registry1.json registry2.json

# Update pack version in index
attune pack index-update --index registry.json ./pack-slack --update \
  --git-url https://github.com/example/pack-slack.git \
  --git-ref 89abcdef0123456789abcdef0123456789abcdef \
  --archive-url https://example.com/packs/slack-2.1.1.zip \
  --archive-checksum sha256:2222222222222222222222222222222222222222222222222222222222222222
```

`index-update` requires a real source just like `index-entry`. Before replacing
the file, it sorts entries by `ref`, updates `last_updated` to the current UTC
time only when pack content changes, validates the complete resulting index,
and atomically replaces the original file.

`index-merge` fully validates every input and the result, selects duplicate
packs by semantic-version precedence, sorts packs by `ref`, and emits a
canonical index using the first input's registry identity and the latest input
timestamp. It atomically replaces the output only after all processing
succeeds, so missing or invalid inputs leave an existing output unchanged.

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
│    - Fail on a higher-priority index fetch/validation error         │
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
│    - Bind pack.yaml ref/version to the resolved registry entry      │
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
    "ref": "0123456789abcdef0123456789abcdef01234567"
  },
  "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
  "registry": "Official Attune Registry"
}
```

For a registry archive install, provenance records the checksum of the archive
that was actually downloaded and verified. If a preferred Git source fails,
Attune tries the first archive source in the same entry using that archive's
independent checksum. Registry installs reject content whose `pack.yaml` ref or
version differs from the selected index entry.

---

## Checksum Verification

To ensure pack integrity, checksums are verified during installation:

### Supported Algorithm

Registry source checksums support SHA-256 only.

### Checksum Format

```text
sha256:<64 lowercase hexadecimal characters>
```

### Generating Checksums

For pack maintainers:

```bash
# Git source: use the maintained index builder or the CLI index commands. They
# compute Attune's framed, sorted path-and-content directory checksum while
# excluding .git metadata.
python scripts/build_index.py --repository OWNER/REPOSITORY

# Archive source: hash the exact downloaded bytes.
sha256sum pack-slack-2.1.0.zip
```

### Verification Process

1. Download/clone the source to a temporary location
2. Calculate the source-specific SHA-256 (archive bytes or framed Git directory)
3. Compare it with the checksum in the selected index entry
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
          zip -r "${RUNNER_TEMP}/pack-slack-${VERSION}.zip" . -x ".git/*" ".github/*"
      
      - name: Calculate checksum
        id: checksum
        run: |
          VERSION=${GITHUB_REF#refs/tags/v}
          CHECKSUM=$(sha256sum "${RUNNER_TEMP}/pack-slack-${VERSION}.zip" | awk '{print $1}')
          echo "checksum=sha256:${CHECKSUM}" >> $GITHUB_OUTPUT
      
      - name: Upload to artifact storage
        run: |
          VERSION=${GITHUB_REF#refs/tags/v}
          aws s3 cp "${RUNNER_TEMP}/pack-slack-${VERSION}.zip" s3://my-bucket/packs/
      
      - name: Generate registry entry
        run: |
          VERSION=${GITHUB_REF#refs/tags/v}
          attune pack index-entry \
            . \
            --git-url https://github.com/example/pack-slack.git \
            --git-ref "${{ github.sha }}" \
            --archive-url https://my-bucket.s3.amazonaws.com/packs/pack-slack-${VERSION}.zip \
            --archive-checksum ${{ steps.checksum.outputs.checksum }} \
            > "${RUNNER_TEMP}/entry.json"
      
      - name: Update registry index
        run: |
          # Download current index
          wget -O "${RUNNER_TEMP}/index.json" https://registry.example.com/index.json
          VERSION=${GITHUB_REF#refs/tags/v}
          
          # Add new entry
          attune pack index-update \
            --index "${RUNNER_TEMP}/index.json" \
            . \
            --git-url https://github.com/example/pack-slack.git \
            --git-ref "${{ github.sha }}" \
            --archive-url https://my-bucket.s3.amazonaws.com/packs/pack-slack-${VERSION}.zip \
            --archive-checksum ${{ steps.checksum.outputs.checksum }}
          
          # Upload updated index
          aws s3 cp "${RUNNER_TEMP}/index.json" s3://registry.example.com/
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

Configure static authenticated indices as structured YAML objects, or submit
their headers through the management API using credentials read from the
deployment's secret store. Do not encode structured `indices` as a
comma-separated environment variable.

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
  "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
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
