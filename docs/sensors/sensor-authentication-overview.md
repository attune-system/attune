# Sensor Authentication Overview

**Version:** 1.0  
**Last Updated:** 2026-07-21

## Quick Summary

This document provides a quick overview of how sensors authenticate with Attune. For full details, see:

- **[Sensor Interface Specification](./sensor-interface.md)** - Complete sensor implementation guide
- **[Service Accounts](./service-accounts.md)** - Token creation and management

## How It Works

1. **Admin creates sensor service account** via API:
   ```bash
   POST /service-accounts
   {
     "name": "sensor:core.timer",
     "scope": "sensor",
     "ttl_days": 90
   }
   ```

2. **Admin receives long-lived token** (shown only once):
   ```json
   {
     "identity_id": 123,
     "token": "eyJhbGci...",
     "expires_at": "2025-04-27T12:34:56Z"
   }
   ```

3. **Token is deployed with sensor** via environment variable:
   ```bash
   export ATTUNE_API_TOKEN="eyJhbGci..."
   export ATTUNE_API_URL="http://localhost:8080"
   export ATTUNE_SENSOR_REF="core.timer"
   ./attune-sensor
   ```

4. **Sensor uses token for all API calls**:
   - Fetch active rules: `GET /rules?trigger_type=core.timer`
   - Create events: `POST /events`
   - Fetch trigger metadata: `GET /triggers/{ref}`

## Token Properties

| Property | Value |
|----------|-------|
| **Type** | JWT (stateless) |
| **Lifetime** | 24-72 hours (auto-expires, REQUIRED) |
| **Scope** | `sensor` |
| **Permissions** | Create events, read rules/triggers (restricted to declared trigger types) |
| **Revocable** | Yes (via `/service-accounts/{id}` DELETE) |
| **Rotation** | Automatic for managed sensors before expiry, using a controlled process restart |
| **Expiration** | All tokens MUST have `exp` claim to prevent revocation table bloat |

## Managed Sensor Cache Authority

The sensor service provisions each managed process with `ATTUNE_API_URL`,
`ATTUNE_API_TOKEN`, `ATTUNE_SENSOR_REF`, and `ATTUNE_PACK_REF`. Token minting
carries the exact registered sensor ref, pack ref, trigger refs, and explicit
permission-set refs. The API resolves the registered sensor and signs the
effective authority; it must not trust unverified request fields or inherit
ordinary identity roles.

Sensors opt into cache permission sets through registered sensor configuration:

```yaml
config:
  cache_permission_set_refs:
    - standard
    - salesforce.cache_writer
```

Only `cache_permission_set_refs` is used for cache authority; a generic
`permission_set_refs` configuration value is ignored. The token request and
response must agree on the registered trigger scope and explicit permission
sets. A cache-enabled sensor fails closed if the API omits or changes its pack
scope or cache authority.

Cache `standard` access is read-only for the registered sensor and pack.
Refresh, upload, seal, promotion, abandonment, and deletion require a named
explicit write grant. Cache values are fetched deliberately through the API;
they are not delivered through sensor configuration, action parameters, secret
stdin, or direct database access.

Sensors must treat Data Caches as Attune-local, reconstructable snapshots. The
external source remains authoritative; credentials stay in Keys and Secrets,
and a sensor must not use a cache as durable business history or a general
query database.

Use a generated SDK from the cache OpenAPI contract for point lookup, bounded
multi-ID lookup, and generation-pinned page traversal. A snapshot-expired
response ends the traversal; callers must never continue with a newer
generation implicitly.
Runtime definitions cannot override managed `ATTUNE_*` variables, including
the signed sensor token and exact pack/sensor refs.

## Security Best Practices

### DO:
- ✅ Store tokens in environment variables or secure config management
- ✅ Use HTTPS for API calls in production
- ✅ Redact tokens in logs (show only last 4 characters)
- ✅ Revoke tokens immediately if compromised
- ✅ Use separate tokens for each sensor type
- ✅ Set TTL to 24-72 hours for sensors (requires periodic rotation)
- ✅ Monitor managed-sensor renewal/restart failures; Attune rotates their tokens before expiry

