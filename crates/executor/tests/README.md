# Executor Integration Tests

This directory contains integration tests for the Attune executor service.

## Test Suites

### Policy Enforcer Tests (`policy_enforcer_tests.rs`)
Tests for policy enforcement including rate limiting, concurrency control, and quota management.

**Run**: `cargo test --test policy_enforcer_tests -- --test-threads=4`

### FIFO Ordering Integration Tests (`fifo_ordering_integration_test.rs`)
Comprehensive integration and stress tests for FIFO policy execution ordering.

**Run**: `cargo test --test fifo_ordering_integration_test -- --test-threads=4`

## Prerequisites

1. **PostgreSQL Running**:
   ```bash
   sudo systemctl start postgresql
   ```

2. **Database Migrations Applied**:
   ```bash
   cd /path/to/attune
   sqlx migrate run
   ```

3. **Configuration**:
   Ensure `config.development.yaml` has correct database URL or set:
   ```bash
   export ATTUNE__DATABASE__URL="postgresql://attune:attune@localhost/attune"
   ```

## Running Tests

### All Integration Tests
```bash
# Run all executor integration tests (except extreme stress)
cargo test -- --test-threads=4
```

### Individual Test Suites
```bash
# Policy enforcer tests
cargo test --test policy_enforcer_tests -- --test-threads=4

# FIFO ordering tests
cargo test --test fifo_ordering_integration_test -- --test-threads=4
```

### Individual Test with Output
```bash
# High concurrency stress test
cargo test --test fifo_ordering_integration_test test_high_concurrency_stress -- --ignored --nocapture

# Multiple workers simulation
cargo test --test fifo_ordering_integration_test test_multiple_workers_simulation -- --ignored --nocapture
```

### Extreme Stress Test (10k executions)
```bash
# This test takes 5-10 minutes - run separately
cargo test --test fifo_ordering_integration_test test_extreme_stress_10k_executions -- --ignored --nocapture --test-threads=4
```

## Test Organization

- **Unit Tests**: Located in `src/` files (e.g., `queue_manager.rs`)
- **Integration Tests**: Located in `tests/` directory
- All tests requiring database are marked with `#[ignore]`

## Important Notes

- Use at least `--test-threads=4`; fixtures own their database state and CI enforces this minimum
- Tests create unique data using timestamps to avoid conflicts
- All tests clean up their test data automatically
- Stress tests output progress messages and performance metrics

## Troubleshooting

### Database Connection Issues
```
Error: Failed to connect to database
```
**Solution**: Ensure PostgreSQL is running and connection URL is correct.

### Queue Full Errors
```
Error: Queue full (max length: 10000)
```
**Solution**: This is expected for `test_queue_full_rejection`. Other tests should not see this.

### Test Data Not Cleaned Up
If tests crash, manually clean up:
```sql
DELETE FROM attune.queue_stats WHERE action_id IN (
    SELECT id FROM attune.action WHERE pack IN (
        SELECT id FROM attune.pack WHERE ref LIKE 'fifo_test_pack_%'
    )
);
DELETE FROM attune.execution WHERE action IN (SELECT id FROM attune.action WHERE pack IN (SELECT id FROM attune.pack WHERE ref LIKE 'fifo_test_pack_%'));
DELETE FROM attune.action WHERE pack IN (SELECT id FROM attune.pack WHERE ref LIKE 'fifo_test_pack_%');
DELETE FROM attune.pack WHERE ref LIKE 'fifo_test_pack_%';
```

## Documentation

For detailed test descriptions and execution plans, see:
- `work-summary/2025-01-fifo-integration-tests.md`
- `docs/testing-status.md` (Executor Service section)
