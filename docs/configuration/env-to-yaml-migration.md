# Migration Guide: .env to YAML Configuration

This guide helps you migrate from `.env` file configuration to the new YAML-based configuration system.

## Why Migrate to YAML?

The YAML configuration format offers several advantages:

- **Better readability**: Clear hierarchical structure
- **Comments**: Document your configuration inline
- **Complex structures**: Native support for arrays, nested objects
- **Type safety**: Automatic type conversion
- **Environment-specific configs**: Easy to maintain separate configs per environment
- **No prefix noise**: No need for `ATTUNE__` prefixes everywhere

## Quick Migration Steps

1. **Copy the example config**
   ```bash
   cp config.example.yaml config.yaml
   ```

2. **Convert your .env values** (see conversion examples below)

3. **Test the new configuration**
   ```bash
   cargo run --bin attune-api
   ```

4. **Remove old .env file** (optional, after verification)

## Conversion Reference

### Database Configuration

**Before (.env):**
```bash
ATTUNE__DATABASE__URL=postgresql://postgres:postgres@localhost:5432/attune
ATTUNE__DATABASE__MAX_CONNECTIONS=50
ATTUNE__DATABASE__MIN_CONNECTIONS=5
ATTUNE__DATABASE__CONNECT_TIMEOUT=30
ATTUNE__DATABASE__IDLE_TIMEOUT=600
ATTUNE__DATABASE__LOG_STATEMENTS=true
```

**After (config.yaml):**
```yaml
database:
  url: postgresql://postgres:postgres@localhost:5432/attune
  max_connections: 50
  min_connections: 5
  connect_timeout: 30
  idle_timeout: 600
  log_statements: true
```

### Server Configuration

**Before (.env):**
```bash
ATTUNE__SERVER__HOST=0.0.0.0
ATTUNE__SERVER__PORT=8080
ATTUNE__SERVER__REQUEST_TIMEOUT=30
ATTUNE__SERVER__ENABLE_CORS=true
ATTUNE__SERVER__CORS_ORIGINS=http://localhost:3000,http://localhost:5173
ATTUNE__SERVER__MAX_BODY_SIZE=10485760
```

**After (config.yaml):**
```yaml
server:
  host: 0.0.0.0
  port: 8080
  request_timeout: 30
  enable_cors: true
  cors_origins:
    - http://localhost:3000
    - http://localhost:5173
  max_body_size: 10485760
```

### Security Configuration

**Before (.env):**
```bash
ATTUNE__SECURITY__JWT_SECRET=your-secret-key-here
ATTUNE__SECURITY__JWT_ACCESS_EXPIRATION=3600
ATTUNE__SECURITY__JWT_REFRESH_EXPIRATION=604800
ATTUNE__SECURITY__ENCRYPTION_KEY=your-32-char-encryption-key
ATTUNE__SECURITY__ENABLE_AUTH=true
```

**After (config.yaml):**
```yaml
security:
  jwt_secret: your-secret-key-here
  jwt_access_expiration: 3600
  jwt_refresh_expiration: 604800
  encryption_key: your-32-char-encryption-key
  enable_auth: true
```

### Logging Configuration

**Before (.env):**
```bash
ATTUNE__LOG__LEVEL=info
ATTUNE__LOG__FORMAT=json
ATTUNE__LOG__CONSOLE=true
ATTUNE__LOG__FILE=/var/log/attune/attune.log
```

**After (config.yaml):**
```yaml
log:
  level: info
  format: json
  console: true
  file: /var/log/attune/attune.log
```

### Metadata Cache Configuration

**Before (.env):**
```bash
ATTUNE__METADATA_CACHE__ENABLED=true
ATTUNE__METADATA_CACHE__URL=redis://localhost:6379
ATTUNE__METADATA_CACHE__MAX_CONNECTIONS=10
```

**After (config.yaml):**
```yaml
metadata_cache:
  enabled: true
  url: redis://localhost:6379
  max_connections: 10  # Redis/Valkey connection managers per service
```

### Message Queue Configuration

**Before (.env):**
```bash
ATTUNE__MESSAGE_QUEUE__URL=amqp://guest:guest@localhost:5672/%2f
ATTUNE__MESSAGE_QUEUE__EXCHANGE=attune
ATTUNE__MESSAGE_QUEUE__ENABLE_DLQ=true
ATTUNE__MESSAGE_QUEUE__MESSAGE_TTL=3600
```

**After (config.yaml):**
```yaml
message_queue:
  url: amqp://guest:guest@localhost:5672/%2f
  exchange: attune
  enable_dlq: true
  message_ttl: 3600
```

## Automated Conversion Script

If you have a complex `.env` file, you can use this Python script to help convert it:

