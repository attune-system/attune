# CLI Pack Installation Quick Reference

This document provides quick reference commands for installing, managing, and working with packs using the Attune CLI.

## Table of Contents

- [Installation Commands](#installation-commands)
- [Using Actions Directly](#using-actions-directly)
- [Using the Workflow](#using-the-workflow)
- [Management Commands](#management-commands)
- [Examples](#examples)

## Installation Commands

### Install Pack from Source

Install a pack from git, HTTP, or registry:

```bash
# From git repository (HTTPS)
attune pack install https://github.com/attune/pack-slack.git

# From git repository with specific ref
attune pack install https://github.com/attune/pack-slack.git --ref-spec v1.0.0

# From HTTP archive
attune pack install https://example.com/packs/slack-1.0.0.tar.gz

# From registry (if configured)
attune pack install slack@1.0.0

# With options
attune pack install slack@1.0.0 \
  --force \
  --skip-tests \
  --skip-deps
```

Remote Git supports explicitly approved HTTPS hosts only. SSH, `git://`,
`file://`, and credential-bearing URLs are rejected.
ZIP, TAR, and TGZ archives are extracted with bounded in-process extraction;
links, special files, traversal paths, and archive bombs are rejected. Failed
registration restores the previously active pack directory.

**Options:**
- `--ref-spec <REF>` - Git branch, tag, or commit
- `--force` - Force reinstall if pack exists
- `--skip-tests` - Skip running pack tests
- `--skip-deps` - Skip dependency validation
- `--no-registry` - Don't use registry for resolution

### Register Pack from Local Path

Register a pack that's already on disk:

```bash
# Register pack from directory
attune pack register /path/to/pack

# With options
attune pack register /path/to/pack \
  --force \
  --skip-tests
```

**Options:**
- `--force` - Replace existing pack
- `--skip-tests` - Skip running pack tests

## Using Actions Directly

The pack installation workflow consists of individual actions that can be run separately:

### 1. Download Packs

```bash
# Download one or more packs
attune action execute core.download_packs \
  --param packs='["https://github.com/attune/pack-slack.git"]' \
  --param destination_dir=/tmp/attune-packs \
  --wait

# Multiple packs
attune action execute core.download_packs \
  --param packs='["slack@1.0.0","aws@2.0.0"]' \
  --param destination_dir=/tmp/attune-packs \
  --param registry_url=https://registry.attune.io/index.json \
  --wait

# Get JSON output
attune action execute core.download_packs \
  --param packs='["https://github.com/attune/pack-slack.git"]' \
  --param destination_dir=/tmp/attune-packs \
  --wait --json
```

### 2. Get Pack Dependencies

```bash
# Analyze pack dependencies
attune action execute core.get_pack_dependencies \
  --param pack_paths='["/tmp/attune-packs/slack"]' \
  --wait

# With JSON output to check for missing dependencies
result=$(attune action execute core.get_pack_dependencies \
  --param pack_paths='["/tmp/attune-packs/slack"]' \
  --wait --json)

echo "$result" | jq '.result.missing_dependencies'
```

### 3. Build Pack Environments

```bash
# Build Python and Node.js environments
attune action execute core.build_pack_envs \
  --param pack_paths='["/tmp/attune-packs/slack"]' \
  --wait

# Skip Node.js environment
attune action execute core.build_pack_envs \
  --param pack_paths='["/tmp/attune-packs/slack"]' \
  --param skip_nodejs=true \
  --wait

# Force rebuild
attune action execute core.build_pack_envs \
  --param pack_paths='["/tmp/attune-packs/slack"]' \
  --param force_rebuild=true \
  --wait
```

### 4. Register Packs

```bash
# Register downloaded packs
attune action execute core.register_packs \
  --param pack_paths='["/tmp/attune-packs/slack"]' \
  --wait

# With force and skip tests
attune action execute core.register_packs \
  --param pack_paths='["/tmp/attune-packs/slack"]' \
  --param force=true \
  --param skip_tests=true \
  --wait
```

## Using the Workflow

The `core.install_packs` workflow automates the entire process:

```bash
# Install pack using workflow
attune action execute core.install_packs \
  --param packs='["https://github.com/attune/pack-slack.git"]' \
  --wait

# With options
attune action execute core.install_packs \
  --param packs='["slack@1.0.0","aws@2.0.0"]' \
  --param force=true \
  --param skip_tests=true \
  --wait

# Install with specific git ref
attune action execute core.install_packs \
  --param packs='["https://github.com/attune/pack-slack.git"]' \
  --param ref_spec=v1.0.0 \
  --wait
```

**Note**: When the workflow feature is fully implemented, use:
```bash
attune workflow execute core.install_packs \
  --input packs='["slack@1.0.0"]'
```

## Management Commands

### List Packs

```bash
# List all installed packs
attune pack list

# Filter by name
attune pack list --name slack

# JSON output
attune pack list --json
```

### Show Pack Details

```bash
# Show pack information
attune pack show slack

# JSON output
attune pack show slack --json
```

### Update Pack Metadata

```bash
# Update pack fields
attune pack update slack \
  --label "Slack Integration" \
  --description "Enhanced Slack pack" \
  --version 1.1.0
```

### Uninstall Pack

```bash
# Uninstall pack (with confirmation)
attune pack uninstall slack

# Force uninstall without confirmation
attune pack uninstall slack --yes
```

### Test Pack

```bash
# Run pack tests
attune pack test slack

# Verbose output
attune pack test slack --verbose

# Detailed output
attune pack test slack --detailed
```

## Examples

### Example 1: Install Pack from Git

```bash
# Full installation process
attune pack install https://github.com/attune/pack-slack.git --ref-spec v1.0.0 --wait

# Verify installation
attune pack show slack

# List actions in pack
attune action list --pack slack
```

### Example 2: Install Multiple Packs

```bash
# Install multiple packs from registry
attune action execute core.install_packs \
  --param packs='["slack@1.0.0","aws@2.1.0","kubernetes@3.0.0"]' \
  --wait
```

### Example 3: Development Workflow

```bash
# Download pack for development
attune action execute core.download_packs \
  --param packs='["https://github.com/myorg/pack-custom.git"]' \
  --param destination_dir=/home/user/packs \
  --param ref_spec=main \
  --wait

# Make changes to pack...

# Register updated pack
attune pack register /home/user/packs/custom --force
```

### Example 4: Check Dependencies Before Install

```bash
# Download pack
attune action execute core.download_packs \
  --param packs='["slack@1.0.0"]' \
  --param destination_dir=/tmp/test-pack \
  --wait

# Check dependencies
deps=$(attune action execute core.get_pack_dependencies \
  --param pack_paths='["/tmp/test-pack/slack"]' \
  --wait --json)

# Check for missing dependencies
missing=$(echo "$deps" | jq -r '.result.missing_dependencies | length')

if [[ "$missing" -gt 0 ]]; then
  echo "Missing dependencies found:"
  echo "$deps" | jq '.result.missing_dependencies'
  exit 1
fi

# Proceed with installation
attune pack register /tmp/test-pack/slack
```

### Example 5: Scripted Installation with Error Handling

```bash
#!/bin/bash
set -e

PACK_SOURCE="https://github.com/attune/pack-slack.git"
PACK_REF="v1.0.0"
TEMP_DIR="/tmp/attune-install-$$"

echo "Installing pack from: $PACK_SOURCE"

# Download
echo "Step 1: Downloading..."
download_result=$(attune action execute core.download_packs \
  --param packs="[\"$PACK_SOURCE\"]" \
  --param destination_dir="$TEMP_DIR" \
  --param ref_spec="$PACK_REF" \
  --wait --json)

success=$(echo "$download_result" | jq -r '.result.success_count // 0')
if [[ "$success" -eq 0 ]]; then
  echo "Error: Download failed"
  echo "$download_result" | jq '.result.failed_packs'
  exit 1
fi

# Get pack path
pack_path=$(echo "$download_result" | jq -r '.result.downloaded_packs[0].pack_path')
echo "Downloaded to: $pack_path"

# Check dependencies
echo "Step 2: Checking dependencies..."
deps_result=$(attune action execute core.get_pack_dependencies \
  --param pack_paths="[\"$pack_path\"]" \
  --wait --json)

missing=$(echo "$deps_result" | jq -r '.result.missing_dependencies | length')
if [[ "$missing" -gt 0 ]]; then
  echo "Warning: Missing dependencies:"
  echo "$deps_result" | jq '.result.missing_dependencies'
fi

# Build environments
echo "Step 3: Building environments..."
attune action execute core.build_pack_envs \
  --param pack_paths="[\"$pack_path\"]" \
  --wait

# Register
echo "Step 4: Registering pack..."
attune pack register "$pack_path"

# Cleanup
rm -rf "$TEMP_DIR"

echo "Installation complete!"
```

### Example 6: Bulk Pack Installation

```bash
#!/bin/bash
# Install multiple packs from a list

PACKS=(
  "slack@1.0.0"
  "aws@2.1.0"
  "kubernetes@3.0.0"
  "datadog@1.5.0"
)

for pack in "${PACKS[@]}"; do
  echo "Installing: $pack"
  if attune pack install "$pack" --skip-tests; then
    echo "✓ $pack installed successfully"
  else
    echo "✗ $pack installation failed"
  fi
done
```

## Output Formats

All commands support multiple output formats:

```bash
# Default table format
attune pack list

# JSON format
attune pack list --json
attune pack list -j

# YAML format
attune pack list --yaml
attune pack list -y
```

## Authentication

Most commands require authentication:

```bash
# Login first
attune auth login

# Or use a token
export ATTUNE_API_TOKEN="your-token-here"
attune pack list

# Or specify token in command
attune pack list --api-url http://localhost:8080
```

## Configuration

Configure CLI settings:

```bash
# Set default API URL
attune config set api_url http://localhost:8080

# Set default profile
attune config set profile production

# View configuration
attune config show
```

## Troubleshooting

### Common Issues

**Authentication errors:**
```bash
# Re-login
attune auth login

# Check token
attune auth token

# Refresh token
attune auth refresh
```

**Pack already exists:**
```bash
# Use --force to replace
attune pack install slack@1.0.0 --force
```

**Network timeouts:**
```bash
# Increase timeout (via environment variable for now)
export ATTUNE_ACTION_TIMEOUT=600
attune pack install large-pack@1.0.0
```

**Missing dependencies:**
```bash
# Install dependencies first
attune pack install core@1.0.0
attune pack install dependent-pack@1.0.0
```

## See Also

- [Pack Installation Actions Documentation](pack-installation-actions.md)
- [Pack Structure](pack-structure.md)
- [Pack Registry](pack-registry-spec.md)
- [CLI Configuration](../crates/cli/README.md)
