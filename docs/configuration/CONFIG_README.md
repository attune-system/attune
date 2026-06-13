# Attune Configuration Guide

This document explains how to configure Attune using YAML configuration files.

## Quick Start

1. **Copy the example configuration:**
   ```bash
   cp config.example.yaml config.yaml
   ```

2. **Edit your configuration:**
   ```bash
   nano config.yaml
   ```

3. **Generate secure secrets:**
   ```bash
   # Generate JWT secret (64 bytes, base64 encoded)
   openssl rand -base64 64

   # Generate encryption key (32 bytes minimum, base64 encoded)
   openssl rand -base64 32
   ```

4. **Run the application:**
   ```bash
   cargo run --bin attune-api
   ```

## Configuration Files

### Available Files

- **`config.example.yaml`** - Template for new installations (safe to commit)
- **`config.development.yaml`** - Development environment settings (can be committed)
- **`config.test.yaml`** - Test environment settings (can be committed)
- **`config.production.yaml`** - Production template with placeholders (safe to commit)
- **`config.yaml`** - Your actual configuration (DO NOT COMMIT - add to .gitignore)

### Loading Priority

Configuration is loaded in this order (later overrides earlier):

1. **Base config file** - `config.yaml` or path from `ATTUNE_CONFIG` environment variable
2. **Environment-specific file** - `config.{environment}.yaml` (e.g., `config.development.yaml`)
3. **Environment variables** - Variables with `ATTUNE__` prefix

**Example:**
```bash
# Use production config with secret from environment
export ATTUNE_CONFIG=config.production.yaml
export ATTUNE__SECURITY__JWT_SECRET=$(openssl rand -base64 64)
cargo run --bin attune-api
```

## Configuration Sections

### Required Settings

These settings must be configured before running:

```yaml
database:
  url: postgresql://user:password@localhost:5432/attune

security:
  jwt_secret: your-secret-key-here          # Generate with: openssl rand -base64 64
  encryption_key: your-encryption-key-here  # Generate with: openssl rand -base64 32
```

### Optional Settings

```yaml
server:
  host: 0.0.0.0
  port: 8080
  cors_origins:
    - http://localhost:3000

log:
  level: info      # trace, debug, info, warn, error
  format: json     # json, pretty

metadata_cache:    # Optional: metadata cache backed by Valkey/Redis
  enabled: true
  url: redis://localhost:6379

message_queue:     # Optional: for async processing
  url: amqp://guest:guest@localhost:5672/%2f
```

## Environment Variables

Override any setting using environment variables with the `ATTUNE__` prefix:

```bash
# Database
export ATTUNE__DATABASE__URL=postgresql://localhost/attune
export ATTUNE__DATABASE__MAX_CONNECTIONS=100

# Server
export ATTUNE__SERVER__PORT=3000
export ATTUNE__SERVER__CORS_ORIGINS="https://app.example.com,https://www.example.com"

# Security (recommended for production)
export ATTUNE__SECURITY__JWT_SECRET=$(openssl rand -base64 64)
export ATTUNE__SECURITY__ENCRYPTION_KEY=$(openssl rand -base64 32)

# Logging
export ATTUNE__LOG__LEVEL=debug
export ATTUNE__LOG__FORMAT=pretty
```

**Syntax:** `ATTUNE__<section>__<key>=value`
- Use double underscores (`__`) to separate nested keys
- For arrays, use comma-separated values
- Boolean values: `true`, `false`, `1`, `0`, `yes`, `no`

## Environment-Specific Configuration

### Development

Create or use `config.development.yaml`:

```yaml
environment: development

database:
  url: postgresql://postgres:postgres@localhost:5432/attune
  log_statements: true

log:
  level: debug
  format: pretty

security:
  jwt_access_expiration: 86400  # 24 hours for dev convenience
```

Run with:
```bash
export ATTUNE__ENVIRONMENT=development
cargo run --bin attune-api
```

### Production

Create `config.production.yaml` with placeholders:

```yaml
environment: production

database:
  url: postgresql://user@db.example.com/attune
  max_connections: 100

log:
  level: info
  format: json
  file: /var/log/attune/attune.log

server:
  cors_origins:
    - https://app.example.com
```

