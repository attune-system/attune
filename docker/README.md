# Attune Docker Configuration

This directory contains Docker-related files for building and running Attune services.

> **⚠️ Important**: When building multiple services in parallel, you may encounter race conditions. See [DOCKER_BUILD_RACE_CONDITIONS.md](./DOCKER_BUILD_RACE_CONDITIONS.md) for solutions and recommended workflows.

## Quick Start

### Default User Credentials

When you start Attune with Docker Compose, a default test user is **automatically created**:

- **Login**: `test@attune.local`
- **Password**: `TestPass123!`

This happens via the `init-user` service which runs after database migrations complete.

### Test Login

```bash
curl -X POST http://localhost:8080/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"login":"test@attune.local","password":"TestPass123!"}'
```

> **⚠️ Security Note**: This default user is for development/testing only. Never use these credentials in production!

## Files

### Dockerfiles

- **`Dockerfile.optimized`** - Multi-stage Dockerfile for Rust services (API, Executor, Notifier)
  - Uses build argument `SERVICE` to select the runtime binary to copy
  - Compiles only the shared `attune-api`, `attune-executor`, and `attune-notifier` binaries
  - Example: `docker build --build-arg SERVICE=api -f docker/Dockerfile.optimized -t attune-api .`

- **`Dockerfile.agent`** - Multi-stage Dockerfile for the statically-linked agent image
  - Compiles only `attune-agent` and `attune-sensor-agent`
  - Builds the `agent-init` image used to populate the shared agent binary volume

- **`Dockerfile.pack-binaries`** - Pack binary builder used by `scripts/build-pack-binaries.sh`
  - Compiles only the `attune-core-timer-sensor` binary

- **`Dockerfile.init-packs-package`** - Release image for built-in pack files
  - Includes the prebuilt `attune-core-timer-sensor` from the release architecture bundle

- **`Dockerfile.web`** - Multi-stage Dockerfile for React Web UI
  - Builds with Node.js and serves with Nginx
  - Includes runtime environment variable injection

### Configuration Files

- **`nginx.conf`** - Nginx configuration for serving Web UI and proxying API/WebSocket requests
  - Serves static React assets
  - Proxies `/api/*` to API service
  - Proxies `/ws/*` to Notifier service (WebSocket)
  - Includes security headers and compression

- **`inject-env.sh`** - Script to inject runtime environment variables into Web UI
  - Runs at container startup
  - Creates `runtime-config.js` with API and WebSocket URLs

### Initialization Scripts

- **`init-db.sh`** - Database initialization script (optional)
  - Waits for PostgreSQL readiness
  - Creates schema and runs migrations
  - Can be used for manual DB setup

- **`init-user.sh`** - Default user initialization script
  - **Automatically creates** test user on first startup
  - Idempotent - safe to run multiple times
  - Creates user: `test@attune.local` / `TestPass123!`
  - Uses pre-computed Argon2id password hash
  - Skips creation if user already exists

- **`run-migrations.sh`** - Database migration runner
  - Runs SQLx migrations automatically on startup
  - Used by the `migrations` service in docker-compose

### Docker Compose

The main `docker compose.yaml` is in the project root. It orchestrates:

- Infrastructure: PostgreSQL, RabbitMQ, Redis
- Services: API, Executor, Worker, Sensor, Notifier
- Web UI: React frontend with Nginx

## Building Images

### Build All Services (Recommended Method)

To avoid race conditions during parallel builds, pre-warm the cache first:

```bash
cd /path/to/attune

# Enable BuildKit for faster incremental builds (recommended)
export DOCKER_BUILDKIT=1
export COMPOSE_DOCKER_CLI_BUILD=1

# Step 1: Pre-warm the build cache (builds API service only)
make docker-cache-warm

# Step 2: Build all services (faster and more reliable)
make docker-build
```

Or build directly with docker compose:

```bash
docker compose build
```

**Note**: The Dockerfile uses `sharing=locked` on cache mounts to prevent race conditions. This makes parallel builds sequential but ensures 100% reliability. See [DOCKER_BUILD_RACE_CONDITIONS.md](./DOCKER_BUILD_RACE_CONDITIONS.md) for details.

### Build Individual Service

```bash
# Enable BuildKit first
export DOCKER_BUILDKIT=1

# API service
docker compose build api

# Web UI
docker compose build web

# Notifier service
docker compose build notifier
```

### Build with Custom Args

```bash
# Build API with specific Rust version
DOCKER_BUILDKIT=1 docker build \
  --build-arg SERVICE=api \
  --build-arg RUST_VERSION=1.92 \
  -f docker/Dockerfile.optimized \
  -t attune-api:custom \
  .
```

