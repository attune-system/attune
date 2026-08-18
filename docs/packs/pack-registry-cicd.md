# Pack Registry CI/CD Integration

This document provides examples and best practices for integrating pack publishing with CI/CD pipelines.

## Overview

Automating pack registry publishing ensures:
- Consistent pack versioning and releases
- Automated checksum generation
- Registry index updates on every release
- Quality assurance through automated testing

## Prerequisites

1. **Pack Structure**: Your pack must have a valid `pack.yaml`
2. **Registry Index**: A git repository hosting your `index.json`
3. **Pack Storage**: A place to host pack archives (GitHub Releases, S3, etc.)
4. **Attune CLI**: Available in CI environment

## GitHub Actions Examples

### Example 1: Publish Pack on Git Tag

```yaml
# .github/workflows/publish-pack.yml
name: Publish Pack

on:
  push:
    tags:
      - 'v*.*.*'

jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - name: Checkout pack repository
        uses: actions/checkout@v3

      - name: Install Attune CLI
        run: |
          curl -L https://github.com/attune/attune/releases/latest/download/attune-cli-linux -o /usr/local/bin/attune
          chmod +x /usr/local/bin/attune

      - name: Validate pack.yaml
        run: |
          if [ ! -f "pack.yaml" ]; then
            echo "Error: pack.yaml not found"
            exit 1
          fi

      - name: Run pack tests
        run: attune pack test . --detailed

      - name: Resolve pack version
        id: version
        run: echo "version=${GITHUB_REF#refs/tags/v}" >> "$GITHUB_OUTPUT"

      - name: Create GitHub Release
        id: create_release
        uses: actions/create-release@v1
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        with:
          tag_name: ${{ github.ref }}
          release_name: Release ${{ github.ref }}
          draft: false
          prerelease: false

      - name: Create pack archive
        run: |
          tar -czf "${RUNNER_TEMP}/pack-${{ steps.version.outputs.version }}.tar.gz" \
            --exclude='.git' .

      - name: Hash exact archive bytes
        id: archive_checksum
        run: |
          HASH=$(sha256sum "${RUNNER_TEMP}/pack-${{ steps.version.outputs.version }}.tar.gz" | cut -d ' ' -f1)
          echo "checksum=sha256:${HASH}" >> $GITHUB_OUTPUT

      - name: Upload pack archive
        uses: actions/upload-release-asset@v1
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        with:
          upload_url: ${{ steps.create_release.outputs.upload_url }}
          asset_path: ${{ runner.temp }}/pack-${{ steps.version.outputs.version }}.tar.gz
          asset_name: pack-${{ steps.version.outputs.version }}.tar.gz
          asset_content_type: application/gzip

      - name: Generate index entry
        run: |
          attune pack index-entry . \
            --git-url "https://github.com/${{ github.repository }}" \
            --git-ref "${{ github.sha }}" \
            --archive-url "https://github.com/${{ github.repository }}/releases/download/${{ github.ref_name }}/pack-${{ steps.version.outputs.version }}.tar.gz" \
            --archive-checksum "${{ steps.archive_checksum.outputs.checksum }}" \
            --format json > "${RUNNER_TEMP}/index-entry.json"

      - name: Checkout registry repository
        uses: actions/checkout@v3
        with:
          repository: your-org/attune-registry
          token: ${{ secrets.REGISTRY_TOKEN }}
          path: registry

      - name: Move registry outside the pack directory
        run: mv registry "${RUNNER_TEMP}/registry"

      - name: Update registry index
        run: |
          attune pack index-update \
            --index "${RUNNER_TEMP}/registry/index.json" \
            . \
            --git-url "https://github.com/${{ github.repository }}" \
            --git-ref "${{ github.sha }}" \
            --archive-url "https://github.com/${{ github.repository }}/releases/download/${{ github.ref_name }}/pack-${{ steps.version.outputs.version }}.tar.gz" \
            --archive-checksum "${{ steps.archive_checksum.outputs.checksum }}" \
            --update

      - name: Commit and push registry changes
        run: |
          cd "${RUNNER_TEMP}/registry"
          git config user.name "GitHub Actions"
          git config user.email "actions@github.com"
          git add index.json
          git commit -m "Add/update pack: $(yq -r '.ref' ../pack.yaml) ${{ github.ref_name }}"
          git push
```

### Example 2: Multi-Pack Repository

