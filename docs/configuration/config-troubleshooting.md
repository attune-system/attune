# Configuration Troubleshooting Guide

This guide helps you troubleshoot common configuration issues with Attune.

## Quick Diagnostics

### Check Configuration is Valid

```bash
# Test configuration loading
cargo run --bin attune-api -- --help

# Enable debug logging to see config details
ATTUNE__LOG__LEVEL=debug cargo run --bin attune-api
```

### Verify Configuration Files Exist

```bash
# Check for config files
ls -la config*.yaml

# View current configuration
cat config.yaml
```

### Test Environment Variables

```bash
# List all Attune environment variables
env | grep ATTUNE

# Test a specific variable
echo $ATTUNE__DATABASE__URL
```

## Common Issues

### 1. Encryption Key Must Be At Least 32 Characters

**Error:**
```
Error: Validation error: Encryption key must be at least 32 characters
```

**Cause:** The `encryption_key` in your configuration is less than 32 characters.

**Solution:**

Generate a proper encryption key:
```bash
# Generate a 32-byte key (base64 encoded = ~44 characters)
openssl rand -base64 32

# Example output:
# R3mHJ8qP5xK2nB9vW7tY4uZ1aE6cF0dG8hI2jK4lM5n=
```

Update your configuration:
```yaml
security:
  encryption_key: R3mHJ8qP5xK2nB9vW7tY4uZ1aE6cF0dG8hI2jK4lM5n=
```

Or use an environment variable:
```bash
export ATTUNE__SECURITY__ENCRYPTION_KEY=$(openssl rand -base64 32)
```

**Note:** The key must be **at least 32 characters** after base64 decoding. A 32-byte random value base64-encoded is typically 44 characters.

### 2. JWT Secret Required

**Error:**
```
Error: Validation error: JWT secret is required when authentication is enabled
```

**Cause:** Authentication is enabled but no JWT secret is configured.

**Solution Option 1 - Provide a JWT secret:**
```yaml
security:
  jwt_secret: your-secret-here
  enable_auth: true
```

Generate a secure secret:
```bash
openssl rand -base64 64
```

**Solution Option 2 - Disable authentication (dev only):**
```yaml
security:
  enable_auth: false
```

### 3. Configuration File Not Found

**Error:**
```
Error: Configuration error: ...
```

**Cause:** The application can't find `config.yaml`.

**Solution:**

Check if the file exists:
```bash
ls -la config.yaml
```

If missing, create it from the example:
```bash
cp config.example.yaml config.yaml
nano config.yaml
```

Or specify a custom path:
```bash
export ATTUNE_CONFIG=/path/to/config.yaml
cargo run --bin attune-api
```

### 4. Database Connection Failed

**Error:**
```
Error: Database error: connection refused
Error: Database error: password authentication failed
```

**Cause:** Cannot connect to PostgreSQL database.

**Solution:**

1. **Verify PostgreSQL is running:**
   ```bash
   pg_isready -h localhost -p 5432
   ```

2. **Check connection string:**
   ```yaml
   database:
     url: postgresql://username:password@host:port/database
   ```

3. **Test connection manually:**
   ```bash
   psql postgresql://postgres:postgres@localhost:5432/attune
   ```

4. **Verify database exists:**
   ```bash
   psql -U postgres -c "SELECT datname FROM pg_database WHERE datname='attune';"
   ```

5. **Create database if needed:**
   ```bash
   createdb -U postgres attune
   ```

### 5. Invalid Log Level

**Error:**
```
Error: Invalid log level: TRACE. Must be one of: ["trace", "debug", "info", "warn", "error"]
```

**Cause:** Log level must be lowercase.

**Solution:**
```yaml
log:
  level: debug  # Use lowercase
```

Not:
```yaml
log:
  level: DEBUG  # Wrong - uppercase not allowed
```

### 6. YAML Syntax Error

**Error:**
```
Error: Configuration error: invalid type: string "true", expected a boolean
Error: while parsing a block mapping
```

