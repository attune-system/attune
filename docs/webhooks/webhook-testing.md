# Webhook Testing Documentation

This document describes the comprehensive test suite for the Attune webhook system, covering both Phase 2 (basic functionality) and Phase 3 (advanced security features).

## Test Files

### 1. `crates/common/tests/webhook_tests.rs`
Repository-level integration tests for webhook database operations.

**Coverage:**
- Webhook enable/disable functionality
- Webhook key generation and uniqueness
- Webhook key regeneration
- Finding triggers by webhook key
- Idempotent webhook enabling

**Key Tests:**
- `test_webhook_enable` - Verifies webhook can be enabled and generates valid key
- `test_webhook_disable` - Verifies webhook can be disabled while preserving key
- `test_webhook_key_regeneration` - Tests key rotation functionality
- `test_find_by_webhook_key` - Tests lookup by webhook key
- `test_webhook_key_uniqueness` - Ensures unique keys across triggers
- `test_enable_webhook_idempotent` - Verifies enabling twice returns same key

### 2. `crates/api/tests/webhook_api_tests.rs`
API endpoint integration tests for webhook management and basic receiving.

**Coverage:**
- Webhook management endpoints (enable/disable/regenerate)
- Basic webhook receiving
- Authentication requirements
- Error handling for invalid keys

**Key Tests:**
- `test_enable_webhook` - Tests enabling webhooks via API
- `test_disable_webhook` - Tests disabling webhooks via API
- `test_regenerate_webhook_key` - Tests key regeneration via API
- `test_receive_webhook` - Tests successful webhook reception
- `test_receive_webhook_invalid_key` - Tests invalid key handling
- `test_receive_webhook_disabled` - Tests disabled webhook rejection
- `test_webhook_requires_auth_for_management` - Tests auth enforcement
- `test_receive_webhook_minimal_payload` - Tests minimal payload acceptance

### 3. `crates/api/tests/webhook_security_tests.rs`
Comprehensive security feature tests (Phase 3).

**Coverage:**
- HMAC signature verification (SHA256, SHA512, SHA1)
- Rate limiting
- IP whitelisting (IPv4, IPv6, CIDR)
- Payload size limits
- Event logging
- Combined security features
- Error scenarios

## Test Categories

### HMAC Signature Tests

#### `test_webhook_hmac_sha256_valid`
Verifies SHA256 HMAC signature validation works correctly.
- Enables HMAC with SHA256 algorithm
- Generates valid signature
- Confirms webhook is accepted (200 OK)

#### `test_webhook_hmac_sha512_valid`
Verifies SHA512 HMAC signature validation.
- Uses SHA512 algorithm
- Tests stronger hashing algorithm support

#### `test_webhook_hmac_invalid_signature`
Tests rejection of invalid signatures.
- Sends webhook with malformed signature
- Expects 401 Unauthorized response

#### `test_webhook_hmac_missing_signature`
Tests rejection when signature is required but missing.
- HMAC enabled but no signature header provided
- Expects 401 Unauthorized response

#### `test_webhook_hmac_wrong_secret`
Tests rejection when signature uses wrong secret.
- Generates signature with incorrect secret
- Expects 401 Unauthorized response

### Rate Limiting Tests

#### `test_webhook_rate_limit_enforced`
Verifies rate limiting prevents excessive requests.
- Configures limit of 3 requests per 60 seconds
- Sends 3 successful requests
- 4th request returns 429 Too Many Requests

#### `test_webhook_rate_limit_disabled`
Confirms webhooks work without rate limiting.
- Sends 10 consecutive requests
- All succeed when rate limiting disabled

### IP Whitelisting Tests

#### `test_webhook_ip_whitelist_allowed`
Tests IP whitelist allows configured IPs.
- Configures whitelist with CIDR and exact IP
- Tests both CIDR range match (192.168.1.100 in 192.168.1.0/24)
- Tests exact IP match (10.0.0.1)
- Both return 200 OK

#### `test_webhook_ip_whitelist_blocked`
Tests IP whitelist blocks non-whitelisted IPs.
- Sends request from IP not in whitelist (8.8.8.8)
- Expects 403 Forbidden response

### Payload Size Limit Tests

#### `test_webhook_payload_size_limit_enforced`
Verifies payload size limits are enforced.
- Sets 1 KB limit
- Sends 2 KB payload
- Expects 400 Bad Request response

#### `test_webhook_payload_size_within_limit`
Tests payloads within limit are accepted.
- Sets 10 KB limit
- Sends small payload
- Expects 200 OK response

### Event Logging Tests

#### `test_webhook_event_logging_success`
Verifies successful webhooks are logged.
- Sends successful webhook
- Checks log entry exists
- Validates log contains status code, IP, user agent

#### `test_webhook_event_logging_failure`
Verifies failed webhooks are logged.
- Sends failing webhook (HMAC failure)
- Checks log entry with error details
- Validates failure reason recorded

### Combined Security Tests

#### `test_webhook_all_security_features_pass`
Tests webhook passing all security checks.
- Enables HMAC, rate limiting, IP whitelist, size limit
- Sends properly authenticated, allowed webhook
- Verifies all checks pass and event created
- Validates log shows all features verified

#### `test_webhook_multiple_security_failures`
Tests multiple security feature failures.
- Enables multiple features
- Sends webhook failing multiple checks
- Verifies first failure prevents further processing

