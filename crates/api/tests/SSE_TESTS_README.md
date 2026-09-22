# SSE Integration Tests

This directory contains integration tests for the Server-Sent Events (SSE) execution streaming functionality.

## Quick Start

```bash
# Run CI-friendly tests (no server required)
cargo test -p attune-api --test sse_execution_stream_tests

# Expected output:
# test result: ok. 2 passed; 0 failed; 3 ignored
```

## Overview

The SSE tests verify the complete real-time update pipeline:
1. PostgreSQL NOTIFY triggers fire on execution changes
2. API service listener receives notifications via LISTEN
3. Notifications are broadcast to SSE clients
4. Web UI receives real-time updates

## Test Categories

### 1. Database-Level Tests (No Server Required) ✅ CI-Friendly

These tests run automatically and do NOT require the API server:

```bash
# Run all non-ignored tests (CI/CD safe)
cargo test -p attune-api --test sse_execution_stream_tests

# Or specifically test PostgreSQL NOTIFY
cargo test -p attune-api test_postgresql_notify_trigger_fires -- --nocapture
```

**What they test:**
- ✅ PostgreSQL trigger fires on execution INSERT/UPDATE
- ✅ Notification payload structure is correct
- ✅ LISTEN/NOTIFY mechanism works
- ✅ Database-level integration is working

**Status**: These tests pass automatically in CI/CD

### 2. End-to-End SSE Tests (Server Required) 🚧 Manual Testing

These tests are **marked as `#[ignore]`** and require a running API service.
They are not run by default in CI/CD.

```bash
# Terminal 1: Start API service
cargo run -p attune-api -- -c config.test.yaml

# Terminal 2: Run ignored SSE tests
cargo test -p attune-api --test sse_execution_stream_tests -- --ignored --nocapture --test-threads=4

# Or run a specific test
cargo test -p attune-api test_sse_stream_receives_execution_updates -- --ignored --nocapture
```

**What they test:**
- 🔍 SSE endpoint receives notifications from PostgreSQL listener
- 🔍 Filtering by execution_id works correctly
- 🔍 Authentication is enforced
- 🔍 Multiple concurrent SSE connections work
- 🔍 Real-time updates are delivered instantly

**Status**: Manual verification only (marked `#[ignore]`)

## Test Files

- `sse_execution_stream_tests.rs` - Main SSE integration tests (539 lines)
- 5 comprehensive test cases covering the full SSE pipeline

## Test Structure

### Database Setup
Each test:
1. Creates a clean test database state
2. Sets up test pack and action
3. Creates test executions

### SSE Connection
Tests use `eventsource-client` crate to:
1. Connect to `/api/v1/executions/stream` endpoint
2. Authenticate with JWT token
3. Subscribe to execution updates
4. Verify received events

### Assertions
Tests verify:
- Correct event structure
- Proper filtering behavior
- Authentication requirements
- Real-time delivery (no polling delay)

## Running All Tests

```bash
# Terminal 1: Start API service
cargo run -p attune-api -- -c config.test.yaml

# Terminal 2: Run all SSE tests
cargo test -p attune-api --test sse_execution_stream_tests -- --test-threads=4 --nocapture

# Or run specific test
cargo test -p attune-api test_sse_stream_receives_execution_updates -- --nocapture
```

## Expected Output

### Default Test Run (CI/CD)

```
running 5 tests
test test_postgresql_notify_trigger_fires ... ok
test test_sse_stream_receives_execution_updates ... ignored
test test_sse_stream_filters_by_execution_id ... ignored
test test_sse_stream_all_executions ... ignored
test test_sse_stream_requires_authentication ... ok

test result: ok. 2 passed; 0 failed; 3 ignored
```

### Full Test Run (With Server Running)

```
running 5 tests
test test_postgresql_notify_trigger_fires ... ok
test test_sse_stream_receives_execution_updates ... ok
test test_sse_stream_filters_by_execution_id ... ok
test test_sse_stream_requires_authentication ... ok
test test_sse_stream_all_executions ... ok

test result: ok. 5 passed; 0 failed; 0 ignored
```

### PostgreSQL Notification Example

```json
{
  "entity_type": "execution",
  "entity_id": 123,
  "timestamp": "2026-01-19T05:02:14.188288+00:00",
  "data": {
    "id": 123,
    "status": "running",
    "action_id": 42,
    "action_ref": "test_sse_pack.test_action",
    "result": null,
    "created": "2026-01-19T05:02:13.982769+00:00",
    "updated": "2026-01-19T05:02:14.188288+00:00"
  }
}
```

## Troubleshooting

### Connection Refused Error

```
error trying to connect: tcp connect error: Connection refused
```

**Solution**: Make sure the API service is running on port 8080:
```bash
cargo run -p attune-api -- -c config.test.yaml
```

### Test Database Not Found

**Solution**: Create the test database:
```bash
createdb attune_test
sqlx migrate run --database-url postgresql://postgres:postgres@localhost:5432/attune_test
```

### Missing Migration

**Solution**: Apply the execution notify trigger migration:
```bash
psql postgresql://postgres:postgres@localhost:5432/attune_test < migrations/20260119000001_add_execution_notify_trigger.sql
```

### Tests Hang

**Cause**: Tests are waiting for SSE events that never arrive

**Debug steps:**
1. Check API service logs for PostgreSQL listener errors
2. Verify trigger exists: `\d+ attune.execution` in psql
3. Manually update execution and check notifications:
   ```sql
   UPDATE attune.execution SET status = 'running' WHERE id = 1;
   LISTEN attune_notifications;
   ```

## CI/CD Integration

### Recommended Approach (Default)

Run only the database-level tests in CI/CD:

```bash
# CI-friendly tests (no server required) ✅
cargo test -p attune-api --test sse_execution_stream_tests
```

This will:
- ✅ Run `test_postgresql_notify_trigger_fires` (database trigger verification)
- ✅ Run `test_sse_stream_requires_authentication` (auth logic verification)
- ⏭️ Skip 3 tests marked `#[ignore]` (require running server)

### Full Testing (Optional)

For complete end-to-end verification in CI/CD:

```bash
# Start API in background
cargo run -p attune-api -- -c config.test.yaml &
API_PID=$!

# Wait for server to start
sleep 3

# Run ALL tests including ignored ones
cargo test -p attune-api --test sse_execution_stream_tests -- --ignored --test-threads=4

# Cleanup
kill $API_PID
```

**Note**: Full testing adds complexity and time. The database-level tests provide
sufficient coverage for the notification pipeline. The ignored tests are for
manual verification during development.

## Related Documentation

- [SSE Architecture](../../docs/sse-architecture.md)
- [Web UI Integration](../../web/src/hooks/useExecutionStream.ts)
- [Session Summary](../../work-summary/session-09-web-ui-detail-pages.md)
