# GitHub publishing and Linux packages

This repository now includes:

- A GitHub Actions publish workflow at `.github/workflows/publish.yml`
- OCI-published container images for the Kubernetes deployment path
- A Helm chart at `charts/attune`
- Linux packages attached to each GitHub release, with optional Nexus repository publication

## What Gets Published

The workflow publishes these images to GitHub Container Registry by default:

- `attune/api`
- `attune/executor`
- `attune/notifier`
- `attune/supervisor`
- `attune/agent`
- `attune/web`
- `attune/migrations`
- `attune/init-user`
- `attune/init-packs`

The Helm chart is pushed as an OCI chart:

- `oci://ghcr.io/<namespace>/attune/charts`

Linux packages are attached to the GitHub release. For stable releases, the
workflow also publishes Debian and RPM packages to Nexus Repository Manager 3
and can publish Arch packages to a raw Nexus repository.

Binary bundles are uploaded as per-architecture workflow artifacts named
`attune-binaries-amd64` and `attune-binaries-arm64`. Tag builds attach those
`attune-binaries-{arch}.tar.gz` files directly to the GitHub Release.

## Required GitHub Repository Configuration

Set these variables:

- `CONTAINER_REGISTRY_HOST`: Optional registry hostname override. If omitted, the workflow uses `ghcr.io`.
- `CONTAINER_REGISTRY_NAMESPACE`: Optional override for the registry namespace. If omitted, the workflow uses the repository owner lowercased. GHCR publishes with a lowercased namespace.
- `NEXUS_URL`: Base URL for Nexus, for example `https://nexus.example.com`.
- `NEXUS_APT_REPOSITORY`: Optional hosted apt repository name. Defaults to `attune-apt`.
- `NEXUS_YUM_REPOSITORY`: Optional hosted yum/RPM repository name. Defaults to `attune-yum`.
- `NEXUS_RAW_REPOSITORY`: Optional raw repository for Arch `.pkg.tar.zst` packages. If omitted, Arch package upload is skipped.
- `NEXUS_APT_COMPONENT`: Optional Debian component path segment. Defaults to `main`.

Set one of these container registry authentication options:

- Preferred: `CONTAINER_REGISTRY_USERNAME` and `CONTAINER_REGISTRY_PASSWORD`
- Fallback: allow the workflow `GITHUB_TOKEN` to push packages and release assets

Set these repository secrets for stable Nexus publication:

- `NEXUS_USERNAME`
- `NEXUS_PASSWORD`

Set these secrets to publish the platform-specific CLI packages:

- `HOMEBREW_TAP_TOKEN`: Writes the stable-release cask to `attune-system/homebrew-attune-client-tap`.
- `CHOCOLATEY_API_KEY`: Publishes the stable-release `attune-cli` package to Chocolatey.
- `ARCH_PACKAGE_TOKEN`: Fine-grained GitHub token with Contents read/write access to `attune-system/aur-attune-bin`.

Set these secrets to sign and notarize the macOS CLI archives:

- `APPLE_DEVELOPER_ID_CERTIFICATE_P12_BASE64`: Base64-encoded password-protected Developer ID Application `.p12` export.
- `APPLE_DEVELOPER_ID_CERTIFICATE_PASSWORD`: Password protecting that `.p12` export.
- `APPLE_NOTARY_PRIVATE_KEY_P8_BASE64`: Base64-encoded contents of the API key's `.p8` private-key file.

Set these repository variables for notarization:

- `APPLE_NOTARY_KEY_ID`: Key ID for an App Store Connect API key authorized for notarization.
- `APPLE_NOTARY_ISSUER_ID`: Issuer ID of the App Store Connect API key.

The macOS release job fails if these credentials are absent. It signs `attune`
and `attune-mcp` with hardened runtime and a secure timestamp, notarizes the
same signed binaries, and then packages them for the GitHub Release and
Homebrew cask. The submission job stores the signed binaries and Apple
submission ID as a 30-day workflow artifact. The separate finalization job
polls that existing submission for up to 25 minutes without resubmitting; if
it remains pending, rerun only the failed finalization job from GitHub Actions.

## Publish Behavior

