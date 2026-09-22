# E2E Test Quick Start Guide

**Last Updated**: 2026-01-27  
**Status**: Ready for Testing

---

## Overview

This guide helps you quickly get started with running end-to-end (E2E) integration tests for the Attune platform.

### Test Structure

```
tests/
├── e2e/                    # New tiered test structure
│   ├── tier1/              # Core automation flows (MVP essential)
│   │   ├── test_t1_01_interval_timer.py
│   │   ├── test_t1_02_date_timer.py
│   │   └── test_t1_04_webhook_trigger.py
│   ├── tier2/              # Orchestration & data flow (coming soon)
│   └── tier3/              # Advanced features (coming soon)
├── helpers/                # Test utilities
│   ├── __init__.py
│   ├── client.py           # AttuneClient API wrapper
│   ├── polling.py          # Wait/poll utilities
│   └── fixtures.py         # Test data creators
├── fixtures/               # Test data
│   └── packs/
│       └── test_pack/
├── conftest.py             # Pytest configuration
├── pytest.ini              # Pytest settings
├── requirements.txt        # Python dependencies
└── run_e2e_tests.sh        # Test runner script
```

---

## Prerequisites

### 1. Services Running

All 5 Attune services must be running:

```bash
# Terminal 1 - API Service
cd crates/api
cargo run

# Terminal 2 - Executor Service
cd crates/executor
cargo run

# Terminal 3 - Worker Service
cd crates/worker
cargo run

# Terminal 4 - Sensor Service
cd crates/sensor
cargo run

# Terminal 5 - Notifier Service
cd crates/notifier
cargo run
```

### 2. Database & Message Queue

```bash
# PostgreSQL (if not already running)
docker run -d --name postgres \
  -e POSTGRES_PASSWORD=postgres \
  -p 5432:5432 \
  postgres:14

# RabbitMQ (if not already running)
docker run -d --name rabbitmq \
  -p 5672:5672 \
  -p 15672:15672 \
  rabbitmq:4.3.6-management-alpine
```

### 3. Database Migrations

```bash
# Ensure migrations are applied
sqlx migrate run
```

---

## Quick Start

### Option 1: Automated Runner (Recommended)

```bash
# Run all tests with automatic setup
cd tests
./run_e2e_tests.sh --setup

# Run specific tier
./run_e2e_tests.sh --tier 1

# Run with verbose output
./run_e2e_tests.sh --tier 1 -v

# Run and stop on first failure
./run_e2e_tests.sh --tier 1 -s
```

### Option 2: Direct Pytest

```bash
cd tests

# Install dependencies first (one-time setup)
python3 -m venv venvs/e2e
source venvs/e2e/bin/activate
pip install -r requirements.txt

# Run all Tier 1 tests
pytest e2e/tier1/ -v

# Run specific test
pytest e2e/tier1/test_t1_01_interval_timer.py -v

# Run by marker
pytest -m tier1 -v
pytest -m webhook -v
pytest -m timer -v

# Run with live output
pytest e2e/tier1/ -v -s
```

---

## Test Tiers

### Tier 1: Core Automation Flows ✅

**Status**: 3 tests implemented  
**Priority**: Critical (MVP)  
**Duration**: ~2 minutes total

Tests implemented:
- ✅ **T1.1**: Interval Timer Automation (30s)
- ✅ **T1.2**: Date Timer One-Shot Execution (15s)
- ✅ **T1.4**: Webhook Trigger with Payload (20s)

Tests pending:
- ⏳ **T1.3**: Cron Timer Execution
- ⏳ **T1.5**: Workflow with Array Iteration
- ⏳ **T1.6**: Key-Value Store Access
- ⏳ **T1.7**: Multi-Tenant Isolation
- ⏳ **T1.8**: Action Failure Handling

Run with:
```bash
./run_e2e_tests.sh --tier 1
```

### Tier 2: Orchestration & Data Flow ⏳

**Status**: Not yet implemented  
**Priority**: High  
**Tests**: Workflows, inquiries, error handling

Coming soon!

