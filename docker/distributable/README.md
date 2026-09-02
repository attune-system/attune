# Attune Docker Dist Bundle

This directory is a distributable Docker bundle built from the main workspace compose setup.

It is designed to run Attune without building the Rust services locally:

- `api`, `executor`, `supervisor`, `notifier`, `agent`, and `web` pull published images
- database and user bootstrap run from local scripts shipped in this bundle
- `init-packs` loads the built-in packs and native timer sensor from its published image
- workers and sensor still use stock runtime images plus the published injected agent binaries

## Registry Defaults

The compose file defaults to:

- registry: `ghcr.io/attune-system`
- tag: `latest`

Override them with env vars:

```bash
export ATTUNE_IMAGE_REGISTRY=ghcr.io/attune-system
export ATTUNE_IMAGE_TAG=latest
```

If the registry requires auth:

```bash
docker login ghcr.io
```

## Run

From this directory:

```bash
docker compose up -d
```

Or with an explicit tag:

```bash
ATTUNE_IMAGE_TAG=sha-xxxxxxxxxxxx docker compose up -d
```

## Rebuild Bundle

Refresh this bundle and create a tarball from the workspace root:

```bash
bash scripts/package-docker-dist.sh
```

## Included Assets

- `docker-compose.yaml` - published-image compose stack
- `config.docker.yaml` - container config mounted into services
- `docker/` - init scripts and SQL helpers
- `migrations/` - schema migrations for the bootstrap job

## Current Limitation

The publish workflow does not currently publish dedicated worker or sensor runtime images. This bundle therefore keeps using stock runtime images with the published `attune/agent` image for injection.