### DON'T:
- ❌ Commit tokens to version control
- ❌ Log full token values
- ❌ Share tokens between sensors
- ❌ Send tokens over unencrypted connections
- ❌ Store tokens on disk unencrypted
- ❌ Pass tokens in URL query parameters

## Configuration Methods

### Method 1: Environment Variables (Recommended)

```bash
export ATTUNE_API_URL="http://localhost:8080"
export ATTUNE_API_TOKEN="eyJhbGci..."
export ATTUNE_SENSOR_REF="core.timer"
export ATTUNE_MQ_URL="amqp://localhost:5672"

./attune-sensor
```

### Method 2: stdin JSON

```bash
echo '{
  "api_url": "http://localhost:8080",
  "api_token": "eyJhbGci...",
  "sensor_ref": "core.timer",
  "mq_url": "amqp://localhost:5672"
}' | ./attune-sensor
```

### Method 3: Configuration File + Environment Override

```yaml
# sensor.yaml
api_url: http://localhost:8080
sensor_ref: core.timer
mq_url: amqp://localhost:5672
# Token provided via environment for security
```

```bash
export ATTUNE_API_TOKEN="eyJhbGci..."
./attune-sensor --config sensor.yaml
```

## Token Lifecycle

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Admin creates service account                            │
│    POST /service-accounts                                    │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────────┐
│ 2. API generates JWT token                                  │
│    - Sets scope: "sensor"                                    │
│    - Sets expiration (e.g., 90 days)                         │
│    - Includes identity_id, trigger_types                     │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────────┐
│ 3. Token stored securely by admin                           │
│    - Environment variable                                    │
│    - Secret management system (Vault, k8s secrets)           │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────────┐
│ 4. Sensor starts and reads token                            │
│    - From ATTUNE_API_TOKEN env var                           │
│    - Or from stdin JSON                                      │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────────┐
│ 5. Sensor makes API calls with token                        │
│    Authorization: Bearer eyJhbGci...                         │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────────┐
│ 6. API validates token on each request                      │
│    - Verify JWT signature                                    │
│    - Check expiration                                        │
│    - Check revocation list                                   │
│    - Verify scope matches endpoint requirements              │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────────┐
│ 7. Token eventually expires or is revoked                   │
│    - Auto-expires after TTL                                  │
│    - Or admin revokes: DELETE /service-accounts/{id}         │
└─────────────────────────────────────────────────────────────┘
```

## JWT Token Structure

```json
{
  "sub": "sensor:core.timer",
  "jti": "abc123...",
  "iat": 1706356496,
  "exp": 1714132496,
  "identity_id": 123,
  "identity_type": "service_account",
  "scope": "sensor",
  "metadata": {
    "trigger_types": ["core.timer"]
  }
}
```

## Permissions by Scope

| Scope | Create Events | Read Rules | Read Triggers | Read Keys | Update Execution |
|-------|---------------|------------|---------------|-----------|------------------|
| `sensor` | ✅ (restricted)* | ✅ | ✅ | ❌ | ❌ |
| `action_execution` | ❌ | ❌ | ❌ | ✅ | ✅ |
| `webhook` | ✅ | ❌ | ❌ | ❌ | ❌ |
| `user` | ✅ | ✅ | ✅ | ✅ | ✅ |
| `admin` | ✅ | ✅ | ✅ | ✅ | ✅ |

**\* Sensor tokens can only create events for trigger types declared in their token's `metadata.trigger_types`. The API enforces this restriction and returns `403 Forbidden` for unauthorized trigger types.**

## Example: Creating a Sensor Token

```bash
# 1. Create service account (admin only)
curl -X POST http://localhost:8080/service-accounts \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "sensor:core.timer",
    "scope": "sensor",
    "description": "Timer sensor for interval-based triggers",
    "ttl_hours": 72,
    "metadata": {
      "trigger_types": ["core.timer"]
    }
  }'

