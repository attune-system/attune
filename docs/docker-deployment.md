# Docker Deployment Guide

This guide explains how to deploy Attune using Docker and Docker Compose.

## Table of Contents

- [Overview](#overview)
- [Prerequisites](#prerequisites)
- [Quick Start](#quick-start)
- [Configuration](#configuration)
- [Building Images](#building-images)
- [Running Services](#running-services)
- [Monitoring](#monitoring)
- [Troubleshooting](#troubleshooting)
- [Production Considerations](#production-considerations)

## Overview

Attune uses Docker Compose to orchestrate multiple services:

- **Infrastructure Services**:
  - PostgreSQL 16 (database)
  - RabbitMQ 3.13 (message queue)
  - Redis 7 (cache)

- **Attune Services**:
  - `attune-api` - REST API gateway (port 8080)
  - `attune-executor` - Execution orchestration
  - `attune-worker` - Action execution
  - `attune-sensor` - Event monitoring
  - `attune-notifier` - WebSocket notifications (port 8081)
  - `attune-web` - React web UI (port 3000)

All services communicate via a dedicated Docker network and use persistent volumes for data storage.

## Prerequisites

- Docker Engine 20.10+ or Docker Desktop
- Docker Compose 2.0+
- At least 4GB RAM available for Docker
- At least 10GB free disk space (15-20GB recommended with BuildKit cache)
- Docker BuildKit enabled (recommended for fast incremental builds)

### Install Docker

**Linux:**
```bash
curl -fsSL https://get.docker.com -o get-docker.sh
sudo sh get-docker.sh
sudo usermod -aG docker $USER
```

**macOS/Windows:**
Download and install Docker Desktop from https://www.docker.com/products/docker-desktop/

## Quick Start

### 1. Clone the Repository

```bash
git clone <repository-url>
cd attune
```

### 2. Configure Environment Variables

Copy the example environment file and customize it:

```bash
cp env.docker.example .env
```

**IMPORTANT**: Edit `.env` and set secure values for:
- `JWT_SECRET` - Generate with: `openssl rand -base64 32`
- `ENCRYPTION_KEY` - Generate with: `openssl rand -base64 32`

### 3. Enable BuildKit (Recommended)

BuildKit dramatically speeds up incremental builds:

```bash
# Enable for current session
export DOCKER_BUILDKIT=1
export COMPOSE_DOCKER_CLI_BUILD=1

# Or enable globally using the helper script
./docker/enable-buildkit.sh

# Or manually add to ~/.bashrc or ~/.zshrc
echo 'export DOCKER_BUILDKIT=1' >> ~/.bashrc
echo 'export COMPOSE_DOCKER_CLI_BUILD=1' >> ~/.bashrc
source ~/.bashrc
```

**Build time comparison:**
- With BuildKit: First build ~5-6 min, incremental ~30-60 sec
- Without BuildKit: Every build ~5-6 min

### 4. Start All Services

```bash
docker-compose up -d
```

This will:
1. Pull/build all required images (first build takes ~5-6 minutes with BuildKit)
2. Create volumes and networks
3. Start all services in the correct order
4. Run database migrations

### 5. Verify Services

Check that all services are running:

```bash
docker-compose ps
```

All services should show `Up` status with healthy health checks.

### 6. Access the Application

- **Web UI**: http://localhost:3000
- **API Documentation**: http://localhost:8080/api-spec/swagger-ui/
- **RabbitMQ Management**: http://localhost:15672 (user: attune, pass: attune)

### 7. Create Initial Admin User

```bash
docker-compose exec api attune-service --create-admin-user
```

Or use the CLI:

```bash
# Install CLI
cargo install --path crates/cli

# Create user
attune auth register --username admin --email admin@example.com --password <password>
```

## Configuration

### Environment Variables

The `.env` file contains all configurable settings. Key variables:

#### Security (Required)
- `JWT_SECRET` - Secret for JWT token signing
- `ENCRYPTION_KEY` - Key for encrypting secrets (min 32 chars)

#### Service URLs
- `API_URL` - External URL for API (default: http://localhost:8080)
- `WS_URL` - External URL for WebSocket (default: ws://localhost:8081)

#### Infrastructure
- `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB` - Database credentials
- `RABBITMQ_DEFAULT_USER`, `RABBITMQ_DEFAULT_PASS` - Message queue credentials

#### Application
- `ENVIRONMENT` - Environment name (docker, production, etc.)
- `RUST_LOG` - Log level (trace, debug, info, warn, error)

### Configuration Files

The Docker deployment uses `config.docker.yaml` which overrides base settings:

- Database URL: `postgresql://attune:attune@postgres:5432/attune`
- Message Queue: `amqp://attune:attune@rabbitmq:5672`
- Metadata cache: `metadata_cache.url: redis://valkey:6379`
- Packs directory: `/opt/attune/packs`

### Custom Configuration

To override specific settings, use environment variables with the `ATTUNE__` prefix:

```bash
# In .env file
ATTUNE__SERVER__PORT=9090
ATTUNE__LOG__LEVEL=debug
ATTUNE__EXECUTOR__MAX_CONCURRENT_EXECUTIONS=100
```

## Building Images

### Build All Images

```bash
docker-compose build
```

### Build Specific Service

```bash
docker-compose build api
docker-compose build worker
docker-compose build web
```

### Build Arguments

The Rust services Dockerfile accepts build arguments:

```bash
# Always use DOCKER_BUILDKIT=1 for cache mounts
DOCKER_BUILDKIT=1 docker build \
  --build-arg SERVICE=api \
  --build-arg RUST_VERSION=1.92 \
  -f docker/Dockerfile \
  -t attune-api .
```

### Multi-Stage Build & BuildKit Cache Mounts

The Dockerfiles use multi-stage builds combined with BuildKit cache mounts:

**Multi-stage benefits:**
1. Compile with full Rust toolchain in builder stage
2. Minimize final image size (~140-180MB per service)
3. Separate build-time and runtime dependencies

**BuildKit cache mounts:**
- Persists Cargo registry (~1-2GB)
- Persists Git dependencies
- Persists incremental compilation artifacts (~5-10GB)

**Build Time:**
- First build: ~5-6 minutes
- Code-only changes (with BuildKit): ~30-60 seconds
- Dependency changes (with BuildKit): ~2-3 minutes
- Without BuildKit: ~5-6 minutes every time

**Enable BuildKit:**
```bash
export DOCKER_BUILDKIT=1
export COMPOSE_DOCKER_CLI_BUILD=1
```

## Running Services

### Start Services

```bash
# Start all services
docker-compose up -d

# Start specific services
docker-compose up -d postgres rabbitmq redis
docker-compose up -d api executor worker
```

### Stop Services

```bash
# Stop all services
docker-compose down

# Stop and remove volumes (WARNING: deletes data)
docker-compose down -v
```

### Restart Services

```bash
# Restart all services
docker-compose restart

# Restart specific service
docker-compose restart api
```

### View Logs

```bash
# All services
docker-compose logs -f

# Specific service
docker-compose logs -f api

# Last 100 lines
docker-compose logs --tail=100 api
```

### Scale Workers

You can run multiple worker instances:

```bash
docker-compose up -d --scale worker=3
```

## Monitoring

### Health Checks

All services have health checks configured. View status:

```bash
docker-compose ps
```

### Service Status

Check if a specific service is healthy:

```bash
# API health check
curl http://localhost:8080/health

# Web UI health check
curl http://localhost:3000/health

# Database
docker-compose exec postgres pg_isready -U attune

# RabbitMQ
docker-compose exec rabbitmq rabbitmq-diagnostics ping
```

### Resource Usage

```bash
# Container stats
docker stats

# Disk usage
docker system df
```

### Access Container Shell

```bash
# API service
docker-compose exec api /bin/sh

# Database
docker-compose exec postgres psql -U attune
```

## Troubleshooting

### Services Won't Start

1. **Check logs**:
   ```bash
   docker-compose logs <service-name>
   ```

2. **Verify dependencies**:
   ```bash
   docker-compose ps
   ```
   Ensure postgres, rabbitmq, and redis are healthy before starting application services.

3. **Check resource availability**:
   ```bash
   docker system df
   docker stats
   ```

### Database Connection Issues

```bash
# Test database connection
docker-compose exec postgres psql -U attune -c "SELECT version();"

# Check database logs
docker-compose logs postgres

# Verify migrations ran
docker-compose exec postgres psql -U attune -c "\dt attune.*"
```

### Message Queue Issues

```bash
# Check RabbitMQ status
docker-compose exec rabbitmq rabbitmqctl status

# List queues
docker-compose exec rabbitmq rabbitmqctl list_queues

# Access management UI
open http://localhost:15672
```

### Port Conflicts

If ports are already in use, modify `docker-compose.yaml` or use environment variables:

```yaml
# In docker-compose.yaml
ports:
  - "${API_PORT:-8080}:8080"
```

Then in `.env`:
```bash
API_PORT=9090
```

### Permission Issues

The services run as non-root user `attune` (UID 1000). If you have permission issues with volumes:

```bash
# Fix ownership
sudo chown -R 1000:1000 ./packs ./logs
```

### Reset Everything

To completely reset the environment:

```bash
# Stop and remove containers, networks, volumes
docker-compose down -v

# Remove images
docker-compose down --rmi all

# Clear build cache (if using BuildKit)
docker builder prune

# Remove all Docker data (CAREFUL!)
docker system prune -a --volumes
```

### BuildKit Cache Management

If using BuildKit (recommended), manage the build cache:

```bash
# View cache size
docker system df

# Clear build cache
docker builder prune

# Clear all unused cache
docker builder prune -a

# View what will be removed
docker builder prune --dry-run
```

BuildKit cache can grow to 5-10GB but dramatically speeds up rebuilds.

## Production Considerations

### Security

1. **Change default credentials**:
   - Generate strong `JWT_SECRET` and `ENCRYPTION_KEY`
   - Update database, RabbitMQ, and Redis passwords
   - Never commit `.env` to version control

2. **Use secrets management**:
   - Docker Swarm secrets
   - Kubernetes secrets
   - HashiCorp Vault
   - AWS Secrets Manager

3. **Network security**:
   - Use TLS/SSL for all external connections
   - Restrict network access with firewall rules
   - Use reverse proxy (Nginx, Traefik) for TLS termination

4. **Container security**:
   - Run containers as non-root (already configured)
   - Keep images updated
   - Scan images for vulnerabilities

### Performance

1. **Resource limits**:
   ```yaml
   api:
     deploy:
       resources:
         limits:
           cpus: '2'
           memory: 2G
         reservations:
           cpus: '1'
           memory: 1G
   ```

2. **Database tuning**:
   - Adjust `max_connections`, `shared_buffers`
   - Enable connection pooling
   - Use read replicas for heavy read loads

3. **Scaling**:
   - Scale worker services horizontally
   - Use load balancer for API instances
   - Separate read/write database connections

### Backup and Recovery

1. **Database backups**:
   ```bash
   # Backup
   docker-compose exec postgres pg_dump -U attune > backup.sql
   
   # Restore
   docker-compose exec -T postgres psql -U attune < backup.sql
   ```

2. **Volume backups**:
   ```bash
   # Backup volumes
   docker run --rm -v attune_postgres_data:/data -v $(pwd):/backup \
     alpine tar czf /backup/postgres_backup.tar.gz /data
   ```

3. **Automated backups**:
   - Use backup solutions (Velero, Restic)
   - Schedule regular backups with cron
   - Test restore procedures regularly

### Logging

1. **Centralized logging**:
   - Configure Docker logging driver
   - Use ELK stack, Loki, or CloudWatch
   - Set log retention policies

2. **Structured logging**:
   - Services already use JSON logging in Docker mode
   - Parse and index logs for analysis

### Monitoring

1. **Metrics collection**:
   - Prometheus + Grafana
   - DataDog, New Relic
   - Container metrics (cAdvisor)

2. **Alerting**:
   - Set up alerts for service failures
   - Monitor resource usage
   - Track error rates

### High Availability

1. **Database**:
   - PostgreSQL streaming replication
   - Automatic failover (Patroni, Stolon)
   - Regular backups

2. **Message Queue**:
   - RabbitMQ clustering
   - Mirrored queues
   - Load balancing

3. **Application**:
   - Run multiple instances
   - Use container orchestration (Kubernetes, Swarm)
   - Implement circuit breakers

## Makefile Integration

The project Makefile includes Docker commands:

```bash
# Build images
make docker-build

# Start services
make docker-up

# Stop services
make docker-down

# View logs
make docker-logs
```

## Next Steps

- Review [Production Deployment Guide](production-deployment.md) for production best practices
- Configure [Monitoring and Alerting](../README.md#monitoring)
- Set up [Backup and Recovery](../README.md#backup)
- Review [Security Guide](authentication/security-review-2024-01-02.md)

## Support

For issues and questions:
- Check [Troubleshooting](#troubleshooting) section
- Review service logs: `docker-compose logs <service>`
- Open an issue on GitHub
- Consult the [Documentation Index](../AGENTS.md)