The workflow runs only on pushed tags matching `v*`. A shared release gate
checks the tag version and requires successful CI for the tagged commit. Build
jobs then produce retained artifacts without publishing to external
destinations.

The GitHub release job downloads and verifies every required asset family,
creates or resumes the draft release, uploads the assets, and makes the release
public. Homebrew, Chocolatey, and Arch publication starts from that public
release. OCI images, the OCI Helm chart, and Nexus packages publish on separate
job branches. A failure in one destination does not block unrelated
destinations.

Fix a failed destination and rerun its failed jobs. Do not create or move the
release tag. A rerun accepts an existing release asset only when its bytes are
identical; it refuses to overwrite different content or add a missing asset to
an already-public release. Homebrew, Arch, and Chocolatey also detect
already-current output before writing it again.

Architecture image pushes and Rust multi-architecture manifest pushes make
three attempts with a five-second delay. Nexus package uploads use curl's retry
handling for five retries with a two-second delay, including non-default
transient errors. macOS notarization finalization polls the existing submission
and never submits a second notarization request during a retry.

Linux package builds do not require Nexus, so the `.deb`, `.rpm`, and
`.pkg.tar.zst` files can reach the GitHub release even if Nexus publication
fails. Stable releases still report the Nexus destination as failed when its
URL or credentials are missing.

For a stable tag such as `v0.4.1`, container images are published with
`0.4.1`, `latest`, and `sha-<12-char-sha>` tags. The workflow publishes the
Homebrew cask, Chocolatey package, and `attune-bin` Arch package repository
only for stable `vX.Y.Z` tags. Their destination jobs fail when the required
credentials are absent. The Arch package installs both `attune` and
`attune-mcp` from the checksummed Linux release archives. The repository is
ready to push to AUR when an AUR account becomes available.

The Linux package set includes split packages for individual components and an
all-in-one `attune` installer package. The all-in-one package is self-contained:
it installs the API, executor, worker, sensor, notifier, supervisor, CLI, MCP,
and agent binaries together under `/opt/attune-system/`, installs service units
that run from that directory, and symlinks the interactive `attune` and
`attune-mcp` commands into `/usr/bin`. It conflicts with the split `attune-*`
packages so the same files are not owned by multiple packages. Use `attune-cli`
for a CLI-only install, or `attune` for a cohesive local service install.

Chart packaging behavior:

- release tags package the chart with the tag version, for example `0.4.1`

## Helm Install Flow

Log in to the registry:

```bash
helm registry login ghcr.io --username <user>
```

Install the chart:

```bash
helm install attune oci://ghcr.io/<namespace>/attune/charts/attune \
  --version 0.4.1 \
  --set global.imageRegistry=ghcr.io \
  --set global.imageNamespace=<namespace> \
  --set global.imageTag=0.4.1 \
  --set packRegistry.standardIndexRef=<40-character-index-commit-sha> \
  --set web.config.apiUrl=https://attune.example.com/api \
  --set web.config.wsUrl=wss://attune.example.com/ws
```

## Chart Expectations

The chart defaults to deploying:

- PostgreSQL via TimescaleDB
- RabbitMQ
- Attune API, executor, worker, sensor, notifier, and web services
- Migration, test-user bootstrap, and built-in pack bootstrap jobs

Important constraints:

- The shared `packs`, `runtime_envs`, and `artifacts` claims default to `ReadWriteMany`
- Your cluster storage class must support RWX for the default values to work as written
- `web.config.apiUrl` and `web.config.wsUrl` must be browser-reachable URLs, not cluster-internal service DNS names
- The default security and bootstrap values in `charts/attune/values.yaml` are placeholders and should be overridden
- `packRegistry.standardIndexRef` must be an immutable 40-character lowercase
  commit SHA. Pin the catalog snapshot tested for the release instead of a branch.

## Suggested First Release Sequence

1. Push the workflow and chart changes.
2. Create `attune-system/aur-attune-bin`, then configure registry credentials
   and, if desired, the Homebrew, Chocolatey, and Arch package credentials.
   `ARCH_PACKAGE_TOKEN` must have Contents read/write access to that repository.
3. Create and push the `v0.4.1` release tag.
4. Install the chart using the `0.4.1` image tag and chart version.
