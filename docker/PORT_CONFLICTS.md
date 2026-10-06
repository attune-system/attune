# Docker Port Conflicts Resolution

## Problem

When starting Attune with Docker Compose, you may encounter port binding errors:

```
Error response from daemon: ports are not available: exposing port TCP 0.0.0.0:5432 -> 127.0.0.1:0: listen tcp 0.0.0.0:5432: bind: address already in use
```

This happens when **system-level services** (PostgreSQL, RabbitMQ, Redis) are already running and using the same ports that Docker containers need.

## Port Conflicts Table

| Service | Port | Docker Container | System Service |
|---------|------|------------------|----------------|
| PostgreSQL | 5432 | attune-postgres | postgresql |
| RabbitMQ (AMQP) | 5672 | attune-rabbitmq | rabbitmq-server |
| RabbitMQ (Management) | 15672 | attune-rabbitmq | rabbitmq-server |
| Redis | 6379 | attune-redis | redis-server |
| API | 8080 | attune-api | (usually free) |
| Notifier (WebSocket) | 8081 | attune-notifier | (usually free) |
| Web UI | 3000 | attune-web | (usually free) |

## Quick Fix

### Automated Script (Recommended)

Run the provided script to stop all conflicting services:

```bash
./scripts/stop-system-services.sh
```

This will:
1. Stop system PostgreSQL, RabbitMQ, and Redis services
2. Verify all ports are free
3. Clean up any orphaned Docker containers
4. Give you the option to disable services on boot

### Manual Fix

If the script doesn't work, follow these steps:

#### 1. Stop System PostgreSQL

```bash
# Check if running
systemctl is-active postgresql

# Stop it
sudo systemctl stop postgresql

# Optionally disable on boot
sudo systemctl disable postgresql
```

#### 2. Stop System RabbitMQ

```bash
# Check if running
systemctl is-active rabbitmq-server

# Stop it
sudo systemctl stop rabbitmq-server

# Optionally disable on boot
sudo systemctl disable rabbitmq-server
```

#### 3. Stop System Redis

```bash
# Check if running
systemctl is-active redis-server

# Stop it
sudo systemctl stop redis-server

# Optionally disable on boot
sudo systemctl disable redis-server
```

#### 4. Verify Ports are Free

```bash
# Check PostgreSQL port
nc -zv localhost 5432

# Check RabbitMQ port
nc -zv localhost 5672

# Check Redis port
nc -zv localhost 6379

# All should return "Connection refused" (meaning free)
```

## Finding What's Using a Port

If ports are still in use after stopping services:

```bash
# Method 1: Using lsof (most detailed)
sudo lsof -i :5432

# Method 2: Using ss
sudo ss -tulpn | grep 5432

# Method 3: Using netstat
sudo netstat -tulpn | grep 5432

# Method 4: Using fuser
sudo fuser 5432/tcp
```

## Killing a Process on a Port

```bash
# Find the process ID
PID=$(lsof -ti tcp:5432)

# Kill it
sudo kill $PID

# Force kill if needed
sudo kill -9 $PID
```

## Docker-Specific Issues

### Orphaned Containers

Sometimes Docker containers remain running after a failed `docker compose down`:

```bash
# List all containers (including stopped)
docker ps -a

# Stop and remove Attune containers
docker compose down

# Remove orphaned containers using specific ports
docker ps -q --filter "publish=5432" | xargs docker stop
docker ps -q --filter "publish=5672" | xargs docker stop
docker ps -q --filter "publish=6379" | xargs docker stop
```

### Corrupted Container in Restart Loop

If `docker ps -a` shows a container with status "Restarting (255)":

```bash
# Check logs
docker logs attune-postgres

# If you see "exec format error", the image is corrupted
docker compose down
docker rmi postgres:18-alpine
docker pull postgres:18-alpine
docker compose up -d
```

## Alternative: Change Docker Ports

If you want to keep system services running, modify `docker compose.yaml` to use different ports:

```yaml
postgres:
  ports:
    - "5433:5432"  # Map to 5433 on host instead

rabbitmq:
  ports:
    - "5673:5672"  # Map to 5673 on host instead
    - "15673:15672"

redis:
  ports:
    - "6380:6379"  # Map to 6380 on host instead
```

Then update your config files to use these new ports:

```yaml
# config.docker.yaml
database:
  url: postgresql://attune:attune@postgres:5432  # Internal still uses 5432

# But if accessing from host:
database:
  url: postgresql://attune:attune@localhost:5433  # Use external port
```

## Recommended Approach for Development

**Option 1: Use Docker Exclusively (Recommended)**

Stop all system services and use Docker for everything:

```bash
./scripts/stop-system-services.sh
docker compose up -d
```

**Pros:**
- Clean separation from system
- Easy to start/stop all services together
- Consistent with production deployment
- No port conflicts

**Cons:**
- Need to use Docker commands to access services
- Slightly more overhead

**Option 2: Use System Services**

Don't use Docker Compose, run services directly:

```bash
sudo systemctl start postgresql
sudo systemctl start rabbitmq-server
sudo systemctl start redis-server

# Then run Attune services natively
cargo run --bin attune-api
cargo run --bin attune-executor
# etc.
```

**Pros:**
- Familiar system tools
- Easier debugging with local tools
- Lower overhead

**Cons:**
- Manual service management
- Different from production
- Version mismatches possible

## Re-enabling System Services

To go back to using system services:

```bash
# Start and enable services
sudo systemctl start postgresql
sudo systemctl start rabbitmq-server
sudo systemctl start redis-server

sudo systemctl enable postgresql
sudo systemctl enable rabbitmq-server
sudo systemctl enable redis-server
```

## Troubleshooting Checklist

- [ ] Stop Docker containers: `docker compose down`
- [ ] Stop system PostgreSQL: `sudo systemctl stop postgresql`
- [ ] Stop system RabbitMQ: `sudo systemctl stop rabbitmq-server`
- [ ] Stop system Redis: `sudo systemctl stop redis-server`
- [ ] Verify port 5432 is free: `nc -zv localhost 5432`
- [ ] Verify port 5672 is free: `nc -zv localhost 5672`
- [ ] Verify port 6379 is free: `nc -zv localhost 6379`
- [ ] Check for orphaned containers: `docker ps -a | grep attune`
- [ ] Check for corrupted images: `docker logs attune-postgres`
- [ ] Start fresh: `docker compose up -d`

## Prevention

To avoid this issue in the future:

1. **Add to your shell profile** (`~/.bashrc` or `~/.zshrc`):

```bash
alias attune-docker-start='cd /path/to/attune && ./scripts/stop-system-services.sh && docker compose up -d'
alias attune-docker-stop='cd /path/to/attune && docker compose down'
alias attune-docker-logs='cd /path/to/attune && docker compose logs -f'
```

2. **Create a systemd service** to automatically stop conflicting services when starting Docker:

See `docs/deployment/systemd-setup.md` for details (if available).

3. **Use different ports** as described above to run both simultaneously.

## Summary

The most reliable approach:

```bash
# One-time setup
./scripts/stop-system-services.sh
# Answer 'y' to disable services on boot

# Then use Docker
docker compose up -d        # Start all services
docker compose logs -f      # View logs
docker compose down         # Stop all services
```

This ensures clean, reproducible environments that match production deployment.
