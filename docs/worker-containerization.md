# Worker Containerization Design

## Overview

This document describes the design and implementation of containerized workers in Attune. Workers can run in containers and declare their runtime capabilities to the system, enabling intelligent scheduling of actions based on available execution environments.

## Architecture

### Worker Types and Runtime Capabilities

Workers declare their capabilities during registration through a standardized capability system:

```json
{
  "runtimes": ["shell", "python", "node", "native"],
  "max_concurrent_executions": 10,
  "container_version": "1.0.0"
}
```

### Runtime Capability Declaration

Workers can declare their capabilities in three ways (in order of precedence):

1. **Environment Variable** (highest priority):
   ```bash
   ATTUNE_WORKER_RUNTIMES="shell,python"
   ```

2. **Configuration File**:
   ```yaml
   worker:
     capabilities:
       runtimes: ["shell", "python"]
   ```

3. **Auto-detection** (fallback):
   - Check for runtime binaries in PATH
   - Default to `["shell"]` if nothing else is detected

### Worker Image Variants

#### 1. Base Worker (Shell Only)
- **Image**: `attune-worker:base`
- **Capabilities**: `["shell"]`
- **Use Case**: Lightweight workers for shell scripts, basic automation
- **Base OS**: Debian slim
- **Size**: ~580MB

#### 2. Python Worker
- **Image**: `attune-worker:python`
- **Capabilities**: `["shell", "python"]`
- **Use Case**: Python actions, scripts with dependencies
- **Base OS**: Python 3.11 slim
- **Size**: ~1.2GB
- **Includes**: pip, virtualenv, common Python libraries (requests, pyyaml, jinja2, python-dateutil)

#### 3. Node.js Worker
- **Image**: `attune-worker:node`
- **Capabilities**: `["shell", "node"]`
- **Use Case**: JavaScript/TypeScript actions, npm packages
- **Base OS**: Node 20 slim
- **Size**: ~760MB
- **Includes**: npm, yarn

#### 4. Full Worker
- **Image**: `attune-worker:full`
- **Capabilities**: `["shell", "python", "node", "native"]`
- **Use Case**: General-purpose automation, multi-language packs
- **Base OS**: Debian
- **Size**: ~1.6GB
- **Includes**: Python, Node.js, build tools

## Implementation

### 1. Worker Registration Enhancement

**File**: `crates/worker/src/registration.rs`

Update the `WorkerRegistration::new()` method to:

1. Read `ATTUNE_WORKER_RUNTIMES` environment variable
2. Parse comma-separated runtime list
3. Fall back to config-based capabilities
4. Auto-detect if neither is set

```rust
fn detect_capabilities(config: &Config) -> HashMap<String, serde_json::Value> {
    let mut capabilities = HashMap::new();
    
    // 1. Try environment variable first
    if let Ok(runtimes_env) = std::env::var("ATTUNE_WORKER_RUNTIMES") {
        let runtimes: Vec<String> = runtimes_env
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        capabilities.insert("runtimes".to_string(), json!(runtimes));
    }
    // 2. Try config file
    else if let Some(ref worker_config) = config.worker {
        if let Some(ref config_caps) = worker_config.capabilities {
            capabilities = config_caps.clone();
        }
    }
    // 3. Auto-detect
    else {
        let runtimes = auto_detect_runtimes();
        capabilities.insert("runtimes".to_string(), json!(runtimes));
    }
    
    capabilities
}

fn auto_detect_runtimes() -> Vec<String> {
    let mut runtimes = vec!["shell".to_string()]; // Always support shell
    
    // Check for Python
    if Command::new("python3").arg("--version").output().is_ok() {
        runtimes.push("python".to_string());
    }
    
    // Check for Node.js
    if Command::new("node").arg("--version").output().is_ok() {
        runtimes.push("node".to_string());
    }
    
    runtimes
}
```

### 2. Worker Dockerfiles

#### Base Worker Dockerfile

**File**: `docker/Dockerfile.worker.base`

