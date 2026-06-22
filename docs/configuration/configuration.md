# Configuration Guide

This guide explains how to configure the Attune automation platform using YAML configuration files.

## Overview

Attune uses a layered configuration system with YAML files as the primary configuration source. Configuration can be overridden using environment variables, making it flexible for different deployment scenarios.

## Configuration Loading Priority

Configuration is loaded in the following order (later sources override earlier ones):

1. **Base configuration file** (`config.yaml` or path from `ATTUNE_CONFIG` environment variable)
2. **Environment-specific configuration** (e.g., `config.development.yaml`, `config.production.yaml`)
3. **Environment variables** (prefix: `ATTUNE__`, separator: `__`)

This layered approach allows you to:
- Keep common settings in `config.yaml`
- Override settings per environment (dev, test, production)
- Override sensitive values with environment variables (recommended for production)

## Quick Start

1. **Copy the example configuration:**
   ```bash
   cp config.example.yaml config.yaml
   ```

2. **Edit the configuration:**
   ```bash
   nano config.yaml
   ```

3. **Set required values:**
   - Database URL
   - JWT secret key
   - Encryption key
   - CORS origins (if applicable)

4. **Run the application:**
   ```bash
   cargo run --bin attune-api
   ```

## Configuration File Structure

### Basic Example

```yaml
service_name: attune
environment: development

database:
  url: postgresql://postgres:postgres@localhost:5432/attune
  max_connections: 50

server:
  host: 0.0.0.0
  port: 8080
  cors_origins:
    - http://localhost:3000

security:
  jwt_secret: your-secret-key-here
  jwt_access_expiration: 3600

log:
  level: info
  format: json
```

## Configuration Sections

### Service Metadata

```yaml
service_name: attune      # Service identifier
environment: development  # Environment: development, test, staging, production
```

### Database Configuration

```yaml
database:
  # PostgreSQL connection URL
  url: postgresql://username:password@host:port/database
  
  # Connection pool settings
  max_connections: 50      # Maximum pool size
  min_connections: 5       # Minimum pool size
  connect_timeout: 30      # Connection timeout (seconds)
  idle_timeout: 600        # Idle connection timeout (seconds)
  
  # Debugging
  log_statements: false    # Log SQL statements (set true for debugging)
```

**Environment variable override:**
```bash
export ATTUNE__DATABASE__URL=postgresql://user:pass@localhost/attune
export ATTUNE__DATABASE__MAX_CONNECTIONS=100
```

### Server Configuration

```yaml
server:
  host: 0.0.0.0           # Bind address (0.0.0.0 for all interfaces)
  port: 8080              # HTTP port
  request_timeout: 30     # Request timeout (seconds)
  enable_cors: true       # Enable CORS middleware
  
  # Allowed CORS origins
  cors_origins:
    - http://localhost:3000
    - https://app.example.com
  
  max_body_size: 10485760  # Max request body size (10MB in bytes)
```

**Environment variable override:**
```bash
export ATTUNE__SERVER__PORT=3000
export ATTUNE__SERVER__CORS_ORIGINS="https://app.com,https://www.app.com"
```

### Security Configuration

```yaml
security:
  # JWT secret for signing tokens (REQUIRED if enable_auth is true)
  # Generate with: openssl rand -base64 64
  jwt_secret: your-secret-key-here
  
  # Token expiration times (seconds)
  jwt_access_expiration: 3600      # Access token: 1 hour
  jwt_refresh_expiration: 604800   # Refresh token: 7 days
  
  # Encryption key for storing secrets (must be at least 32 characters)
  # Generate with: openssl rand -base64 32
  encryption_key: your-32-char-encryption-key-here
  
  # Enable/disable authentication
  enable_auth: true
```

**Environment variable override (recommended for production):**
```bash
export ATTUNE__SECURITY__JWT_SECRET=$(openssl rand -base64 64)
export ATTUNE__SECURITY__ENCRYPTION_KEY=$(openssl rand -base64 32)
```