```yaml
# .github/workflows/publish-packs.yml
name: Publish All Packs

on:
  push:
    branches:
      - main
    paths:
      - 'packs/**'

jobs:
  detect-changed-packs:
    runs-on: ubuntu-latest
    outputs:
      packs: ${{ steps.changed.outputs.packs }}
    steps:
      - uses: actions/checkout@v3
        with:
          fetch-depth: 2

      - name: Detect changed packs
        id: changed
        run: |
          CHANGED_PACKS=$(git diff --name-only HEAD^ HEAD | grep '^packs/' | cut -d'/' -f2 | sort -u | jq -R -s -c 'split("\n")[:-1]')
          echo "packs=$CHANGED_PACKS" >> $GITHUB_OUTPUT

  publish-pack:
    needs: detect-changed-packs
    runs-on: ubuntu-latest
    strategy:
      matrix:
        pack: ${{ fromJson(needs.detect-changed-packs.outputs.packs) }}
    steps:
      - uses: actions/checkout@v3

      - name: Install Attune CLI
        run: |
          curl -L https://github.com/attune/attune/releases/latest/download/attune-cli-linux -o /usr/local/bin/attune
          chmod +x /usr/local/bin/attune

      - name: Test pack
        run: attune pack test packs/${{ matrix.pack }}

      - name: Update registry
        run: |
          # Clone registry
          git clone https://${{ secrets.REGISTRY_TOKEN }}@github.com/your-org/attune-registry.git registry
          
          # Update index
          attune pack index-update \
            --index registry/index.json \
            packs/${{ matrix.pack }} \
            --git-url "https://github.com/${{ github.repository }}" \
            --git-ref "${{ github.sha }}" \
            --update
          
          # Commit changes
          cd registry
          git config user.name "GitHub Actions"
          git config user.email "actions@github.com"
          git add index.json
          git commit -m "Update pack: ${{ matrix.pack }}" || exit 0
          git push
```

### Example 3: Registry Maintenance

```yaml
# .github/workflows/maintain-registry.yml
name: Maintain Registry

on:
  schedule:
    # Run weekly on Sundays at midnight
    - cron: '0 0 * * 0'
  workflow_dispatch:

jobs:
  merge-registries:
    runs-on: ubuntu-latest
    steps:
      - name: Install Attune CLI
        run: |
          curl -L https://github.com/attune/attune/releases/latest/download/attune-cli-linux -o /usr/local/bin/attune
          chmod +x /usr/local/bin/attune

      - name: Download registries
        run: |
          mkdir registries
          curl -o registries/main.json https://registry.attune.io/index.json
          curl -o registries/community.json https://community.attune.io/index.json

      - name: Merge registries
        run: |
          attune pack index-merge \
            --file merged-index.json \
            registries/*.json

      - name: Upload merged index
        uses: actions/upload-artifact@v3
        with:
          name: merged-registry
          path: merged-index.json
```

## GitLab CI Examples

### Example 1: Publish on Tag

```yaml
# .gitlab-ci.yml
stages:
  - test
  - publish

variables:
  PACK_VERSION: ${CI_COMMIT_TAG#v}

test:pack:
  stage: test
  image: attune/cli:latest
  script:
    - attune pack test .
  only:
    - tags

publish:pack:
  stage: publish
  image: attune/cli:latest
  script:
    # Create archive
    - ARCHIVE_PATH="/tmp/pack-${PACK_VERSION}.tar.gz"
    - tar -czf "${ARCHIVE_PATH}" --exclude='.git' .

    # Hash the exact archive bytes
    - ARCHIVE_CHECKSUM="sha256:$(sha256sum "${ARCHIVE_PATH}" | cut -d ' ' -f1)"
    
    # Upload to package registry
    - 'curl --header "JOB-TOKEN: $CI_JOB_TOKEN" --upload-file "${ARCHIVE_PATH}" "${CI_API_V4_URL}/projects/${CI_PROJECT_ID}/packages/generic/pack/${PACK_VERSION}/pack-${PACK_VERSION}.tar.gz"'
    
    # Clone registry
    - REGISTRY_DIR="${CI_BUILDS_DIR}/attune-registry-${CI_JOB_ID}"
    - git clone https://oauth2:${REGISTRY_TOKEN}@gitlab.com/your-org/attune-registry.git "${REGISTRY_DIR}"
    
    # Update index
    - |
      attune pack index-update \
        --index "${REGISTRY_DIR}/index.json" \
        . \
        --git-url "${CI_PROJECT_URL}" \
        --git-ref "${CI_COMMIT_SHA}" \
        --archive-url "${CI_API_V4_URL}/projects/${CI_PROJECT_ID}/packages/generic/pack/${PACK_VERSION}/pack-${PACK_VERSION}.tar.gz" \
        --archive-checksum "${ARCHIVE_CHECKSUM}" \
        --update
    
    # Commit and push
    - cd "${REGISTRY_DIR}"
    - git config user.name "GitLab CI"
    - git config user.email "ci@gitlab.com"
    - git add index.json
    - git commit -m "Update pack from ${CI_PROJECT_NAME} ${CI_COMMIT_TAG}"
    - git push
  only:
    - tags
```