# Note: This token can ONLY create events for "core.timer" trigger type.
# Attempting to create events for other trigger types will fail with 403 Forbidden.

# Response (SAVE THE TOKEN - shown only once):
{
  "identity_id": 123,
  "name": "sensor:core.timer",
  "scope": "sensor",
  "token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJzZW5zb3I6Y29yZS50aW1lciIsImp0aSI6ImFiYzEyMyIsImlhdCI6MTcwNjM1NjQ5NiwiZXhwIjoxNzA2NjE1Njk2LCJpZGVudGl0eV9pZCI6MTIzLCJpZGVudGl0eV90eXBlIjoic2VydmljZV9hY2NvdW50Iiwic2NvcGUiOiJzZW5zb3IiLCJtZXRhZGF0YSI6eyJ0cmlnZ2VyX3R5cGVzIjpbImNvcmUudGltZXIiXX19.signature",
  "expires_at": "2025-01-30T12:34:56Z"
}

# 2. Deploy token with sensor
export ATTUNE_API_TOKEN="eyJhbGci..."
export ATTUNE_API_URL="http://localhost:8080"
export ATTUNE_SENSOR_REF="core.timer"
export ATTUNE_MQ_URL="amqp://localhost:5672"

./attune-sensor

# 3. Rotate token before expiration (every 24-72 hours)
# - Create new service account
# - Update ATTUNE_API_TOKEN
# - Restart sensor
```

## Troubleshooting

### Token Validation Errors

**Error: "Token expired"**
- Token has exceeded its TTL
- Solution: Create a new service account and token

**Error: "Token revoked"**
- Token was manually revoked by admin
- Solution: Create a new service account and token

**Error: "Invalid signature"**
- JWT_SECRET mismatch between token creation and validation
- Solution: Ensure all services use the same JWT_SECRET

**Error: "Insufficient permissions"**
- Token scope doesn't match required endpoint permissions
- For sensors: Attempting to create event for trigger type not in `metadata.trigger_types`
- Solution: Create token with correct scope and trigger types (e.g., "sensor" scope with ["core.timer"])

### Common Mistakes

1. **Using user token for sensor**: User tokens have different scope, create a service account instead
2. **Hardcoding token in code**: Use environment variables or config management
3. **Sharing token between sensors**: Each sensor should have its own token
4. **Not revoking compromised tokens**: Use DELETE /service-accounts/{id} immediately

## Implementation Status

- [ ] Database schema for service accounts (`identity_type` column)
- [ ] Database schema for token revocation (`token_revocation` table with `token_exp` column)
- [ ] API endpoint: POST /service-accounts (with TTL parameter)
- [ ] API endpoint: GET /service-accounts
- [ ] API endpoint: DELETE /service-accounts/{id}
- [ ] Middleware for token validation (check expiration)
- [ ] Middleware for revocation checking (skip expired tokens)
- [ ] Executor creates execution tokens (TTL = action timeout)
- [ ] Worker passes execution tokens to actions
- [ ] CLI commands for service account management
- [ ] Sensor accepts and uses tokens
- [ ] Cleanup job for expired token revocations (hourly cron)
- [ ] Monitoring alerts for token expiration (6 hours before)

## Next Steps

1. Implement database migrations for service accounts
2. Add service account CRUD endpoints to API (with TTL parameters)
3. Update sensor to accept and use API tokens
4. Add token creation to executor for action executions (TTL = action timeout)
5. Implement cleanup job for expired token revocations
6. Document token rotation procedures (manual every 24-72 hours)
7. Add monitoring for token expiration warnings (alert 6 hours before)
8. Add graceful handling of token expiration in sensors

## Related Documentation

- [Sensor Interface Specification](./sensor-interface.md) - Full sensor implementation guide
- [Service Accounts](./service-accounts.md) - Detailed token management
- [API Architecture](./api-architecture.md) - API design and authentication
- [Security Best Practices](./security.md) - Security guidelines (future)