```python
#!/usr/bin/env python3
"""
Convert .env file to YAML configuration
Usage: python env_to_yaml.py .env > config.yaml
"""

import sys
import re

def parse_env_line(line):
    """Parse a single .env line"""
    line = line.strip()
    if not line or line.startswith('#'):
        return None, None
    
    match = re.match(r'ATTUNE__(.+?)=(.+)', line)
    if not match:
        return None, None
    
    key = match.group(1)
    value = match.group(2)
    
    # Remove quotes if present
    value = value.strip('"').strip("'")
    
    # Convert boolean strings
    if value.lower() in ('true', 'false'):
        value = value.lower()
    # Try to convert to number
    elif value.isdigit():
        value = int(value)
    
    return key, value

def build_yaml_structure(env_vars):
    """Build nested YAML structure from flat env vars"""
    structure = {}
    
    for key, value in env_vars.items():
        parts = key.lower().split('__')
        
        # Navigate/create nested structure
        current = structure
        for part in parts[:-1]:
            if part not in current:
                current[part] = {}
            current = current[part]
        
        # Handle special cases (arrays)
        final_key = parts[-1]
        if isinstance(value, str) and ',' in value:
            # Assume comma-separated list
            current[final_key] = [v.strip() for v in value.split(',')]
        else:
            current[final_key] = value
    
    return structure

def print_yaml(data, indent=0):
    """Print YAML structure"""
    for key, value in data.items():
        if isinstance(value, dict):
            print(f"{'  ' * indent}{key}:")
            print_yaml(value, indent + 1)
        elif isinstance(value, list):
            print(f"{'  ' * indent}{key}:")
            for item in value:
                print(f"{'  ' * (indent + 1)}- {item}")
        elif isinstance(value, bool):
            print(f"{'  ' * indent}{key}: {str(value).lower()}")
        elif isinstance(value, (int, float)):
            print(f"{'  ' * indent}{key}: {value}")
        else:
            # Quote strings that might be ambiguous
            if any(c in str(value) for c in [':', '#', '@', '%']):
                print(f"{'  ' * indent}{key}: \"{value}\"")
            else:
                print(f"{'  ' * indent}{key}: {value}")

def main():
    if len(sys.argv) < 2:
        print("Usage: python env_to_yaml.py <.env file>", file=sys.stderr)
        sys.exit(1)
    
    env_file = sys.argv[1]
    env_vars = {}
    
    # Parse .env file
    with open(env_file, 'r') as f:
        for line in f:
            key, value = parse_env_line(line)
            if key and value is not None:
                env_vars[key] = value
    
    # Build and print YAML structure
    structure = build_yaml_structure(env_vars)
    
    print("# Attune Configuration")
    print("# Generated from .env file")
    print("# Please review and adjust as needed")
    print()
    
    print_yaml(structure)

if __name__ == '__main__':
    main()
```

**Usage:**
```bash
python env_to_yaml.py .env > config.yaml
```

**Note:** Always review the generated YAML file and adjust as needed.

## Handling Environment-Specific Configurations

One of the benefits of YAML is easy environment-specific configurations.

### Create Base Configuration

`config.yaml` (common settings):
```yaml
service_name: attune

database:
  max_connections: 50
  connect_timeout: 30

server:
  enable_cors: true
  max_body_size: 10485760

log:
  console: true
```

### Development Override

`config.development.yaml`:
```yaml
environment: development

database:
  url: postgresql://postgres:postgres@localhost:5432/attune
  log_statements: true

server:
  host: 127.0.0.1
  port: 8080

log:
  level: debug
  format: pretty

security:
  jwt_secret: dev-secret-not-for-production
  jwt_access_expiration: 86400  # 24 hours
```

### Production Configuration

`config.production.yaml`:
```yaml
environment: production

database:
  url: postgresql://user:password@db.prod.example.com/attune
  max_connections: 100

server:
  host: 0.0.0.0
  port: 8080
  cors_origins:
    - https://app.example.com
    - https://www.example.com

log:
  level: info
  format: json
  file: /var/log/attune/attune.log

# Use environment variables for secrets!
# ATTUNE__SECURITY__JWT_SECRET
# ATTUNE__SECURITY__ENCRYPTION_KEY
```

## Still Using Environment Variables

Environment variables still work for overrides! This is especially useful for:

- **Container deployments** (Docker, Kubernetes)
- **Production secrets** (don't commit secrets to YAML files)
- **CI/CD pipelines**
- **Quick overrides** without editing files

**Example:**
```bash
# Base config in config.yaml, override database URL
export ATTUNE__DATABASE__URL=postgresql://prod-db/attune
cargo run --bin attune-api
```

## Migration Checklist

- [ ] Back up your existing `.env` file
- [ ] Copy `config.example.yaml` to `config.yaml`
- [ ] Convert `.env` values to YAML format
- [ ] Add comments to document your configuration
- [ ] Test the application starts successfully
- [ ] Verify all features work (database, authentication, etc.)
- [ ] Create environment-specific configs if needed
- [ ] Update deployment scripts/documentation
- [ ] Set up environment variables for production secrets
- [ ] Add `config.yaml` to `.gitignore` (if it contains secrets)
- [ ] Commit example configs to version control

## Rollback Plan

If you need to roll back to `.env` files:

1. The old `.env` file still works (keep your backup)
2. Rename/remove `config.yaml` temporarily
3. Environment variables with `ATTUNE__` prefix still work

**Note:** The application will still load config from environment variables if no YAML file is present.

## Common Issues

### Issue: Configuration not loading

**Problem:** Application can't find `config.yaml`

**Solution:** 
- Ensure `config.yaml` is in the current working directory
- Or set `ATTUNE_CONFIG` environment variable:
  ```bash
  export ATTUNE_CONFIG=/path/to/config.yaml
  ```

### Issue: YAML syntax error

**Problem:** YAML parsing fails

**Solution:**
- Check indentation (use spaces, not tabs)
- Validate YAML syntax:
  ```bash
  # Using Python
  python -c "import yaml; yaml.safe_load(open('config.yaml'))"
  
  # Using yq
  yq eval config.yaml
  ```

### Issue: Boolean values not working

**Problem:** Boolean value treated as string

**Solution:** Use lowercase without quotes:
```yaml
# Correct
enable_auth: true
log_statements: false

# Incorrect
enable_auth: "true"    # String, not boolean
LOG_STATEMENTS: FALSE  # Wrong case
```

### Issue: Array values not parsing

**Problem:** CORS origins not loading correctly

**Solution:** Use YAML array syntax:
```yaml
# Correct
cors_origins:
  - http://localhost:3000
  - http://localhost:5173

# Also correct (inline)
cors_origins: [http://localhost:3000, http://localhost:5173]

# For env vars, use comma-separated
export ATTUNE__SERVER__CORS_ORIGINS="http://localhost:3000,http://localhost:5173"
```

### Issue: Special characters in strings

**Problem:** URL or path not parsing correctly

**Solution:** Quote strings with special characters:
```yaml
# With special characters
database:
  url: "postgresql://user:p@ssw0rd!@localhost/attune"

# Or use single quotes
metadata_cache:
  enabled: true
  url: 'redis://localhost:6379'
```

## Tips and Best Practices

### 1. Use Comments

Document your configuration:
```yaml
database:
  # Production database endpoint
  url: postgresql://user@db.example.com/attune
  
  # Adjust based on expected load
  max_connections: 100  # Increased for production traffic
```

### 2. Group Related Settings

Keep related configuration together:
```yaml
# All server-related settings
server:
  host: 0.0.0.0
  port: 8080
  cors_origins:
    - https://app.example.com
```

### 3. Use Environment-Specific Files

Don't duplicate settings—use base + overrides:
```
config.yaml              # Common settings
config.development.yaml  # Dev overrides
config.production.yaml   # Prod overrides
```

### 4. Never Commit Secrets

Use environment variables for sensitive data:
```yaml
# config.production.yaml (committed to git)
security:
  # Override these with environment variables!
  jwt_secret: CHANGE_ME_USE_ENV_VAR
  encryption_key: CHANGE_ME_USE_ENV_VAR
```

```bash
# In production (not in git)
export ATTUNE__SECURITY__JWT_SECRET=$(openssl rand -base64 64)
export ATTUNE__SECURITY__ENCRYPTION_KEY=$(openssl rand -base64 32)
```

### 5. Validate Configuration

Always validate after migration:
```bash
# The application will validate on startup
cargo run --bin attune-api

# Check for validation errors in logs
```

## Getting Help

If you encounter issues during migration:

1. **Check the logs**: Enable debug logging
   ```bash
   ATTUNE__LOG__LEVEL=debug cargo run --bin attune-api
   ```

2. **Review documentation**:
   - [Configuration Guide](configuration.md)
   - [Quick Start Guide](quick-start.md)

3. **Validate YAML syntax**: Use online validators or CLI tools

4. **Compare with examples**: Check `config.example.yaml`

## Summary

The migration from `.env` to YAML provides:

✅ Better readability and maintainability  
✅ Native support for complex data types  
✅ Easy environment-specific configurations  
✅ Inline documentation with comments  
✅ Backward compatibility with environment variables  

The YAML configuration system gives you more flexibility while maintaining the security and override capabilities of environment variables.