### Enable BuildKit Globally

BuildKit dramatically speeds up incremental builds by caching compilation artifacts.

```bash
# Run the configuration script
./docker/enable-buildkit.sh

# Or manually add to your shell profile (~/.bashrc, ~/.zshrc, etc.)
export DOCKER_BUILDKIT=1
export COMPOSE_DOCKER_CLI_BUILD=1

# Apply changes
source ~/.bashrc  # or ~/.zshrc
```

## Image Structure

### Rust Services

**Builder Stage:**
- Base: `rust:1.92-bookworm`
- Installs build dependencies
- Compiles the shared API, Executor, and Notifier binaries in release mode
- **Uses BuildKit cache mounts for incremental builds**
- Build time: 
  - First build: ~5-6 minutes
  - Incremental builds (with BuildKit): ~30-60 seconds
  - Without BuildKit: ~5-6 minutes every time

**Runtime Stage:**
- Base: `debian:bookworm-slim`
- Minimal runtime dependencies (ca-certificates, libssl3, curl)
- Runs as non-root user `attune` (UID 1000)
- Binary copied to `/usr/local/bin/attune-service`
- Configuration in `/opt/attune/`
- Packs directory: `/opt/attune/packs`

### Web UI

**Builder Stage:**
- Base: `node:20-alpine`
- Installs npm dependencies
- Builds React application with Vite

**Runtime Stage:**
- Base: `nginx:1.25-alpine`
- Custom Nginx configuration
- Static files in `/usr/share/nginx/html`
- Environment injection script at startup

## Environment Variables

Rust services support environment-based configuration with `ATTUNE__` prefix:

```bash
# Database
ATTUNE__DATABASE__URL=postgresql://user:pass@host:5432/db

# Message Queue
ATTUNE__MESSAGE_QUEUE__URL=amqp://user:pass@host:5672

# Security
JWT_SECRET=your-secret-here
ENCRYPTION_KEY=your-32-char-key-here

# Logging
RUST_LOG=debug
```

Web UI environment variables:

```bash
API_URL=http://localhost:8080
WS_URL=ws://localhost:8081
ENVIRONMENT=production
```

## Volumes

The following volumes are used:

**Data Volumes:**
- `postgres_data` - PostgreSQL database files
- `rabbitmq_data` - RabbitMQ data
- `redis_data` - Redis persistence

**Log Volumes:**
- `api_logs`, `executor_logs`, `worker_logs`, `sensor_logs`, `notifier_logs`

**Temporary:**
- `worker_temp` - Worker service temporary files

**Bind Mounts:**
- `./packs:/opt/attune/packs:ro` - Read-only pack files

## Networking

All services run on the `attune-network` bridge network with subnet `172.28.0.0/16`.

**Service Communication:**
- Services communicate using container names as hostnames
- Example: API connects to `postgres:5432`, `rabbitmq:5672`

**External Access:**
- API: `localhost:8080`
- Notifier WebSocket: `localhost:8081`
- Web UI: `localhost:3000`
- RabbitMQ Management: `localhost:15672`
- PostgreSQL: `localhost:5432`

## Health Checks

All services have health checks configured:

**API:**
```bash
curl -f http://localhost:8080/health
```

**Web UI:**
```bash
wget --spider http://localhost:80/health
```

**PostgreSQL:**
```bash
pg_isready -U attune
```

**RabbitMQ:**
```bash
rabbitmq-diagnostics -q ping
```

**Background Services:**
Process existence check with `pgrep`

## Security

### Non-Root User

All services run as non-root user `attune` (UID 1000) for security.

### Secrets Management

**Development:**
- Secrets in `.env` file (not committed to git)
- Default values for testing only

**Production:**
- Use Docker secrets or external secrets manager
- Never hardcode secrets in images
- Rotate secrets regularly

### Network Security

- Services isolated on private network
- Only necessary ports exposed to host
- Use TLS/SSL for external connections
- Dedicated bridge network
- Proper startup dependencies

## Optimization

### BuildKit Cache Mounts (Recommended)

**Enable BuildKit** for dramatically faster incremental builds:

```bash
export DOCKER_BUILDKIT=1
docker compose build
```

**How it works:**
- Persists `/usr/local/cargo/registry` (downloaded crates, ~1-2GB)
- Persists `/usr/local/cargo/git` (git dependencies)
- Persists `/build/target` (compilation artifacts, ~5-10GB)

**Performance improvement:**
- First build: ~5-6 minutes
- Code-only changes: ~30-60 seconds (vs 5+ minutes without caching)
- Dependency changes: ~2-3 minutes (vs full rebuild)