### Logging Configuration

```yaml
log:
  # Log level: trace, debug, info, warn, error
  level: info
  
  # Log format controls stdout rendering: json (structured), pretty (human-readable)
  format: json
  
  # Reserved/no-op today; services currently always log to stdout
  console: true
  
  # Reserved for a future file sink; not currently implemented
  # file: /var/log/attune/attune.log
```

**Environment variable override:**
```bash
export ATTUNE__LOG__LEVEL=debug
export ATTUNE__LOG__FORMAT=pretty
```

Current runtime behavior:
- Docker/distributable configs intentionally default to `format: json`
- `config.development.yaml` intentionally uses `format: pretty`
- `log.console` and `log.file` are retained in schema/config examples, but do not currently switch sinks; services log to stdout today

### Redis Configuration (Optional)

```yaml
redis:
  url: redis://localhost:6379
  pool_size: 10
```

### Message Queue Configuration (Optional)

```yaml
message_queue:
  url: amqp://guest:guest@localhost:5672/%2f
  exchange: attune
  enable_dlq: true        # Enable dead letter queue
  message_ttl: 3600       # Message TTL (seconds)
```

### Worker Configuration (Optional)

```yaml
worker:
  name: attune-worker-1
  worker_type: local      # local, remote, container
  labels:                 # optional placement labels for executor scheduling
    gpu: nvidia
    zone: us-east-1a
  taints:                 # optional taints; actions need matching tolerations
    - key: gpu
      value: "true"
      effect: no_schedule
  max_concurrent_tasks: 10
  heartbeat_interval: 30  # seconds
  task_timeout: 300       # seconds

  # Per-execution action stdout/stderr log artifact retention.
  # Policy can be: versions, days, hours, or minutes.
  execution_log_retention_policy: days
  execution_log_retention_limit: 7
```

Pack/API-created actions and sensors can override log artifact retention per row with nullable `log_retention_policy` and `log_retention_limit` fields. Action logs default to `days` / `7`; sensor logs default to `versions` / `4` because sensor artifact versions are created only when the rotating log segment exceeds its size limit.

## Environment-Specific Configuration

### Development Environment

Create `config.development.yaml`:

```yaml
environment: development

database:
  url: postgresql://postgres:postgres@localhost:5432/attune
  log_statements: true

server:
  host: 127.0.0.1
  cors_origins:
    - http://localhost:3000
    - http://localhost:5173

log:
  level: debug
  format: pretty

security:
  jwt_secret: dev-secret-not-for-production
  jwt_access_expiration: 86400  # 24 hours for dev convenience
```

### Production Environment

Create `config.production.yaml`:

```yaml
environment: production

database:
  url: postgresql://attune_user:CHANGE_ME@db.example.com/attune
  max_connections: 100
  log_statements: false

server:
  cors_origins:
    - https://app.example.com
    - https://www.example.com

log:
  level: info
  format: json
  # file: /var/log/attune/attune.log  # Reserved; file sink not implemented yet

security:
  # Override with environment variables!
  jwt_secret: CHANGE_ME_USE_ENV_VAR
  encryption_key: CHANGE_ME_USE_ENV_VAR
```

**Important:** Always override sensitive values in production using environment variables:

```bash
export ATTUNE__SECURITY__JWT_SECRET=$(openssl rand -base64 64)
export ATTUNE__SECURITY__ENCRYPTION_KEY=$(openssl rand -base64 32)
export ATTUNE__DATABASE__URL=postgresql://user:pass@db.example.com/attune
```

### Test Environment

Create `config.test.yaml`:

```yaml
environment: test

database:
  url: postgresql://postgres:postgres@localhost:5432/attune_test
  max_connections: 10

server:
  port: 0  # Random port for tests

log:
  level: warn
  format: pretty

security:
  jwt_secret: test-secret-not-secure
  jwt_access_expiration: 300  # 5 minutes
```