### Tier 3: Advanced Features ⏳

**Status**: Not yet implemented  
**Priority**: Medium  
**Tests**: Performance, security, edge cases

Coming soon!

---

## Example Test Run

```bash
$ cd tests
$ ./run_e2e_tests.sh --tier 1

╔════════════════════════════════════════════════════════╗
║  Attune E2E Integration Test Suite                    ║
╚════════════════════════════════════════════════════════╝

  Tier 1: Core Automation Flows (MVP Essential)
  Tests: Timers, Webhooks, Basic Workflows

ℹ Checking if Attune services are running...
✓ API service is running at http://localhost:8080

═══ Running Tier 1 Tests
ℹ Command: pytest e2e/tier1/ -v -m tier1

======================== test session starts =========================
platform linux -- Python 3.11.0, pytest-7.4.3
rootdir: /path/to/attune/tests
configfile: pytest.ini
testpaths: tests/e2e
plugins: timeout-2.1.0, asyncio-0.21.0

collected 6 items

e2e/tier1/test_t1_01_interval_timer.py::TestIntervalTimerAutomation::test_interval_timer_creates_executions PASSED
e2e/tier1/test_t1_01_interval_timer.py::TestIntervalTimerAutomation::test_interval_timer_precision PASSED
e2e/tier1/test_t1_02_date_timer.py::TestDateTimerAutomation::test_date_timer_fires_once PASSED
e2e/tier1/test_t1_02_date_timer.py::TestDateTimerAutomation::test_date_timer_past_date PASSED
e2e/tier1/test_t1_04_webhook_trigger.py::TestWebhookTrigger::test_webhook_trigger_with_payload PASSED
e2e/tier1/test_t1_04_webhook_trigger.py::TestWebhookTrigger::test_multiple_webhook_posts PASSED

======================= 6 passed in 85.32s ==========================

✓ All tests passed!

╔════════════════════════════════════════════════════════╗
║  ✓ All E2E tests passed successfully                  ║
╚════════════════════════════════════════════════════════╝
```

---

## Configuration

### Environment Variables

```bash
# API URL (default: http://localhost:8080)
export ATTUNE_API_URL="http://localhost:8080"

# Test timeout in seconds (default: 60)
export TEST_TIMEOUT="60"

# Test user credentials (optional)
export TEST_USER_LOGIN="test@attune.local"
export TEST_USER_PASSWORD="TestPass123!"
```

### pytest.ini Settings

Key configuration in `tests/pytest.ini`:
- Test discovery patterns
- Markers for test categorization
- Logging configuration
- Timeout settings

---

## Troubleshooting

### Services Not Running

**Error**: `API service is not reachable`

**Solution**:
1. Check all 5 services are running (see Prerequisites)
2. Verify API responds: `curl http://localhost:8080/health`
3. Check service logs for errors

### Tests Timing Out

**Error**: `TimeoutError: Execution did not reach status 'succeeded'`

**Possible Causes**:
- Executor service not running
- Worker service not consuming queue
- RabbitMQ connection issues
- Sensor service not detecting triggers

**Solution**:
1. Check all services are running: `ps aux | grep attune`
2. Check RabbitMQ queues: http://localhost:15672 (guest/guest)
3. Check database: `psql -d attune_dev -c "SELECT * FROM attune.execution ORDER BY created DESC LIMIT 5;"`
4. Increase timeout: `export TEST_TIMEOUT=120`

### Import Errors

**Error**: `ModuleNotFoundError: No module named 'helpers'`

**Solution**:
```bash
# Make sure you're in the tests directory
cd tests

# Activate venv
source venvs/e2e/bin/activate

# Install dependencies
pip install -r requirements.txt

# Set PYTHONPATH
export PYTHONPATH="$PWD:$PYTHONPATH"
```

### Database Issues

**Error**: `Database connection failed` or `Relation does not exist`

**Solution**:
```bash
# Verify database exists
psql -U postgres -l | grep attune

# Run migrations
cd /path/to/attune
sqlx migrate run

# Check tables exist
psql -d attune_dev -c "\dt attune.*"
```

