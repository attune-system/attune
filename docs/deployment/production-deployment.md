# Production Deployment Guide

This document provides guidelines and checklists for deploying Attune to production environments.

## Table of Contents

- [Pre-Deployment Checklist](#pre-deployment-checklist)
- [Database Configuration](#database-configuration)
- [Environment Variables](#environment-variables)
- [Schema Verification](#schema-verification)
- [Security Best Practices](#security-best-practices)
- [Deployment Steps](#deployment-steps)
- [Post-Deployment Validation](#post-deployment-validation)
- [Troubleshooting](#troubleshooting)

---

## Pre-Deployment Checklist

Before deploying Attune to production, verify the following:

- [ ] PostgreSQL 14+ database is provisioned and accessible
- [ ] RabbitMQ 3.12+ message queue is configured
- [ ] All required environment variables are set (see below)
- [ ] Database migrations have been tested in staging
- [ ] SSL/TLS certificates are configured for HTTPS
- [ ] Log aggregation and monitoring are configured
- [ ] Backup and disaster recovery procedures are in place
- [ ] Security audit has been completed
- [ ] Load balancing and high availability are configured (if applicable)

---

## Environment Variables

### Required Variables

These environment variables **MUST** be set before deploying:

```bash
# Database connection (required)
export DATABASE_URL="postgresql://username:password@host:port/database"

# JWT secret for authentication (required, 64+ characters)
# Generate with: openssl rand -base64 64
export JWT_SECRET="your-secure-jwt-secret-here"

# Encryption key for secrets storage (required, 32+ characters)
# Generate with: openssl rand -base64 32
export ENCRYPTION_KEY="your-secure-encryption-key-here"
```

### Optional Variables

```bash
# Redis (for caching)
export REDIS_URL="redis://host:6379"

# RabbitMQ (for message queue)
export RABBITMQ_URL="amqp://user:pass@host:5672/%2f"

# CORS origins (comma-separated)
export ATTUNE__SERVER__CORS_ORIGINS="https://app.example.com,https://www.example.com"

# Log level override
export ATTUNE__LOG__LEVEL="info"

# Server port override
export ATTUNE__SERVER__PORT="8080"

# Schema override (should always be 'attune' in production)
export ATTUNE__DATABASE__SCHEMA="attune"
```

### Environment Variable Format

Attune uses hierarchical configuration with the prefix `ATTUNE__` and separator `__`:

- `ATTUNE__DATABASE__URL` → `database.url`
- `ATTUNE__SERVER__PORT` → `server.port`
- `ATTUNE__LOG__LEVEL` → `log.level`

---

## Schema Verification

### Automatic Verification

Attune includes built-in schema validation:

1. **Schema Name Validation**: Only alphanumeric and underscores allowed (max 63 chars)
2. **SQL Injection Prevention**: Schema names are validated before use
3. **Logging**: Production schema usage is logged prominently at startup

### Manual Verification Script

Run this verification before deployment:

```bash
# Verify configuration loads correctly
cargo run --release --bin attune-api -- --config config.production.yaml --dry-run

# Check logs for schema confirmation
cargo run --release --bin attune-api 2>&1 | grep -i schema
```

Expected output:
```
INFO Using production schema: attune
INFO Connecting to database with max_connections=20, schema=attune
```

### Database Schema Check

After deployment, verify the schema in the database:

```bash
# Connect to your production database
psql $DATABASE_URL

# Verify schema exists
\dn attune

# Verify search_path includes attune
SHOW search_path;

# Verify tables are in attune schema
SELECT schemaname, tablename 
FROM pg_tables 
WHERE schemaname = 'attune' 
ORDER BY tablename;
```

You should see all 17 Attune tables:
- `action`
- `enforcement`
- `event`
- `execution`
- `execution_log`
- `identity`
- `inquiry`
- `inquiry_response`
- `key`
- `pack`
- `rule`
- `rule_enforcement`
- `sensor`
- `sensor_instance`
- `trigger`
- `trigger_instance`
- `workflow_definition`

---

## Security Best Practices

### Secrets Management

1. **Never commit secrets to version control**
2. **Use environment variables or secret management systems** (e.g., AWS Secrets Manager, HashiCorp Vault)
3. **Rotate secrets regularly** (JWT secret, encryption key, database passwords)
4. **Use strong, randomly generated secrets** (use provided generation commands)

### Database Security

1. **Use dedicated database user** with minimal required permissions
2. **Enable SSL/TLS** for database connections
3. **Use connection pooling** (configured via `max_connections`)
4. **Restrict network access** to database (firewall rules, VPC, etc.)
5. **Enable audit logging** for sensitive operations

### Application Security

1. **Run as non-root user** in containers/VMs
2. **Enable HTTPS** for all API endpoints (use reverse proxy like nginx)
3. **Configure CORS properly** (only allow trusted origins)
4. **Set up rate limiting** and DDoS protection
5. **Enable security headers** (CSP, HSTS, X-Frame-Options, etc.)
6. **Keep dependencies updated** (run `cargo audit` regularly)

---

## Deployment Steps

### 1. Prepare Database

```bash
# Create production database (if not exists)
createdb -h your-db-host -U your-db-user attune_prod

# Run migrations
export DATABASE_URL="postgresql://user:pass@host:port/attune_prod"
export ATTUNE__DATABASE__SCHEMA="attune"
sqlx migrate run --source ./migrations
```

### 2. Build Application

```bash
# Build release binary
cargo build --release --bin attune-api

# Or build Docker image
docker build -t attune-api:latest -f docker/api.Dockerfile .
```

### 3. Configure Environment

```bash
# Set all required environment variables
export DATABASE_URL="postgresql://..."
export JWT_SECRET="$(openssl rand -base64 64)"
export ENCRYPTION_KEY="$(openssl rand -base64 32)"
export ATTUNE__DATABASE__SCHEMA="attune"
# ... etc
```

### 4. Deploy Services

```bash
# Start API service
./target/release/attune-api --config config.production.yaml

# Or with Docker
docker run -d \
  --name attune-api \
  -p 8080:8080 \
  -e DATABASE_URL="$DATABASE_URL" \
  -e JWT_SECRET="$JWT_SECRET" \
  -e ENCRYPTION_KEY="$ENCRYPTION_KEY" \
  -v ./config.production.yaml:/app/config.production.yaml \
  attune-api:latest
```

### 4a. Configure Service Log Forwarding

For Docker / Compose deployments, the supported Attune service-log forwarding
contract is **container `stdout`/`stderr`**, not `/opt/attune/logs` named
volumes.

Canonical reference: [`structured-logging.md`](structured-logging.md).

Operator summary:

- Base Compose files default to Docker `json-file` logging with rotation
- Long-running Attune containers include stable labels such as
  `com.attune.service`, `com.attune.log.contract=container-stdout-stderr`, and
  Datadog `service`/`env`/`version` tags
- Datadog / Splunk forwarders should ingest structured JSON service logs from
  the container stream
- Raw execution and sensor stdout/stderr remains in private
  `classification=runtime_log` artifacts and is not the service-log forwarding
  source of truth

Quick verification:

```bash
docker inspect attune-api --format '{{json .HostConfig.LogConfig}}'
docker inspect attune-api --format '{{json .Config.Labels}}'
docker logs --tail 20 attune-api
```

### 5. Load Core Pack

```bash
# Load the core pack (provides essential actions and sensors)
./scripts/load-core-pack.sh
```

---

## Post-Deployment Validation

### Health Check

```bash
# Check API health endpoint
curl http://your-api-host:8080/health

# Expected response:
# {"status":"ok","timestamp":"2024-01-15T12:00:00Z"}
```

### Schema Validation

```bash
# Check application logs for schema confirmation
docker logs attune-api 2>&1 | grep -i schema

# Expected output:
# INFO Using production schema: attune
```

### Functional Tests

```bash
# Test authentication
curl -X POST http://your-api-host:8080/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"your-password"}'

# Test pack listing
curl http://your-api-host:8080/api/v1/packs \
  -H "Authorization: Bearer YOUR_TOKEN"

# Test action execution
curl -X POST http://your-api-host:8080/api/v1/executions \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"action_ref":"core.echo","parameters":{"message":"Hello"}}'
```

### Monitoring

Set up monitoring for:

- **Application health**: `/health` endpoint availability
- **Database connections**: Pool size and connection errors
- **Error rates**: 4xx and 5xx HTTP responses
- **Response times**: P50, P95, P99 latencies
- **Resource usage**: CPU, memory, disk, network
- **Schema usage**: Verify `attune` schema in logs
- **Container log forwarding**: Verify your Docker logging driver / agent is
  receiving Attune container stdout/stderr

---

## Troubleshooting

### Issue: Schema Not Found

**Symptoms:**
- Application startup fails with "schema does not exist"
- Database queries fail with "schema not found"

**Solution:**
1. Verify schema exists: `psql $DATABASE_URL -c "\dn attune"`
2. If missing, run migrations: `sqlx migrate run --source ./migrations`
3. Check migration files uncommented schema creation (first migration)

### Issue: Connection Pool Exhausted

**Symptoms:**
- Timeout errors
- "connection pool exhausted" errors
- Slow response times

**Solution:**
1. Increase `max_connections` in config
2. Check for connection leaks in application logs
3. Verify database can handle the connection load
4. Consider scaling horizontally (multiple instances)

### Issue: Authentication Fails

**Symptoms:**
- All requests return 401 Unauthorized
- Token validation errors in logs

**Solution:**
1. Verify `JWT_SECRET` is set correctly
2. Check token expiration times in config
3. Ensure clocks are synchronized (NTP)
4. Verify `enable_auth: true` in config

### Issue: Migrations Fail

**Symptoms:**
- `sqlx migrate run` errors
- "relation already exists" or "schema already exists"

**Solution:**
1. Check `_sqlx_migrations` table: `SELECT * FROM attune._sqlx_migrations;`
2. Verify migrations are in correct order
3. For fresh deployment, drop and recreate schema if safe
4. Check PostgreSQL version compatibility (requires 14+)

---

## Rollback Procedure

If issues occur after deployment:

1. **Stop the application**: `systemctl stop attune-api` (or equivalent)
2. **Revert to previous version**: Deploy previous known-good version
3. **Restore database backup** (if migrations were run):
   ```bash
   pg_restore -d attune_prod backup.dump
   ```
4. **Verify old version works**: Run post-deployment validation steps
5. **Investigate issue**: Review logs, error messages, configuration changes

---

## Additional Resources

- [Configuration Guide](./configuration.md)
- [Schema-Per-Test Architecture](./schema-per-test.md)
- [API Documentation](./api-overview.md)
- [Security Best Practices](./security.md)
- [Monitoring and Observability](./monitoring.md)

---

## Support

For production issues or questions:

- GitHub Issues: https://github.com/your-org/attune/issues
- Documentation: https://docs.attune.example.com
- Community: https://community.attune.example.com
