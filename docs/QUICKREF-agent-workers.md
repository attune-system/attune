# Quick Reference: Agent-Based Workers

> **TL;DR**: Inject the `attune-agent` binary into _any_ container image to turn it into an Attune worker. No Dockerfiles. No Rust compilation. ~12 lines of YAML.

## ⚠️ Required: Bootstrap Token

The API endpoint that serves the agent binary (`GET /api/v1/agent/binary`) **requires** a bootstrap token. The API will refuse to start if `agent.binary_dir` is configured without a corresponding `agent.bootstrap_token`, and any download request without (or with an invalid) token is rejected with HTTP 503/401.

```bash
# 1. Generate a strong token
openssl rand -hex 32

# 2. Pass it to the API (config.docker.yaml or env)
#    config.docker.yaml:
#      agent:
#        binary_dir: /opt/attune/agent
#        bootstrap_token: "replace-with-literal-token"
#
#    .env (or shell):
export AGENT_BOOTSTRAP_TOKEN=<paste-token-here>

# 3. Pass the same token to every agent worker as ATTUNE_AGENT_TOKEN
#    so attune-agent-wrapper.sh can authenticate when downloading the binary.
```

Raw Attune YAML does not interpolate `${TOKEN}` placeholders. Set the literal
value securely or use the corresponding `ATTUNE__...` environment override.

If your agent workers mount the `agent_bin` volume directly (the default in `docker-compose.agent.yaml`), they don't need the token — the volume mount bypasses the API endpoint. The token is only required when bootstrapping over HTTP via `attune-agent-wrapper.sh`.

## How It Works

1. The `init-agent` service (in `docker-compose.yaml`) builds the statically-linked `attune-agent` binary and copies it into the `agent_bin` volume
2. Your worker service mounts `agent_bin` read-only and uses the agent as its entrypoint
3. On startup, the agent auto-detects available runtimes (Python, Ruby, Node.js, Shell, etc.)
4. The worker registers with Attune and starts processing executions

## Quick Start

### Option A: Use the override file

```bash
# Start all services including the example Ruby agent worker
docker compose -f docker-compose.yaml -f docker-compose.agent.yaml up -d
```

The `docker-compose.agent.yaml` file includes a ready-to-use Ruby worker and commented-out templates for Python 3.12, GPU, and custom images.

### Option B: Add to docker-compose.override.yaml

Create a `docker-compose.override.yaml` in the project root:

```yaml
services:
  worker-my-runtime:
    image: my-org/my-custom-image:latest
    container_name: attune-worker-my-runtime
    depends_on:
      init-agent:
        condition: service_completed_successfully
      init-packs:
        condition: service_completed_successfully
      migrations:
        condition: service_completed_successfully
      postgres:
        condition: service_healthy
      rabbitmq:
        condition: service_healthy
    entrypoint: ["/opt/attune/agent/attune-agent"]
    stop_grace_period: 45s
    environment:
      RUST_LOG: info
      ATTUNE_CONFIG: /opt/attune/config/config.yaml
      ATTUNE_WORKER_NAME: worker-my-runtime-01
      ATTUNE_WORKER_TYPE: container
      ATTUNE__SECURITY__JWT_SECRET: ${JWT_SECRET:-docker-dev-secret-change-in-production}
      ATTUNE__SECURITY__ENCRYPTION_KEY: ${ENCRYPTION_KEY:-docker-dev-encryption-key-please-change-in-production-32plus}
      ATTUNE__DATABASE__URL: postgresql://attune:attune@postgres:5432/attune
      ATTUNE__MESSAGE_QUEUE__URL: amqp://attune:attune@rabbitmq:5672
      ATTUNE_API_URL: http://attune-api:8080
    volumes:
      - agent_bin:/opt/attune/agent:ro
      - ${ATTUNE_DOCKER_CONFIG_PATH:-./config.docker.yaml}:/opt/attune/config/config.yaml:ro
      - packs_data:/opt/attune/packs:ro
      - runtime_envs:/opt/attune/runtime_envs
      - artifacts_data:/opt/attune/artifacts
    healthcheck:
      test: ["CMD-SHELL", "pgrep -f attune-agent || exit 1"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 20s
    networks:
      - attune-network
    restart: unless-stopped
```

Then run:

```bash
docker compose up -d
```

Docker Compose automatically merges `docker-compose.override.yaml`.

## Required Volumes

Every agent worker needs these volumes:

| Volume | Mount Path | Mode | Purpose |
|--------|-----------|------|---------|
| `agent_bin` | `/opt/attune/agent` | `ro` | The statically-linked agent binary |
| `packs_data` | `/opt/attune/packs` | `ro` | Pack files (actions, workflows, etc.) |
| `runtime_envs` | `/opt/attune/runtime_envs` | `rw` | Isolated runtime environments (venvs, node_modules) |
| `artifacts_data` | `/opt/attune/artifacts` | `rw` | File-backed artifact storage |
| Config YAML | `/opt/attune/config/config.yaml` | `ro` | Attune configuration |

