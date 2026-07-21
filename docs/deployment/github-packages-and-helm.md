# GitHub Publishing And Nexus Linux Packages

This repository now includes:

- A GitHub Actions publish workflow at `.github/workflows/publish.yml`
- OCI-published container images for the Kubernetes deployment path
- A Helm chart at `charts/attune`
- Nexus-published Linux packages plus Docker distribution, Helm chart, and binary bundle archives

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

Linux packages are published to Nexus Repository Manager 3. GitHub Packages
supports ecosystems such as OCI containers, npm, Maven, NuGet, RubyGems, and
Cargo, but it does not provide native Debian/RPM/Arch repository hosting.

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

Set these Nexus credentials as repository secrets:

- `NEXUS_USERNAME`
- `NEXUS_PASSWORD`

Set these secrets to publish the platform-specific CLI packages:

- `HOMEBREW_TAP_TOKEN`: Writes the stable-release cask to `attune-system/homebrew-attune-client-tap`.
- `CHOCOLATEY_API_KEY`: Publishes the stable-release `attune-cli` package to Chocolatey.

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
submission ID as a 14-day workflow artifact. The separate finalization job
polls that existing submission for up to 25 minutes without resubmitting; if
it remains pending, rerun only the failed finalization job from GitHub Actions.

## Publish Behavior

The workflow runs only on pushed tags matching `v*`. Each release tag builds
and publishes every service image, the web image, Helm chart, Docker
distribution, Linux packages, and CLI archives.

For a stable tag such as `v0.2.0`, container images are published with
`0.2.0`, `latest`, and `sha-<12-char-sha>` tags. The workflow publishes the
Homebrew cask and Chocolatey package only for stable `vX.Y.Z` tags and only
when their respective credentials are configured.

The Linux package set includes split packages for individual components and an
all-in-one `attune` installer package. The all-in-one package is self-contained:
it installs the API, executor, worker, sensor, notifier, supervisor, CLI, MCP,
and agent binaries together under `/opt/attune-system/`, installs service units
that run from that directory, and symlinks the interactive `attune` and
`attune-mcp` commands into `/usr/bin`. It conflicts with the split `attune-*`
packages so the same files are not owned by multiple packages. Use `attune-cli`
for a CLI-only install, or `attune` for a cohesive local service install.

Chart packaging behavior:

- release tags package the chart with the tag version, for example `0.2.0`

## Helm Install Flow

Log in to the registry:

```bash
helm registry login ghcr.io --username <user>
```

Install the chart:

```bash
helm install attune oci://ghcr.io/<namespace>/attune/charts/attune \
  --version 0.2.0 \
  --set global.imageRegistry=ghcr.io \
  --set global.imageNamespace=<namespace> \
  --set global.imageTag=0.2.0 \
  --set web.config.apiUrl=https://attune.example.com/api \
  --set web.config.wsUrl=wss://attune.example.com/ws
```

## Chart Expectations

The chart defaults to deploying:

- PostgreSQL via TimescaleDB
- RabbitMQ
- Redis
- Attune API, executor, worker, sensor, notifier, and web services
- Migration, test-user bootstrap, and built-in pack bootstrap jobs

Important constraints:

- The shared `packs`, `runtime_envs`, and `artifacts` claims default to `ReadWriteMany`
- Your cluster storage class must support RWX for the default values to work as written
- `web.config.apiUrl` and `web.config.wsUrl` must be browser-reachable URLs, not cluster-internal service DNS names
- The default security and bootstrap values in `charts/attune/values.yaml` are placeholders and should be overridden

## Suggested First Release Sequence

1. Push the workflow and chart changes.
2. Configure registry credentials and, if desired, the Homebrew and Chocolatey secrets.
3. Create and push the `v0.2.0` release tag.
4. Install the chart using the `0.2.0` image tag and chart version.