## Jenkins Pipeline Example

```groovy
// Jenkinsfile
pipeline {
    agent any
    
    environment {
        PACK_VERSION = sh(script: "yq -r '.version' pack.yaml", returnStdout: true).trim()
        PACK_REF = sh(script: "yq -r '.ref' pack.yaml", returnStdout: true).trim()
    }
    
    stages {
        stage('Test') {
            steps {
                sh 'attune pack test .'
            }
        }
        
        stage('Build') {
            when {
                tag pattern: "v.*", comparator: "REGEXP"
            }
            steps {
                sh "tar -czf pack-${PACK_VERSION}.tar.gz --exclude='.git' ."
                archiveArtifacts artifacts: "pack-${PACK_VERSION}.tar.gz"
            }
        }
        
        stage('Publish') {
            when {
                tag pattern: "v.*", comparator: "REGEXP"
            }
            steps {
                script {
                    // Upload to artifact repository
                    sh """
                        curl -u ${ARTIFACTORY_CREDS} \
                            -T pack-${PACK_VERSION}.tar.gz \
                            "https://artifactory.example.com/packs/${PACK_REF}/${PACK_VERSION}/"
                    """
                    
                    // Update registry
                    sh """
                        ARCHIVE_CHECKSUM="sha256:\$(sha256sum "pack-${PACK_VERSION}.tar.gz" | cut -d ' ' -f1)"
                        rm "pack-${PACK_VERSION}.tar.gz"
                        REGISTRY_DIR=\$(mktemp -d)
                        git clone https://${REGISTRY_CREDS}@github.com/your-org/attune-registry.git "\${REGISTRY_DIR}"
                        
                        attune pack index-update \
                            --index "\${REGISTRY_DIR}/index.json" \
                            . \
                            --git-url "${GIT_URL}" \
                            --git-ref "${GIT_COMMIT}" \
                            --archive-url "https://artifactory.example.com/packs/${PACK_REF}/${PACK_VERSION}/pack-${PACK_VERSION}.tar.gz" \
                            --archive-checksum "\${ARCHIVE_CHECKSUM}" \
                            --update
                        
                        cd "\${REGISTRY_DIR}"
                        git config user.name "Jenkins"
                        git config user.email "jenkins@example.com"
                        git add index.json
                        git commit -m "Update ${PACK_REF} to ${PACK_VERSION}"
                        git push
                    """
                }
            }
        }
    }
}
```

## Best Practices

### 1. Versioning Strategy

Use semantic versioning for packs:
- **Major**: Breaking changes to actions/sensors
- **Minor**: New features, backward compatible
- **Patch**: Bug fixes

```yaml
# pack.yaml
version: "2.1.3"
```

### 2. Automated Testing

Always run pack tests before publishing:

```bash
# In CI pipeline
attune pack test . --detailed || exit 1
```

### 3. Checksum Verification

Always generate and include checksums:

```bash
ARCHIVE_PATH="${TMPDIR:-/tmp}/pack-${VERSION}.tar.gz"
tar -czf "$ARCHIVE_PATH" --exclude='.git' .
ARCHIVE_CHECKSUM="sha256:$(sha256sum "$ARCHIVE_PATH" | cut -d ' ' -f1)"
```

The CLI computes Attune's framed directory checksum for Git sources. Archive
sources require `--archive-checksum`; calculate it from the completed archive
file so it covers the exact bytes that will be uploaded and downloaded.

### 4. Registry Security

- Use separate tokens for registry access
- Never commit tokens to source control
- Use CI/CD secrets management
- Rotate tokens regularly

### 5. Archive Hosting