**Cause:** Invalid YAML syntax.

**Common mistakes:**

**Wrong - Quotes around boolean:**
```yaml
enable_auth: "true"  # This is a string, not a boolean
```

**Correct:**
```yaml
enable_auth: true
```

**Wrong - Mixed spaces and tabs:**
```yaml
database:
	url: postgresql://...  # Tab used for indentation
```

**Correct - Use spaces only:**
```yaml
database:
  url: postgresql://...  # 2 spaces
```

**Wrong - Missing space after colon:**
```yaml
level:debug  # No space after colon
```

**Correct:**
```yaml
level: debug  # Space after colon
```

**Validate YAML syntax:**
```bash
# Using Python
python3 -c "import yaml; yaml.safe_load(open('config.yaml'))"

# Using yq
yq eval config.yaml

# Using Ruby
ruby -ryaml -e "YAML.load_file('config.yaml')"
```

### 7. Environment Variable Not Working

**Error:** Configuration value not being overridden by environment variable.

**Cause:** Incorrect environment variable format.

**Correct format:**
```bash
ATTUNE__SECTION__KEY=value
```

**Common mistakes:**

**Wrong - Single underscore:**
```bash
ATTUNE_DATABASE_URL=...  # Wrong
```

**Correct - Double underscore:**
```bash
ATTUNE__DATABASE__URL=...  # Correct
```

**Wrong - Missing ATTUNE prefix:**
```bash
DATABASE__URL=...  # Wrong
```

**Correct - With ATTUNE prefix:**
```bash
ATTUNE__DATABASE__URL=...  # Correct
```

**Wrong - Incorrect case:**
```bash
attune__database__url=...  # Wrong (lowercase prefix)
```

**Correct - ATTUNE in uppercase:**
```bash
ATTUNE__DATABASE__URL=...  # Correct
```

**For arrays (comma-separated):**
```bash
ATTUNE__SERVER__CORS_ORIGINS="http://localhost:3000,http://localhost:5173"
```

### 8. Port Already in Use

**Error:**
```
Error: Address already in use (os error 98)
```

**Cause:** Another process is using port 8080.

**Solution:**

1. **Find the process:**
   ```bash
   lsof -i :8080
   # Or
   netstat -tulpn | grep 8080
   ```

2. **Kill the process:**
   ```bash
   kill <PID>
   ```

3. **Or change the port:**
   ```yaml
   server:
     port: 8081
   ```
   
   Or with environment variable:
   ```bash
   ATTUNE__SERVER__PORT=8081 cargo run --bin attune-api
   ```

### 9. CORS Issues

**Error in browser console:**
```
Access to fetch at 'http://localhost:8080/api/v1/...' from origin 'http://localhost:3000' 
has been blocked by CORS policy
```

**Cause:** Frontend origin not in allowed CORS origins.

**Solution:**

Add your frontend URL to CORS origins:
```yaml
server:
  cors_origins:
    - http://localhost:3000
    - http://localhost:5173  # Vite default
```

Or use environment variable:
```bash
export ATTUNE__SERVER__CORS_ORIGINS="http://localhost:3000,http://localhost:5173"
```

### 10. Configuration Changes Not Taking Effect

**Cause:** Cached binary or incorrect environment.

**Solution:**

1. **Rebuild the application:**
   ```bash
   cargo clean
   cargo build --bin attune-api
   ```

2. **Check which config file is being loaded:**
   ```bash
   ATTUNE__LOG__LEVEL=debug cargo run --bin attune-api 2>&1 | grep -i config
   ```

3. **Verify environment:**
   ```bash
   env | grep ATTUNE
   ```

4. **Use explicit config file:**
   ```bash
   ATTUNE_CONFIG=./config.development.yaml cargo run --bin attune-api
   ```

## Advanced Diagnostics

### Debug Configuration Loading

Enable debug logging to see configuration details:

```bash
ATTUNE__LOG__LEVEL=debug cargo run --bin attune-api 2>&1 | tee debug.log
```

Look for lines containing:
- "Configuration loaded"
- "Environment:"
- "Loaded configuration from"

### Verify Configuration Merging

Test the configuration priority:

1. **Base config only:**
   ```bash
   cargo run --bin attune-api
   ```

2. **With environment-specific config:**
   ```bash
   ATTUNE__ENVIRONMENT=development cargo run --bin attune-api
   ```

3. **With environment variable override:**
   ```bash
   ATTUNE__SERVER__PORT=9999 cargo run --bin attune-api
   ```

### Check Database Connection

Test database connectivity separately:

```bash
# Using psql
psql -U postgres -h localhost -d attune -c "SELECT version();"

# Using sqlx-cli
sqlx database create --database-url postgresql://postgres:postgres@localhost/attune
sqlx migrate run --database-url postgresql://postgres:postgres@localhost/attune
```

### Validate All Configuration Values

```bash
# Dump configuration (be careful - may contain secrets!)
ATTUNE__LOG__LEVEL=debug cargo run --bin attune-api 2>&1 | grep -A 50 "Configuration loaded"
```

## Security Checklist

When troubleshooting, ensure:

- [ ] Secrets are not logged or displayed
- [ ] Configuration files with secrets are in `.gitignore`
- [ ] Production secrets use environment variables
- [ ] File permissions are restrictive (`chmod 600 config.yaml`)
- [ ] JWT secrets are cryptographically strong (64+ bytes)
- [ ] Encryption keys are at least 32 characters

## Getting More Help

### Enable Verbose Logging

```bash
RUST_LOG=debug,attune_common=trace cargo run --bin attune-api
```

### Check Application Logs

```bash
# If logging to file
tail -f /var/log/attune/attune.log

# With pretty formatting
ATTUNE__LOG__FORMAT=pretty cargo run --bin attune-api
```

### Validate Configuration Programmatically

Create a test file `check_config.rs`:

```rust
use attune_common::config::Config;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load()?;
    config.validate()?;
    println!("✓ Configuration is valid");
    println!("Environment: {}", config.environment);
    println!("Database URL: {}", config.database.url);
    println!("Server: {}:{}", config.server.host, config.server.port);
    Ok(())
}
```

Run it:
```bash
cargo run --bin check_config
```

## Reference

- [Configuration Guide](configuration.md) - Complete reference
- [Migration Guide](env-to-yaml-migration.md) - Migrating from .env
- [Quick Start](quick-start.md) - Getting started
- [CONFIG_README.md](../CONFIG_README.md) - Quick reference

## Common Configuration Patterns

### Development Setup

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
  jwt_secret: dev-secret-change-in-production
  encryption_key: dev-key-at-least-32-characters-long
  enable_auth: true
```

### Production Setup

```yaml
environment: production
database:
  url: postgresql://user:pass@db.example.com/attune
  max_connections: 100
server:
  host: 0.0.0.0
  cors_origins:
    - https://app.example.com
log:
  level: info
  format: json
  # file: /var/log/attune/attune.log  # Reserved; file sink not implemented yet
```

With environment variables:
```bash
export ATTUNE__SECURITY__JWT_SECRET=$(cat /run/secrets/jwt_secret)
export ATTUNE__SECURITY__ENCRYPTION_KEY=$(cat /run/secrets/encryption_key)
```

### Testing Setup

```yaml
environment: test
database:
  url: postgresql://postgres:postgres@localhost:5432/attune_test
server:
  port: 0  # Random port
log:
  level: warn
security:
  jwt_secret: test-secret-not-secure
  encryption_key: test-key-at-least-32-characters-long
```

---

If you continue to experience issues, please check the [GitHub Issues](https://github.com/yourusername/attune/issues) or open a new issue with:
1. Error message
2. Your configuration (with secrets redacted)
3. Output of `cargo --version` and `rustc --version`
4. Operating system and PostgreSQL version