## Required Environment Variables

| Variable | Description | Example |
|----------|-------------|---------|
| `ATTUNE_CONFIG` | Path to config file inside container | `/opt/attune/config/config.yaml` |
| `ATTUNE_WORKER_NAME` | Unique worker name | `worker-ruby-01` |
| `ATTUNE_WORKER_TYPE` | Worker type | `container` |
| `ATTUNE__DATABASE__URL` | PostgreSQL connection string | `postgresql://attune:attune@postgres:5432/attune` |
| `ATTUNE__MESSAGE_QUEUE__URL` | RabbitMQ connection string | `amqp://attune:attune@rabbitmq:5672` |
| `ATTUNE__SECURITY__JWT_SECRET` | JWT signing secret | (use env var) |
| `ATTUNE__SECURITY__ENCRYPTION_KEY` | Encryption key for secrets | (use env var) |

### Optional Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `ATTUNE_WORKER_RUNTIMES` | Override auto-detection | Auto-detected |
| `ATTUNE_API_URL` | API URL for token generation | `http://attune-api:8080` |
| `RUST_LOG` | Log level | `info` |

## Runtime Auto-Detection

The agent probes for these runtimes automatically:

| Runtime | Probed Binaries |
|---------|----------------|
| Shell | `bash`, `sh` |
| Python | `python3`, `python` |
| Node.js | `node`, `nodejs` |
| Ruby | `ruby` |
| Go | `go` |
| Java | `java` |
| R | `Rscript` |
| Perl | `perl` |

To override, set `ATTUNE_WORKER_RUNTIMES`:

```yaml
environment:
  ATTUNE_WORKER_RUNTIMES: python,shell  # Only advertise Python and Shell
```

## Testing Detection

Run the agent in detect-only mode to see what it finds:

```bash
# In a running container
docker exec <container> /opt/attune/agent/attune-agent --detect-only

# Or start a throwaway container
docker run --rm -v agent_bin:/opt/attune/agent:ro ruby:3.3-slim /opt/attune/agent/attune-agent --detect-only
```

## Examples

### Ruby Worker
```yaml
worker-ruby:
  image: ruby:3.3-slim
  entrypoint: ["/opt/attune/agent/attune-agent"]
  # ... (standard depends_on, volumes, env, networks)
```

### Node.js 22 Worker
```yaml
worker-node22:
  image: node:22-slim
  entrypoint: ["/opt/attune/agent/attune-agent"]
  # ...
```

### GPU Worker (NVIDIA CUDA)
```yaml
worker-gpu:
  image: nvidia/cuda:12.3.1-runtime-ubuntu22.04
  runtime: nvidia
  entrypoint: ["/opt/attune/agent/attune-agent"]
  environment:
    ATTUNE_WORKER_RUNTIMES: python,shell  # Override — CUDA image has python
  # ...
```

### Multi-Runtime Custom Image
```yaml
worker-data-science:
  image: my-org/data-science:latest  # Has Python, R, and Julia
  entrypoint: ["/opt/attune/agent/attune-agent"]
  # Agent auto-detects all available runtimes
  # ...
```

## Comparison: Traditional vs Agent Workers

| Aspect | Traditional Worker | Agent Worker |
|--------|-------------------|--------------|
| Docker build | Required (5+ min) | None |
| Dockerfile | Custom per runtime | Not needed |
| Base image | `debian:bookworm-slim` | Any image |
| Runtime install | Via apt/NodeSource | Pre-installed in image |
| Configuration | Manual `ATTUNE_WORKER_RUNTIMES` | Auto-detected |
| Binary | Compiled into image | Injected via volume |
| Update cycle | Rebuild image | Restart `init-agent` |

## Troubleshooting

### Agent binary not found
```
exec /opt/attune/agent/attune-agent: no such file or directory
```
The `init-agent` service hasn't completed. Check:
```bash
docker compose logs init-agent
```

### "No runtimes detected"
The container image doesn't have any recognized interpreters in `$PATH`. Either:
- Use an image that includes your runtime (e.g., `ruby:3.3-slim`)
- Set `ATTUNE_WORKER_RUNTIMES` manually

### Connection refused to PostgreSQL/RabbitMQ
Ensure your `depends_on` conditions include `postgres` and `rabbitmq` health checks, and that the container is on the `attune-network`.

## See Also

- [Universal Worker Agent Plan](plans/universal-worker-agent.md) — Full architecture document
- [Docker Deployment](docker-deployment.md) — General Docker setup
- [Worker Service](architecture/worker-service.md) — Worker architecture details
