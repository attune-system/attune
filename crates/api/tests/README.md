# API Integration Tests

This directory contains integration tests for the Attune API service.

## Test Files

- `webhook_api_tests.rs` - Basic webhook management and receiver endpoint tests (8 tests)
- `webhook_security_tests.rs` - Comprehensive webhook security feature tests (17 tests)

## Prerequisites

Before running tests, ensure:

1. **PostgreSQL is running** on `localhost:5432` (or set `DATABASE_URL`)
2. **Database migrations are applied**: `sqlx migrate run`
3. **Test user exists** (username: `test_user`, password: `test_password`)

### Quick Setup

```bash
# Set database URL
export DATABASE_URL="postgresql://postgres:postgres@localhost:5432/attune"

# Run migrations
sqlx migrate run

# Create test user (run from psql or create via API)
# The test user is created automatically when you run the API for the first time
# Or create manually:
psql $DATABASE_URL -c "
INSERT INTO attune.identity (username, email, password_hash, enabled)
VALUES ('test_user', 'test@example.com', 
  crypt('test_password', gen_salt('bf')), true)
ON CONFLICT (username) DO NOTHING;
"
```

## Running Tests

Database-backed tests run normally after `make db-test-setup`. Tests that need additional services remain ignored.

### Run all API integration tests
```bash
cargo test -p attune-api --test '*' -- --test-threads=4
```

### Run webhook API tests only
```bash
cargo test -p attune-api --test webhook_api_tests -- --test-threads=4
```

### Run webhook security tests only
```bash
cargo test -p attune-api --test webhook_security_tests -- --test-threads=4
```

### Run a specific test
```bash
cargo test -p attune-api --test webhook_security_tests test_webhook_hmac_sha256_valid -- --nocapture --test-threads=4
```

### Run tests with output
```bash
cargo test -p attune-api --test webhook_security_tests -- --nocapture --test-threads=4
```

## Test Categories

### Basic Webhook Tests (`webhook_api_tests.rs`)
- Webhook enable/disable/regenerate operations
- Webhook receiver with valid/invalid keys
- Authentication enforcement
- Disabled webhook handling

### Security Feature Tests (`webhook_security_tests.rs`)

#### HMAC Signature Tests
- `test_webhook_hmac_sha256_valid` - SHA256 signature validation
- `test_webhook_hmac_sha512_valid` - SHA512 signature validation
- `test_webhook_hmac_invalid_signature` - Invalid signature rejection
- `test_webhook_hmac_missing_signature` - Missing signature rejection
- `test_webhook_hmac_wrong_secret` - Wrong secret rejection

#### Rate Limiting Tests
- `test_webhook_rate_limit_enforced` - Rate limit enforcement
- `test_webhook_rate_limit_disabled` - No rate limit when disabled

#### IP Whitelisting Tests
- `test_webhook_ip_whitelist_allowed` - Allowed IPs pass
- `test_webhook_ip_whitelist_blocked` - Blocked IPs rejected

#### Payload Size Tests
- `test_webhook_payload_size_limit_enforced` - Size limit enforcement
- `test_webhook_payload_size_within_limit` - Valid size acceptance

#### Event Logging Tests
- `test_webhook_event_logging_success` - Success logging
- `test_webhook_event_logging_failure` - Failure logging

#### Combined Security Tests
- `test_webhook_all_security_features_pass` - All features enabled
- `test_webhook_multiple_security_failures` - Multiple failures

#### Error Scenarios
- `test_webhook_malformed_json` - Invalid JSON handling
- `test_webhook_empty_payload` - Empty payload handling

## Troubleshooting

### "Failed to connect to database"
- Ensure PostgreSQL is running: `pg_isready -h localhost -p 5432`
- Check `DATABASE_URL` is set correctly
- Test connection: `psql $DATABASE_URL -c "SELECT 1"`

### "Trigger not found" or table errors
- Run migrations: `sqlx migrate run`
- Check schema exists: `psql $DATABASE_URL -c "\dn"`

### "Authentication required" errors
- Ensure test user exists with correct credentials
- Check `JWT_SECRET` environment variable is set

### Tests timeout
- Increase timeout while keeping the required concurrency: `cargo test -- --test-threads=4`
- Check database performance
- Reduce concurrent test execution

### Rate limit tests fail
- Clear webhook event logs between runs
- Database fixtures provide per-test isolation; run with `cargo test -- --test-threads=4`

## Documentation

For comprehensive test documentation, see:
- `docs/webhook-testing.md` - Full test suite documentation
- `docs/webhook-manual-testing.md` - Manual testing guide
- `docs/webhook-system-architecture.md` - Webhook system architecture

## CI/CD

These tests are designed to run in CI with:
- PostgreSQL service container
- Automatic migration application
- Test user creation script
- Parallel test execution (where safe)