**Manage cache:**
```bash
# View cache size
docker system df

# Clear build cache
docker builder prune

# Clear specific cache
docker builder prune --filter type=exec.cachemount
```

### Layer Caching

The Dockerfiles are also optimized for Docker layer caching:

1. Copy manifests first
2. Download and compile dependencies
3. Copy actual source code last
4. Source code changes don't invalidate dependency layers

### Image Size

**Rust Services:**
- Multi-stage build reduces size
- Only runtime dependencies in final image
- Typical size: 140-180MB per service

**Web UI:**
- Static files only in final image
- Alpine-based Nginx
- Typical size: 50-80MB

### Build Time

**With BuildKit (recommended):**
- First build: ~5-6 minutes
- Code-only changes: ~30-60 seconds
- Dependency changes: ~2-3 minutes

**Without BuildKit:**
- Every build: ~5-6 minutes

**Enable BuildKit:**
```bash
./docker/enable-buildkit.sh
# or
export DOCKER_BUILDKIT=1
```

## Troubleshooting

### Build Failures

**Slow builds / No caching:**
If builds always take 5+ minutes even for small code changes, BuildKit may not be enabled.

Solution:
```bash
# Check if BuildKit is enabled
echo $DOCKER_BUILDKIT

# Enable BuildKit
export DOCKER_BUILDKIT=1
export COMPOSE_DOCKER_CLI_BUILD=1

# Add to shell profile for persistence
echo 'export DOCKER_BUILDKIT=1' >> ~/.bashrc
echo 'export COMPOSE_DOCKER_CLI_BUILD=1' >> ~/.bashrc

# Or use the helper script
./docker/enable-buildkit.sh

# Rebuild
docker compose build
```

**Cargo.lock version error:**
```
error: failed to parse lock file at: /build/Cargo.lock
Caused by:
  lock file version `4` was found, but this version of Cargo does not understand this lock file
```

Solution: Update Rust version in the optimized Dockerfile
```bash
# Edit docker/Dockerfile.optimized and change:
ARG RUST_VERSION=1.75
# to:
ARG RUST_VERSION=1.92
```

Cargo.lock version 4 requires Rust 1.82+. The project uses Rust 1.92.

**Cargo dependencies fail:**
```bash
# Clear Docker build cache
docker builder prune -a

# Rebuild without cache
docker compose build --no-cache
```

### Runtime Issues

**Service won't start:**
```bash
# Check logs
docker compose logs <service-name>

# Check health
docker compose ps
```

**Database connection fails:**
```bash
# Verify PostgreSQL is ready
docker compose exec postgres pg_isready -U attune

# Check connection from service
docker compose exec api /bin/sh
# Then: curl postgres:5432
```

**Permission errors:**
```bash
# Fix volume permissions
sudo chown -R 1000:1000 ./packs ./logs
```

## Development Workflow

### Local Development with Docker

```bash
# Start infrastructure only
docker compose up -d postgres rabbitmq redis

# Run services locally
cargo run --bin attune-api
cargo run --bin attune-worker

# Or start everything
docker compose up -d
```

### Rebuilding After Code Changes

```bash
# Rebuild and restart specific service
docker compose build api
docker compose up -d api

# Rebuild all
docker compose build
docker compose up -d
```

### Debugging

```bash
# Access service shell
docker compose exec api /bin/sh

# View logs in real-time
docker compose logs -f api worker

# Check resource usage
docker stats
```

## Production Deployment

See [Docker Deployment Guide](../docs/docker-deployment.md) for:
- Production configuration
- Security hardening
- Scaling strategies
- Monitoring setup
- Backup procedures
- High availability

## CI/CD Integration

Example GitHub Actions workflow:

```yaml
- name: Build Docker images
  run: docker compose build

- name: Run tests
  run: docker compose run --rm api cargo test

- name: Push to registry
  run: |
    docker tag attune-api:latest registry.example.com/attune-api:${{ github.sha }}
    docker push registry.example.com/attune-api:${{ github.sha }}
```

## Maintenance

### Updating Images

```bash
# Pull latest base images
docker compose pull

# Rebuild services
docker compose build --pull

# Restart with new images
docker compose up -d
```

### Cleaning Up

```bash
# Remove stopped containers
docker compose down

# Remove volumes (WARNING: deletes data)
docker compose down -v

# Clean up unused images
docker image prune -a

# Full cleanup
docker system prune -a --volumes
```

## References

- [Docker Compose Documentation](https://docs.docker.com/compose/)
- [Multi-stage Builds](https://docs.docker.com/build/building/multi-stage/)
- [Dockerfile Best Practices](https://docs.docker.com/develop/dev-best-practices/)
- [Main Documentation](../docs/docker-deployment.md)