```dockerfile
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    bash \
    && rm -rf /var/lib/apt/lists/*

# Create worker user
RUN useradd -m -u 1000 attune && \
    mkdir -p /opt/attune/packs /opt/attune/logs && \
    chown -R attune:attune /opt/attune

WORKDIR /opt/attune

# Copy worker binary (built separately)
COPY --from=builder /build/attune-worker /usr/local/bin/attune-worker

# Copy configuration
COPY config.docker.yaml ./config.yaml

USER attune

ENV ATTUNE_WORKER_RUNTIMES="shell"
ENV RUST_LOG=info

CMD ["/usr/local/bin/attune-worker"]
```

#### Python Worker Dockerfile

**File**: `docker/Dockerfile.worker.python`

```dockerfile
FROM python:3.11-slim-bookworm

# Install system dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Install common Python packages
RUN pip install --no-cache-dir \
    requests \
    pyyaml \
    jinja2 \
    python-dateutil

# Create worker user
RUN useradd -m -u 1000 attune && \
    mkdir -p /opt/attune/packs /opt/attune/logs && \
    chown -R attune:attune /opt/attune

WORKDIR /opt/attune

# Copy worker binary
COPY --from=builder /build/attune-worker /usr/local/bin/attune-worker

# Copy configuration
COPY config.docker.yaml ./config.yaml

USER attune

ENV ATTUNE_WORKER_RUNTIMES="shell,python"
ENV RUST_LOG=info

CMD ["/usr/local/bin/attune-worker"]
```

#### Full Worker Dockerfile

**File**: `docker/Dockerfile.worker.full`

```dockerfile
FROM debian:bookworm

# Install system dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    build-essential \
    python3 \
    python3-pip \
    python3-venv \
    nodejs \
    npm \
    && rm -rf /var/lib/apt/lists/*

# Create symlink for python
RUN ln -s /usr/bin/python3 /usr/bin/python

# Install common Python packages
RUN pip3 install --no-cache-dir --break-system-packages \
    requests \
    pyyaml \
    jinja2 \
    python-dateutil

# Create worker user
RUN useradd -m -u 1000 attune && \
    mkdir -p /opt/attune/packs /opt/attune/logs && \
    chown -R attune:attune /opt/attune

WORKDIR /opt/attune

# Copy worker binary
COPY --from=builder /build/attune-worker /usr/local/bin/attune-worker

# Copy configuration
COPY config.docker.yaml ./config.yaml

USER attune

ENV ATTUNE_WORKER_RUNTIMES="shell,python,node,native"
ENV RUST_LOG=info

CMD ["/usr/local/bin/attune-worker"]
```

### 3. Multi-stage Build Support

**File**: `docker/Dockerfile.worker`