Options for hosting pack archives:
- **GitHub Releases**: Free, integrated with source
- **GitLab Package Registry**: Built-in package management
- **Artifactory/Nexus**: Enterprise artifact management
- **S3/Cloud Storage**: Scalable, CDN-friendly

### 6. Registry Structure

Maintain a separate git repository for your registry:

```
attune-registry/
├── index.json          # Main registry index
├── README.md           # Usage instructions
├── .github/
│   └── workflows/
│       └── validate.yml # Index validation
└── scripts/
    └── validate.sh     # Validation script
```

### 7. Index Validation

Add validation to registry repository:

```yaml
# .github/workflows/validate.yml
name: Validate Index

on: [pull_request, push]

jobs:
  validate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Validate JSON
        run: |
          jq empty index.json || exit 1
      - name: Check schema
        run: |
          # Ensure required fields exist
          jq -e '.packs | length > 0' index.json
          jq -e '.packs[] | .ref, .version, .install_sources' index.json
```

## Manual Publishing Workflow

For manual/local publishing:

```bash
#!/bin/bash
# publish-pack.sh

set -e

PACK_DIR="${1:-}"
REGISTRY_DIR="${2:-}"

if [ -z "$PACK_DIR" ] || [ -z "$REGISTRY_DIR" ]; then
    echo "Usage: $0 <pack-dir> <registry-dir>"
    exit 1
fi

PACK_DIR=$(realpath "$PACK_DIR")
REGISTRY_DIR=$(realpath "$REGISTRY_DIR")

cd "$PACK_DIR"

# Extract metadata
PACK_REF=$(yq -r '.ref' pack.yaml)
VERSION=$(yq -r '.version' pack.yaml)
GIT_SHA=$(git rev-parse HEAD)

echo "Publishing ${PACK_REF} v${VERSION}..."

# Run tests
echo "Running tests..."
attune pack test . || exit 1

# Create archive
echo "Creating archive..."
ARCHIVE_NAME="pack-${VERSION}.tar.gz"
tar -czf "/tmp/${ARCHIVE_NAME}" --exclude='.git' .

# Hash the exact archive bytes
echo "Calculating archive checksum..."
ARCHIVE_CHECKSUM="sha256:$(sha256sum "/tmp/${ARCHIVE_NAME}" | cut -d ' ' -f1)"
echo "Archive checksum: $ARCHIVE_CHECKSUM"

# Upload archive (customize for your storage)
echo "Uploading archive..."
# aws s3 cp "/tmp/${ARCHIVE_NAME}" "s3://my-bucket/packs/${PACK_REF}/${VERSION}/"
# OR
# scp "/tmp/${ARCHIVE_NAME}" "server:/path/to/packs/${PACK_REF}/${VERSION}/"

# Update registry
echo "Updating registry index..."
cd "$REGISTRY_DIR"
git pull

attune pack index-update \
    --index index.json \
    "$PACK_DIR" \
    --git-url "https://github.com/your-org/${PACK_REF}" \
    --git-ref "$GIT_SHA" \
    --archive-url "https://storage.example.com/packs/${PACK_REF}/${VERSION}/${ARCHIVE_NAME}" \
    --archive-checksum "$ARCHIVE_CHECKSUM" \
    --update

# Commit and push
git add index.json
git commit -m "Add ${PACK_REF} v${VERSION}"
git push

echo "✓ Successfully published ${PACK_REF} v${VERSION}"
```

## Troubleshooting

### Issue: Checksum Mismatch

```bash
# Verify exact archive bytes locally
sha256sum /path/to/pack.tar.gz

# Re-generate archive with consistent settings
tar --sort=name --mtime='@0' --owner=0 --group=0 --numeric-owner \
    -czf pack.tar.gz --exclude='.git' pack/
```

### Issue: Registry Update Fails

```bash
# Validate index.json syntax
jq empty index.json

# Check for duplicate refs
jq -r '.packs[].ref' index.json | sort | uniq -d
```

### Issue: Pack Tests Fail in CI

```bash
# Run with verbose output
attune pack test . --detailed --verbose

# Check runtime dependencies
attune pack test . 2>&1 | grep -i "dependency"
```

## See Also

- [Pack Registry Documentation](pack-registry.md)
- [Pack Testing Framework](pack-testing-framework.md)
- [Pack Install with Testing](pack-install-testing.md)
- [API Pack Testing](api-pack-testing.md)