## Environment Variables

You can override any configuration value using environment variables with the `ATTUNE__` prefix and `__` separator for nested keys.

### Syntax

```
ATTUNE__<section>__<key>=value
```

### Examples

```bash
# Database
export ATTUNE__DATABASE__URL=postgresql://localhost/attune
export ATTUNE__DATABASE__MAX_CONNECTIONS=100

# Server
export ATTUNE__SERVER__PORT=8080
export ATTUNE__SERVER__HOST=0.0.0.0

# Security
export ATTUNE__SECURITY__JWT_SECRET=my-secret-key
export ATTUNE__SECURITY__ENABLE_AUTH=true

# Logging
export ATTUNE__LOG__LEVEL=debug
export ATTUNE__LOG__FORMAT=pretty

# Arrays (comma-separated)
export ATTUNE__SERVER__CORS_ORIGINS="https://app.com,https://www.app.com"
```

### Boolean Values

Boolean values can be set using:
- `true`, `1`, `yes`, `on` → true
- `false`, `0`, `no`, `off` → false

```bash
export ATTUNE__DATABASE__LOG_STATEMENTS=true
export ATTUNE__SECURITY__ENABLE_AUTH=false
```

## Custom Configuration File Path

By default, Attune looks for `config.yaml` in the current directory. You can specify a custom path:

```bash
export ATTUNE_CONFIG=/path/to/custom-config.yaml
cargo run --bin attune-api
```

Or:

```bash
ATTUNE_CONFIG=/etc/attune/config.yaml attune-api
```

## Configuration Validation

The application validates configuration on startup and will fail with clear error messages if:

- Required fields are missing (e.g., JWT secret when auth is enabled)
- Values are invalid (e.g., invalid log level)
- Security requirements are not met (e.g., encryption key too short)

Example validation errors:

```
Error: Configuration validation failed: JWT secret is required when authentication is enabled
Error: Invalid log level: trace. Must be one of: ["trace", "debug", "info", "warn", "error"]
Error: Encryption key must be at least 32 characters
```

## Security Best Practices

### 1. Never Commit Secrets

Add sensitive config files to `.gitignore`:

```gitignore
config.yaml
config.*.yaml
!config.example.yaml
!config.development.yaml
!config.test.yaml
```

### 2. Use Strong Secrets

Generate cryptographically secure secrets:

```bash
# JWT secret (64 bytes, base64 encoded)
openssl rand -base64 64

# Encryption key (32 bytes minimum, base64 encoded)
openssl rand -base64 32
```

### 3. Environment Variables in Production

Always use environment variables for sensitive values in production:

```bash
# In production deployment
export ATTUNE__SECURITY__JWT_SECRET=$(openssl rand -base64 64)
export ATTUNE__SECURITY__ENCRYPTION_KEY=$(openssl rand -base64 32)
export ATTUNE__DATABASE__URL=postgresql://user:${DB_PASSWORD}@db.example.com/attune
```

### 4. File Permissions

Restrict access to configuration files:

```bash
chmod 600 config.yaml
chown appuser:appuser config.yaml
```

### 5. Separate Configs Per Environment

Use environment-specific files and never share production secrets with development:

```
config.yaml                 # Base (committed)
config.example.yaml         # Example (committed)
config.development.yaml     # Development (committed, no real secrets)
config.production.yaml      # Production template (committed, placeholders only)
config.yaml                 # Actual config (NOT committed, local overrides)
```

## Docker/Container Deployments

### Option 1: Mount Configuration File

```bash
docker run -v /path/to/config.yaml:/app/config.yaml attune-api
```

### Option 2: Environment Variables (Recommended)

```bash
docker run \
  -e ATTUNE__DATABASE__URL=postgresql://db/attune \
  -e ATTUNE__SECURITY__JWT_SECRET=$(cat /run/secrets/jwt_secret) \
  -e ATTUNE__SERVER__PORT=8080 \
  attune-api
```