```dockerfile
# Multi-stage Dockerfile for Attune workers
# Supports building different worker variants with different runtime capabilities
# Usage:
#   docker build --target worker-base -t attune-worker:base .
#   docker build --target worker-python -t attune-worker:python .
#   docker build --target worker-full -t attune-worker:full .

ARG RUST_VERSION=1.98.1
ARG DEBIAN_VERSION=bookworm

# ============================================================================
# Stage 1: Builder - Compile the worker binary
# ============================================================================
FROM rust:${RUST_VERSION}-${DEBIAN_VERSION} AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY migrations/ ./migrations/
COPY .sqlx/ ./.sqlx/

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo build --release --bin attune-worker && \
    cp /build/target/release/attune-worker /build/attune-worker

# ============================================================================
# Stage 2a: Base Worker (Shell only)
# ============================================================================
FROM debian:${DEBIAN_VERSION}-slim AS worker-base

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    bash \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -u 1000 attune && \
    mkdir -p /opt/attune/packs /opt/attune/logs && \
    chown -R attune:attune /opt/attune

WORKDIR /opt/attune

COPY --from=builder /build/attune-worker /usr/local/bin/attune-worker
COPY config.docker.yaml ./config.yaml
COPY packs/ ./packs/

RUN chown -R attune:attune /opt/attune

USER attune

ENV ATTUNE_WORKER_RUNTIMES="shell"
ENV ATTUNE_WORKER_TYPE="container"
ENV RUST_LOG=info
ENV ATTUNE_CONFIG=/opt/attune/config.yaml

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD pgrep -f attune-worker || exit 1

CMD ["/usr/local/bin/attune-worker"]

# ============================================================================
# Stage 2b: Python Worker (Shell + Python)
# ============================================================================
FROM python:3.11-slim-${DEBIAN_VERSION} AS worker-python

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

RUN pip install --no-cache-dir \
    requests \
    pyyaml \
    jinja2 \
    python-dateutil

RUN useradd -m -u 1000 attune && \
    mkdir -p /opt/attune/packs /opt/attune/logs && \
    chown -R attune:attune /opt/attune

WORKDIR /opt/attune

COPY --from=builder /build/attune-worker /usr/local/bin/attune-worker
COPY config.docker.yaml ./config.yaml
COPY packs/ ./packs/

RUN chown -R attune:attune /opt/attune

USER attune

ENV ATTUNE_WORKER_RUNTIMES="shell,python"
ENV ATTUNE_WORKER_TYPE="container"
ENV RUST_LOG=info
ENV ATTUNE_CONFIG=/opt/attune/config.yaml

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD pgrep -f attune-worker || exit 1

CMD ["/usr/local/bin/attune-worker"]

# ============================================================================
# Stage 2c: Node Worker (Shell + Node.js)
# ============================================================================
FROM node:20-slim AS worker-node

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -u 1000 attune && \
    mkdir -p /opt/attune/packs /opt/attune/logs && \
    chown -R attune:attune /opt/attune

WORKDIR /opt/attune

COPY --from=builder /build/attune-worker /usr/local/bin/attune-worker
COPY config.docker.yaml ./config.yaml
COPY packs/ ./packs/

RUN chown -R attune:attune /opt/attune

USER attune

ENV ATTUNE_WORKER_RUNTIMES="shell,node"
ENV ATTUNE_WORKER_TYPE="container"
ENV RUST_LOG=info
ENV ATTUNE_CONFIG=/opt/attune/config.yaml

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD pgrep -f attune-worker || exit 1

CMD ["/usr/local/bin/attune-worker"]

# ============================================================================
# Stage 2d: Full Worker (All runtimes)
# ============================================================================
FROM debian:${DEBIAN_VERSION} AS worker-full

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    build-essential \
    python3 \
    python3-pip \
    python3-venv \
    nodejs \
    npm \
    && rm -rf /var/lib/apt/lists/*

RUN ln -s /usr/bin/python3 /usr/bin/python

RUN pip3 install --no-cache-dir --break-system-packages \
    requests \
    pyyaml \
    jinja2 \
    python-dateutil

RUN useradd -m -u 1000 attune && \
    mkdir -p /opt/attune/packs /opt/attune/logs && \
    chown -R attune:attune /opt/attune

WORKDIR /opt/attune

COPY --from=builder /build/attune-worker /usr/local/bin/attune-worker
COPY config.docker.yaml ./config.yaml
COPY packs/ ./packs/

RUN chown -R attune:attune /opt/attune

USER attune

ENV ATTUNE_WORKER_RUNTIMES="shell,python,node,native"
ENV ATTUNE_WORKER_TYPE="container"
ENV RUST_LOG=info
ENV ATTUNE_CONFIG=/opt/attune/config.yaml

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD pgrep -f attune-worker || exit 1

CMD ["/usr/local/bin/attune-worker"]
```

### 4. Docker Compose Integration

**File**: `docker-compose.yaml` (update workers section)