### Error Scenario Tests

#### `test_webhook_malformed_json`
Tests handling of malformed JSON payloads.
- Sends invalid JSON
- Expects 400 Bad Request response

#### `test_webhook_empty_payload`
Tests handling of empty payloads.
- Sends empty body
- Expects 400 Bad Request response

## Running Tests

### Run All Tests
```bash
cargo test --workspace
```

### Run Specific Test Suite
```bash
# Repository tests
cargo test -p attune-common webhook_tests

# API tests
cargo test -p attune-api webhook_api_tests

# Security tests
cargo test -p attune-api webhook_security_tests
```

### Run Database Tests
```bash
cargo test --workspace -- --test-threads=4
```

### Run Specific Test
```bash
cargo test -p attune-api test_webhook_hmac_sha256_valid -- --test-threads=4
```

## Test Environment Setup

Tests require:
1. **PostgreSQL Database** - Running on localhost:5432 (or configured via `DATABASE_URL`)
2. **Test User** - Username: `test_user`, Password: `test_password`
3. **Migrations Applied** - All database migrations must be up to date

### Setup Commands
```bash
# Set database URL
export DATABASE_URL="postgresql://postgres:postgres@localhost:5432/attune"

# Run migrations
sqlx migrate run

# Create test user (if not exists)
psql $DATABASE_URL -c "
INSERT INTO attune.identity (username, email, password_hash, enabled)
VALUES ('test_user', 'test@example.com', '$argon2id$...', true)
ON CONFLICT (username) DO NOTHING;
"
```

## Test Coverage Summary

| Feature | Test Count | Status |
|---------|-----------|--------|
| Basic Webhook Management | 8 | ✅ Complete |
| HMAC Verification | 5 | ✅ Complete |
| Rate Limiting | 2 | ✅ Complete |
| IP Whitelisting | 2 | ✅ Complete |
| Payload Size Limits | 2 | ✅ Complete |
| Event Logging | 2 | ✅ Complete |
| Combined Security | 2 | ✅ Complete |
| Error Scenarios | 2 | ✅ Complete |
| **Total** | **25** | **✅ Complete** |

## Security Test Matrix

| HMAC | Rate Limit | IP Whitelist | Size Limit | Expected Result |
|------|-----------|--------------|------------|-----------------|
| ✅ Valid | ✅ OK | ✅ Allowed | ✅ OK | 200 OK |
| ❌ Invalid | N/A | N/A | N/A | 401 Unauthorized |
| ⚠️ Missing | N/A | N/A | N/A | 401 Unauthorized |
| ✅ Valid | ❌ Exceeded | N/A | N/A | 429 Too Many Requests |
| ✅ Valid | ✅ OK | ❌ Blocked | N/A | 403 Forbidden |
| ✅ Valid | ✅ OK | ✅ Allowed | ❌ Too Large | 400 Bad Request |

## Known Test Limitations

1. **Rate Limit Window Tests** - Tests don't verify time-based window expiry (would require time manipulation)
2. **Concurrent Rate Limiting** - No tests for concurrent request handling
3. **IPv6 Whitelist** - Limited IPv6 testing coverage
4. **Webhook Retry** - Not yet implemented (Phase 4 feature)
5. **Performance Tests** - No load/stress tests included

## Future Test Additions

### Phase 4 Features (Planned)
- Webhook retry logic tests
- Payload transformation tests
- Multiple webhook keys per trigger
- Webhook health check tests
- Analytics and metrics tests

### Additional Coverage Needed
- Concurrent webhook processing
- Database connection failure handling
- Message queue failure scenarios
- Performance benchmarks
- Security penetration testing

## Debugging Failed Tests

### Common Issues

**Test fails with "Failed to connect to database"**
```bash
# Check database is running
pg_isready -h localhost -p 5432

# Verify DATABASE_URL
echo $DATABASE_URL

# Test connection
psql $DATABASE_URL -c "SELECT 1"
```

**Test fails with "Trigger not found"**
- Ensure migrations are up to date: `sqlx migrate run`
- Check schema exists: `psql $DATABASE_URL -c "\dn"`

**Test fails with "Authentication required"**
- Verify test user exists in database
- Check JWT_SECRET environment variable is set

**Rate limit test fails unexpectedly**
- Database state may have rate limit entries from previous tests
- Clear webhook_event_log table between test runs

## Manual Testing

For manual testing instructions, see [webhook-manual-testing.md](webhook-manual-testing.md).

## Continuous Integration

Tests are configured to run in CI/CD pipeline with:
- PostgreSQL service container
- Test database initialization
- Migration application
- Test user creation
- Parallel test execution

See `.github/workflows/test.yml` (if configured) for CI setup.

## Test Maintenance

### Adding New Tests
1. Follow existing test structure and naming conventions
2. Use test helpers (`setup_test_state`, `create_test_pack`, etc.)
3. Clean up test data after test completion
4. Mark as `#[ignore]` if requires external dependencies
5. Document test purpose and expected behavior
6. Add test to this documentation

### Updating Tests for New Features
1. Update relevant test files
2. Add new test categories if needed
3. Update test coverage summary
4. Update security test matrix if applicable
5. Document any new test requirements

---

**Last Updated:** 2025-01-20  
**Test Suite Version:** 1.0  
**Phase Completion:** Phase 3 ✅