### Docker Compose Example

```yaml
version: '3.8'
services:
  api:
    image: attune-api
    environment:
      ATTUNE__DATABASE__URL: postgresql://postgres:postgres@db:5432/attune
      ATTUNE__SERVER__PORT: 8080
      ATTUNE__LOG__LEVEL: info
    secrets:
      - jwt_secret
      - encryption_key
    command: sh -c "
      export ATTUNE__SECURITY__JWT_SECRET=$$(cat /run/secrets/jwt_secret) &&
      export ATTUNE__SECURITY__ENCRYPTION_KEY=$$(cat /run/secrets/encryption_key) &&
      attune-api
    "

secrets:
  jwt_secret:
    file: ./secrets/jwt_secret.txt
  encryption_key:
    file: ./secrets/encryption_key.txt
```

## Kubernetes Deployments

### ConfigMap for Base Configuration

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: attune-config
data:
  config.yaml: |
    service_name: attune
    environment: production
    server:
      host: 0.0.0.0
      port: 8080
    log:
      level: info
      format: json
```

### Secrets for Sensitive Values

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: attune-secrets
type: Opaque
stringData:
  jwt-secret: "your-base64-jwt-secret"
  encryption-key: "your-base64-encryption-key"
  database-url: "postgresql://user:pass@db/attune"
```

### Deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: attune-api
spec:
  template:
    spec:
      containers:
      - name: api
        image: attune-api:latest
        env:
        - name: ATTUNE__SECURITY__JWT_SECRET
          valueFrom:
            secretKeyRef:
              name: attune-secrets
              key: jwt-secret
        - name: ATTUNE__SECURITY__ENCRYPTION_KEY
          valueFrom:
            secretKeyRef:
              name: attune-secrets
              key: encryption-key
        - name: ATTUNE__DATABASE__URL
          valueFrom:
            secretKeyRef:
              name: attune-secrets
              key: database-url
        volumeMounts:
        - name: config
          mountPath: /app/config.yaml
          subPath: config.yaml
      volumes:
      - name: config
        configMap:
          name: attune-config
```

## Troubleshooting

### Configuration Not Loading

1. **Check file exists:**
   ```bash
   ls -la config.yaml
   ```

2. **Check YAML syntax:**
   ```bash
   # Install yq if needed: brew install yq
   yq eval config.yaml
   ```

3. **Check environment variable:**
   ```bash
   echo $ATTUNE_CONFIG
   ```

### Environment Variables Not Working

1. **Check variable name format:**
   - Must start with `ATTUNE__`
   - Use double underscores for nesting
   - Case-sensitive

2. **Verify variable is set:**
   ```bash
   env | grep ATTUNE
   ```

3. **Check for typos:**
   ```bash
   export ATTUNE__DATABASE__URL=...  # Correct
   export ATTUNE_DATABASE_URL=...     # Wrong! (single underscore)
   ```

### Validation Errors

Enable debug logging to see detailed configuration:

```bash
ATTUNE__LOG__LEVEL=debug cargo run --bin attune-api
```

## Migration from .env Files

If you're migrating from `.env` files, here's a conversion guide:

### Before (.env):
```bash
ATTUNE__DATABASE__URL=postgresql://localhost/attune
ATTUNE__SERVER__PORT=8080
ATTUNE__LOG__LEVEL=info
ATTUNE__SECURITY__JWT_SECRET=my-secret
```

### After (config.yaml):
```yaml
database:
  url: postgresql://localhost/attune

server:
  port: 8080

log:
  level: info

security:
  jwt_secret: my-secret
```

**Note:** You can still use environment variables for overrides, but the base configuration is now in YAML.

## Additional Resources

- [Quick Start Guide](quick-start.md)
- [Authentication Documentation](authentication.md)
- [API Reference](api-reference.md)
- [Deployment Guide](deployment.md)