```yaml
services:
  # Base shell worker
  worker-shell:
    build:
      context: .
      dockerfile: docker/Dockerfile.worker
      target: worker-base
    image: attune-worker:base
    container_name: attune-worker-shell
    environment:
      - ATTUNE_WORKER_RUNTIMES=shell
      - ATTUNE_WORKER_TYPE=container
      - ATTUNE_WORKER_NAME=worker-shell-01
      - ATTUNE__DATABASE__URL=postgres://attune:attune@postgres:5432/attune
      - ATTUNE__RABBITMQ__URL=amqp://attune:attune@rabbitmq:5672
      - RUST_LOG=info
    volumes:
      - ./packs:/opt/attune/packs:ro
      - worker-shell-logs:/opt/attune/logs
    depends_on:
      postgres:
        condition: service_healthy
      rabbitmq:
        condition: service_healthy
    restart: unless-stopped
    networks:
      - attune

  # Python worker
  worker-python:
    build:
      context: .
      dockerfile: docker/Dockerfile.worker
      target: worker-python
    image: attune-worker:python
    container_name: attune-worker-python
    environment:
      - ATTUNE_WORKER_RUNTIMES=shell,python
      - ATTUNE_WORKER_TYPE=container
      - ATTUNE_WORKER_NAME=worker-python-01
      - ATTUNE__DATABASE__URL=postgres://attune:attune@postgres:5432/attune
      - ATTUNE__RABBITMQ__URL=amqp://attune:attune@rabbitmq:5672
      - RUST_LOG=info
    volumes:
      - ./packs:/opt/attune/packs:ro
      - worker-python-logs:/opt/attune/logs
    depends_on:
      postgres:
        condition: service_healthy
      rabbitmq:
        condition: service_healthy
    restart: unless-stopped
    networks:
      - attune

  # Full-featured worker
  worker-full:
    build:
      context: .
      dockerfile: docker/Dockerfile.worker
      target: worker-full
    image: attune-worker:full
    container_name: attune-worker-full
    environment:
      - ATTUNE_WORKER_RUNTIMES=shell,python,node,native
      - ATTUNE_WORKER_TYPE=container
      - ATTUNE_WORKER_NAME=worker-full-01
      - ATTUNE__DATABASE__URL=postgres://attune:attune@postgres:5432/attune
      - ATTUNE__RABBITMQ__URL=amqp://attune:attune@rabbitmq:5672
      - RUST_LOG=info
    volumes:
      - ./packs:/opt/attune/packs:ro
      - worker-full-logs:/opt/attune/logs
    depends_on:
      postgres:
        condition: service_healthy
      rabbitmq:
        condition: service_healthy
    restart: unless-stopped
    networks:
      - attune

volumes:
  worker-shell-logs:
  worker-python-logs:
  worker-full-logs:
```

### 5. Configuration

**File**: `config.docker.yaml` (worker section)

```yaml
worker:
  # Name will be set via environment variable ATTUNE_WORKER_NAME
  # Worker type will be set via environment variable ATTUNE_WORKER_TYPE
  # Runtimes will be detected from ATTUNE_WORKER_RUNTIMES environment variable
  
  max_concurrent_tasks: 10
  heartbeat_interval: 30
  task_timeout: 300
  max_stdout_bytes: 10485760  # 10MB
  max_stderr_bytes: 10485760  # 10MB
  stream_logs: true
```

## Usage

### Building Worker Images

```bash
# Build all worker variants
docker build --target worker-base -t attune-worker:base -f docker/Dockerfile.worker .
docker build --target worker-python -t attune-worker:python -f docker/Dockerfile.worker .
docker build --target worker-node -t attune-worker:node -f docker/Dockerfile.worker .
docker build --target worker-full -t attune-worker:full -f docker/Dockerfile.worker .

# Or use docker-compose
docker-compose build worker-shell worker-python worker-full
```

### Running Workers

```bash
# Start all workers
docker-compose up -d worker-shell worker-python worker-full

# Start specific worker type
docker-compose up -d worker-python

# Scale workers
docker-compose up -d --scale worker-python=3
```

### Custom Runtime Configuration

To run a worker with custom runtime capabilities:

```bash
docker run -d \
  --name my-custom-worker \
  -e ATTUNE_WORKER_RUNTIMES="shell,python,custom" \
  -e ATTUNE_WORKER_NAME="custom-worker-01" \
  -e ATTUNE__DATABASE__URL="postgres://..." \
  -e ATTUNE__RABBITMQ__URL="amqp://..." \
  -v /path/to/packs:/opt/attune/packs:ro \
  attune-worker:python
```

## Scheduling Behavior

When the executor schedules an action:

1. **Action Runtime Check**: Read the action's `runtime` field
2. **Runtime Lookup**: Query the `runtime` table for runtime details
3. **Worker Selection**: Find workers with matching runtime in `capabilities.runtimes`
4. **Filter Active**: Only consider workers with `status = 'active'`
5. **Load Balancing**: Select based on capacity (TODO: improve algorithm)

Example:
- Action requires `runtime: "python"`
- System finds workers with `"python"` in their `capabilities.runtimes` array
- Only `worker-python` and `worker-full` are eligible
- One is selected based on availability

## Security Considerations

### 1. Non-root Execution
All workers run as non-root user `attune` (UID 1000)

### 2. Read-only Packs
Pack files are mounted read-only to prevent modification

### 3. Resource Limits
Set CPU and memory limits in docker-compose:

```yaml
worker-python:
  deploy:
    resources:
      limits:
        cpus: '2'
        memory: 2G
      reservations:
        cpus: '0.5'
        memory: 512M
```

### 4. Network Isolation
Workers run in isolated network with only necessary service access

## Pack Dependencies

For Python packs with dependencies:

### Option 1: Pre-install in Worker Image
Extend the worker Dockerfile:

```dockerfile
FROM attune-worker:python

USER root
RUN pip install --no-cache-dir \
    pandas \
    numpy \
    scikit-learn
USER attune
```

### Option 2: Pack-level Virtual Environments
Workers create virtualenvs per pack (future enhancement)

### Option 3: Container Per Pack
Advanced: Each pack runs in its own container (future enhancement)

## Monitoring

### Worker Registration
Workers automatically register on startup and send heartbeats every 30 seconds.

Check registered workers:
```bash
# Via API
curl http://localhost:8080/api/v1/workers

# Via database
psql -U attune -d attune -c "SELECT name, worker_type, status, capabilities FROM worker;"
```

### Worker Health
```bash
# Check container health
docker ps --filter name=worker

# View logs
docker logs attune-worker-python

# Check heartbeat
docker exec attune-worker-python ps aux | grep attune-worker
```

## Troubleshooting

### Worker Not Registering

1. **Check Database Connection**:
   ```bash
   docker logs attune-worker-python | grep database
   ```

2. **Verify Environment Variables**:
   ```bash
   docker exec attune-worker-python env | grep ATTUNE
   ```

3. **Check RabbitMQ Connection**:
   ```bash
   docker logs attune-worker-python | grep rabbitmq
   ```

### Actions Not Scheduled to Worker

1. **Verify Runtime Match**:
   - Action's runtime must match worker's capabilities
   - Check case-sensitivity (matching is case-insensitive)

2. **Check Worker Status**:
   ```sql
   SELECT name, status, capabilities FROM worker WHERE name = 'worker-python-01';
   ```

3. **Verify Worker is Active**:
   ```sql
   UPDATE worker SET status = 'active' WHERE name = 'worker-python-01';
   ```

## Future Enhancements

1. **Dynamic Runtime Detection**: Auto-detect available runtimes in container
2. **Pack-specific Workers**: Workers dedicated to specific packs
3. **GPU Support**: Workers with GPU access for ML workloads
4. **Custom Runtime Registration**: Allow packs to define custom runtimes
5. **Worker Pools**: Group workers by capability sets
6. **Auto-scaling**: Scale workers based on queue depth
7. **Pack Isolation**: Run each pack in isolated container

## References

- Worker Service Implementation: `crates/worker/src/service.rs`
- Worker Registration: `crates/worker/src/registration.rs`
- Scheduler Logic: `crates/executor/src/scheduler.rs`
- Runtime Model: `crates/common/src/models/runtime.rs`
- Configuration: `docs/configuration.md`