### Test Isolation Issues

**Problem**: Tests interfere with each other

**Solution**:
- Use `unique_user_client` fixture for complete isolation
- Tests automatically get unique references via `unique_ref()`
- Each test creates its own resources (pack, trigger, action, rule)

### Flaky Timer Tests

**Problem**: Timer tests occasionally fail with timing issues

**Solution**:
- Timer tests have built-in tolerance (±1-2 seconds)
- System load can affect timing - run on idle system
- Increase poll intervals if needed
- Check sensor service logs for timer processing

---

## Writing New Tests

### Test Template

```python
#!/usr/bin/env python3
"""
T1.X: Test Name

Test description and flow.
"""

import pytest
from helpers import (
    AttuneClient,
    create_echo_action,
    create_rule,
    wait_for_execution_status,
)


@pytest.mark.tier1  # or tier2, tier3
@pytest.mark.integration
@pytest.mark.timeout(30)
class TestMyFeature:
    """Test my feature"""

    def test_my_scenario(self, client: AttuneClient, pack_ref: str):
        """Test that my scenario works"""
        
        print(f"\n=== T1.X: Test Name ===")
        
        # Step 1: Create resources
        print("\n[1/3] Creating resources...")
        action = create_echo_action(client=client, pack_ref=pack_ref)
        print(f"✓ Action created: {action['ref']}")
        
        # Step 2: Execute action
        print("\n[2/3] Executing action...")
        # ... test logic ...
        
        # Step 3: Verify results
        print("\n[3/3] Verifying results...")
        # ... assertions ...
        
        print("\n✓ Test PASSED")
```

### Available Helpers

**Fixtures** (conftest.py):
- `client` - Authenticated API client
- `unique_user_client` - Client with unique user (isolation)
- `test_pack` - Test pack fixture
- `pack_ref` - Pack reference string
- `wait_time` - Standard wait time dict

**Client Methods** (helpers/client.py):
- `client.register_pack(path)`
- `client.create_action(...)`
- `client.create_trigger(...)`
- `client.create_rule(...)`
- `client.fire_webhook(id, payload)`
- `client.list_executions(...)`
- `client.get_execution(id)`

**Polling Utilities** (helpers/polling.py):
- `wait_for_execution_status(client, id, status, timeout)`
- `wait_for_execution_count(client, count, ...)`
- `wait_for_event_count(client, count, ...)`
- `wait_for_condition(fn, timeout, ...)`

**Fixture Creators** (helpers/fixtures.py):
- `create_interval_timer(client, seconds, ...)`
- `create_date_timer(client, fire_at, ...)`
- `create_cron_timer(client, expression, ...)`
- `create_webhook_trigger(client, ...)`
- `create_echo_action(client, ...)`
- `create_rule(client, trigger_id, action_ref, ...)`

---

## Next Steps

1. **Run existing tests** to verify setup:
   ```bash
   ./run_e2e_tests.sh --tier 1
   ```

2. **Implement remaining Tier 1 tests**:
   - T1.3: Cron Timer
   - T1.5: Workflow with-items
   - T1.6: Datastore access
   - T1.7: Multi-tenancy
   - T1.8: Error handling

3. **Implement Tier 2 tests** (orchestration)

4. **Implement Tier 3 tests** (advanced features)

5. **CI/CD Integration**:
   - Add GitHub Actions workflow
   - Run tests on every PR
   - Generate test reports

---

## Resources

- **Test Plan**: `docs/e2e-test-plan.md` - Complete test specifications
- **Test Status**: `docs/testing-status.md` - Current testing coverage
- **API Docs**: `docs/api-*.md` - API endpoint documentation
- **Architecture**: `docs/` - System architecture documentation

---

## Support

If you encounter issues:

1. Check service logs in each terminal
2. Verify database state: `psql -d attune_dev`
3. Check RabbitMQ management UI: http://localhost:15672
4. Review test output for detailed error messages
5. Enable verbose output: `./run_e2e_tests.sh -v -s`

**Status**: Ready to run! 🚀