**IMPORTANT:** Override secrets with environment variables:
```bash
export ATTUNE_CONFIG=config.production.yaml
export ATTUNE__SECURITY__JWT_SECRET=$(cat /run/secrets/jwt_secret)
export ATTUNE__SECURITY__ENCRYPTION_KEY=$(cat /run/secrets/encryption_key)
export ATTUNE__DATABASE__URL=$(cat /run/secrets/db_url)
```

### Testing

The `config.test.yaml` is automatically used during tests:

```bash
cargo test  # Uses config.test.yaml
```

## Docker Deployment

### Option 1: Mount Configuration File

```bash
docker run -v $(pwd)/config.production.yaml:/app/config.yaml \
  -e ATTUNE__SECURITY__JWT_SECRET=$JWT_SECRET \
  attune-api
```

### Option 2: Environment Variables Only

```bash
docker run \
  -e ATTUNE__DATABASE__URL=postgresql://db/attune \
  -e ATTUNE__SECURITY__JWT_SECRET=$JWT_SECRET \
  -e ATTUNE__SECURITY__ENCRYPTION_KEY=$ENCRYPTION_KEY \
  -e ATTUNE__SERVER__PORT=8080 \
  attune-api
```

### Docker Compose

```yaml
version: '3.8'
services:
  api:
    image: attune-api
    volumes:
      - ./config.production.yaml:/app/config.yaml
    environment:
      ATTUNE__SECURITY__JWT_SECRET: ${JWT_SECRET}
      ATTUNE__SECURITY__ENCRYPTION_KEY: ${ENCRYPTION_KEY}
      ATTUNE__DATABASE__URL: postgresql://postgres:postgres@db:5432/attune
```

## Security Best Practices

### 1. Never Commit Secrets

Your `.gitignore` is already configured to exclude `config.yaml`. Always use:

- `config.example.yaml` for templates (safe to commit)
- `config.yaml` for local development (never commit)
- Environment variables for production secrets

### 2. Generate Strong Secrets

```bash
# JWT secret (64 bytes recommended)
openssl rand -base64 64

# Encryption key (32 bytes minimum)
openssl rand -base64 32

# Or use a password manager to generate and store secrets
```

### 3. Use Environment Variables in Production

```bash
# Good: Load from secure secret store
export ATTUNE__SECURITY__JWT_SECRET=$(vault read -field=jwt_secret secret/attune)

# Bad: Hardcode in config file (especially if committed)
# config.yaml:
# security:
#   jwt_secret: hardcoded-secret-in-git  # ❌ NEVER DO THIS
```

### 4. Restrict File Permissions

```bash
chmod 600 config.yaml
chown appuser:appuser config.yaml
```

## Troubleshooting

### Configuration Not Loading

**Problem:** Application can't find `config.yaml`

**Solution:**
```bash
# Check file exists
ls -la config.yaml

# Or specify path explicitly
export ATTUNE_CONFIG=/path/to/config.yaml
cargo run --bin attune-api
```

### YAML Syntax Error

**Problem:** Configuration fails to parse

**Solution:**
```bash
# Validate YAML syntax
python3 -c "import yaml; yaml.safe_load(open('config.yaml'))"

# Or use yq
yq eval config.yaml
```

**Common issues:**
- Use spaces, not tabs for indentation
- Ensure consistent indentation (2 spaces recommended)
- Quote strings with special characters: `:`, `#`, `@`, `%`

### Environment Variables Not Working

**Problem:** Environment variables not overriding config

**Solution:**
```bash
# Check variable is set
env | grep ATTUNE

# Ensure correct format
export ATTUNE__DATABASE__URL=...  # ✅ Correct (double underscore)
export ATTUNE_DATABASE_URL=...     # ❌ Wrong (single underscore)
```

### Validation Errors

**Problem:** "JWT secret is required when authentication is enabled"

**Solution:**
```yaml
# Either disable auth for development
security:
  enable_auth: false

# Or provide a secret
security:
  enable_auth: true
  jwt_secret: your-secret-here
```

## Complete Configuration Reference

See [docs/configuration.md](docs/configuration.md) for complete documentation of all configuration options.

## Migration from .env

If you're migrating from `.env` files, see [docs/env-to-yaml-migration.md](docs/env-to-yaml-migration.md) for a complete migration guide.

## Getting Help

- **Configuration Guide:** [docs/configuration.md](docs/configuration.md)
- **Quick Start:** [docs/quick-start.md](docs/quick-start.md)
- **Migration Guide:** [docs/env-to-yaml-migration.md](docs/env-to-yaml-migration.md)
- **README:** [README.md](README.md)