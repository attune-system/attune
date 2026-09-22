# End-to-End Integration Testing

**Status**: 🔄 In Progress - Tier 3 (62% Complete)  
**Last Updated**: 2026-01-24  
**Purpose**: Comprehensive integration testing across all 5 Attune services

> **🆕 API Client Migration**: Tests now use auto-generated OpenAPI client for improved type safety and maintainability. See [`MIGRATION_TO_GENERATED_CLIENT.md`](MIGRATION_TO_GENERATED_CLIENT.md) for details.

**Test Coverage:**
- ✅ **Tier 1**: Complete (8 scenarios, 33 tests) - Core automation flows
- ✅ **Tier 2**: Complete (13 scenarios, 37 tests) - Orchestration & data flow
- 🔄 **Tier 3**: 62% Complete (13/21 scenarios, 40 tests) - Advanced features & edge cases

---

## Overview

This directory contains end-to-end integration tests that verify the complete Attune automation platform works correctly when all services are running together.

### API Client

Tests use an **auto-generated Python client** created from the Attune API's OpenAPI specification:
- **Generated Client**: `tests/generated_client/` - 71 endpoints, 200+ Pydantic models
- **Wrapper Client**: `tests/helpers/client_wrapper.py` - Backward-compatible interface
- **Benefits**: Type safety, automatic schema sync, reduced maintenance

For migration details and usage examples, see [`MIGRATION_TO_GENERATED_CLIENT.md`](MIGRATION_TO_GENERATED_CLIENT.md).

### Test Scope

**Services Under Test:**
1. **API Service** (`attune-api`) - REST API gateway
2. **Executor Service** (`attune-executor`) - Orchestration & scheduling
3. **Worker Service** (`attune-worker`) - Action execution
4. **Sensor Service** (`attune-sensor`) - Event monitoring
5. **Notifier Service** (`attune-notifier`) - Real-time notifications
6. **Supervisor Service** (`attune-supervisor`) - Runtime retention and maintenance

**External Dependencies:**
- PostgreSQL (database)
- RabbitMQ (message queue)

---

## Test Organization

Tests are organized into three tiers based on priority and complexity:

### **Tier 1: Core Automation Flows** ✅ COMPLETE
Essential MVP functionality - timer, webhook, workflow, datastore, multi-tenancy, failure handling.
- **Location**: `tests/e2e/tier1/`
- **Count**: 8 scenarios, 33 test functions
- **Duration**: ~4 minutes total

### **Tier 2: Orchestration & Data Flow** ✅ COMPLETE  
Advanced orchestration - nested workflows, datastore writes, criteria, inquiries, retry policies.
- **Location**: `tests/e2e/tier2/`
- **Count**: 13 scenarios, 37 test functions
- **Duration**: ~6 minutes total

### **Tier 3: Advanced Features & Edge Cases** 🔄 IN PROGRESS (62%)
Security, edge cases, notifications, container runner, log limits, crash recovery.
- **Location**: `tests/e2e/tier3/`
- **Count**: 13/21 scenarios complete, 40 test functions
- **Duration**: ~8 minutes (when complete)

**Completed T3 Scenarios:**
- T3.1: Date Timer with Past Date (3 tests) ⏱️
- T3.2: Timer Cancellation (3 tests) ⏱️
- T3.3: Multiple Concurrent Timers (3 tests) ⏱️
- T3.4: Webhook with Multiple Rules (2 tests) 🔗
- T3.5: Webhook with Rule Criteria Filtering (4 tests) 🎯
- T3.10: RBAC Permission Checks (4 tests) 🔒
- T3.11: System vs User Packs (4 tests) 🔒
- T3.13: Invalid Action Parameters (4 tests) ⚠️
- T3.14: Execution Completion Notifications (4 tests) 🔔
- T3.15: Inquiry Creation Notifications (4 tests) 🔔
- T3.17: Container Runner Execution (4 tests) 🐳
- T3.18: HTTP Runner Execution (4 tests) 🌐
- T3.20: Secret Injection Security (4 tests) 🔐
- T3.21: Action Log Size Limits (4 tests) 📝

**Remaining T3 Scenarios:**
- T3.6: Sensor-generated custom events
- T3.7: Complex workflow orchestration
- T3.8: Chained webhook triggers
- T3.9: Multi-step approval workflow
- T3.12: Worker crash recovery
- T3.16: Rule trigger notifications
- T3.19: Dependency conflict isolation

---

## Running Tests

### Quick Start

```bash
# Run all tests
./tests/run_e2e_tests.sh

# Run specific tier
pytest tests/e2e/tier1/
pytest tests/e2e/tier2/
pytest tests/e2e/tier3/

# Run by marker
pytest -m "tier1"
pytest -m "tier3 and notifications"
pytest -m "container"
```

### Prerequisites

1. **Start all services**:
   ```bash
   docker-compose up -d postgres rabbitmq
   cargo run --bin attune-api &
   cargo run --bin attune-executor &
   cargo run --bin attune-worker &
   cargo run --bin attune-sensor &
   cargo run --bin attune-notifier &
   ```

2. **Install test dependencies**:
   ```bash
   cd tests
   pip install -r requirements.txt
   ```

3. **Verify services are healthy**:
   ```bash
   curl http://localhost:8080/health
   ```

---

## Example Test Scenarios

### Scenario 1: Basic Timer Automation
**Duration**: ~30 seconds  
**Flow**: Timer → Event → Rule → Enforcement → Execution → Completion

**Steps:**
1. Create a pack via API
2. Create a timer trigger (fires every 10 seconds)
3. Create a simple echo action
4. Create a rule linking trigger to action
5. Sensor detects timer and generates event
6. Rule evaluates and creates enforcement
7. Executor schedules execution
8. Worker executes action
9. Verify execution completed successfully

**Success Criteria:**
- ✅ Event created within 10 seconds
- ✅ Enforcement created with correct rule_id
- ✅ Execution scheduled with correct action_ref
- ✅ Execution status progresses: requested → scheduled → running → succeeded
- ✅ Worker logs action output
- ✅ Completion notification sent back to executor
- ✅ No errors in any service logs

---

### Scenario 2: Workflow Execution
**Duration**: ~45 seconds  
**Flow**: Manual trigger → Workflow with 3 tasks → All tasks complete

**Steps:**
1. Create a workflow with sequential tasks:
   - Task 1: Echo "Starting workflow"
   - Task 2: Wait 2 seconds
   - Task 3: Echo "Workflow complete"
2. Trigger workflow execution via API
3. Monitor task execution order
4. Verify task outputs and variables

**Success Criteria:**
- ✅ Workflow execution created
- ✅ Tasks execute in correct order (sequential)
- ✅ Task 1 completes before Task 2 starts
- ✅ Task 2 completes before Task 3 starts
- ✅ Workflow variables propagate correctly
- ✅ Workflow status becomes 'succeeded'
- ✅ All task outputs captured

---

### Scenario 3: FIFO Queue Ordering
**Duration**: ~20 seconds  
**Flow**: Multiple executions with concurrency limit

**Steps:**
1. Create action with concurrency policy (max=1)
2. Submit 5 execution requests rapidly
3. Monitor execution order
4. Verify FIFO ordering maintained

**Success Criteria:**
- ✅ Executions enqueued in submission order
- ✅ Only 1 execution runs at a time
- ✅ Next execution starts after previous completes
- ✅ Queue stats accurate (queue_length, active_count)
- ✅ All 5 executions complete successfully
- ✅ Order preserved: exec1 → exec2 → exec3 → exec4 → exec5

---

### Scenario 4: Secret Management
**Duration**: ~15 seconds  
**Flow**: Action uses secrets securely

**Steps:**
1. Create a secret/key via API
2. Create action that uses the secret
3. Execute action
4. Verify secret injected via stdin (not env vars)
5. Check process environment doesn't contain secret

**Success Criteria:**
- ✅ Secret created and stored encrypted
- ✅ Worker retrieves secret for execution
- ✅ Secret passed via stdin to action
- ✅ Secret NOT in process environment
- ✅ Secret NOT in execution logs
- ✅ Action can access secret via get_secret() helper

---

### Scenario 5: Human-in-the-Loop (Inquiry)
**Duration**: ~30 seconds  
**Flow**: Action requests user input → Execution pauses → User responds → Execution resumes

**Steps:**
1. Create action that creates an inquiry
2. Execute action
3. Verify execution pauses with status 'paused'
4. Submit inquiry response via API
5. Verify execution resumes and completes

**Success Criteria:**
- ✅ Inquiry created with correct prompt
- ✅ Execution status changes to 'paused'
- ✅ Inquiry status is 'pending'
- ✅ Response submission updates inquiry
- ✅ Execution resumes after response
- ✅ Action receives response data
- ✅ Execution completes successfully

---

### Scenario 6: Error Handling & Recovery
**Duration**: ~25 seconds  
**Flow**: Action fails → Retry logic → Final failure

**Steps:**
1. Create action that always fails
2. Configure retry policy (max_retries=2)
3. Execute action
4. Monitor retry attempts
5. Verify final failure status

**Success Criteria:**
- ✅ Action fails on first attempt
- ✅ Executor retries execution
- ✅ Action fails on second attempt
- ✅ Executor retries again
- ✅ Action fails on third attempt
- ✅ Execution status becomes 'failed'
- ✅ Retry count accurate (3 total attempts)
- ✅ Error message captured

---

### Scenario 7: Real-Time Notifications
**Duration**: ~20 seconds  
**Flow**: Execution state changes → Notifications sent → WebSocket clients receive updates

**Steps:**
1. Connect WebSocket client to notifier
2. Create and execute action
3. Monitor notifications for state changes
4. Verify notification delivery

**Success Criteria:**
- ✅ WebSocket connection established
- ✅ Notification on execution created
- ✅ Notification on execution scheduled
- ✅ Notification on execution running
- ✅ Notification on execution succeeded
- ✅ All notifications contain correct entity_id
- ✅ Notifications delivered in real-time (<100ms)

---

### Scenario 8: Dependency Isolation
**Duration**: ~40 seconds  
**Flow**: Two packs with conflicting dependencies execute correctly

**Steps:**
1. Create Pack A with Python dependency: requests==2.25.0
2. Create Pack B with Python dependency: requests==2.28.0
3. Create actions in both packs
4. Execute both actions
5. Verify correct dependency versions used

**Success Criteria:**
- ✅ Pack A venv created with requests 2.25.0
- ✅ Pack B venv created with requests 2.28.0
- ✅ Pack A action uses correct venv
- ✅ Pack B action uses correct venv
- ✅ Both executions succeed
- ✅ No dependency conflicts

---

## Test Infrastructure

### Prerequisites

**Required Services:**
```bash
# PostgreSQL
docker run -d --name postgres \
  -e POSTGRES_PASSWORD=postgres \
  -p 5432:5432 \
  postgres:14

# RabbitMQ
docker run -d --name rabbitmq \
  -p 5672:5672 \
  -p 15672:15672 \
  rabbitmq:4.3.6-management-alpine

```

**Database Setup:**
```bash
# Create test database
createdb attune_e2e

# Run migrations
export DATABASE_URL="postgresql://postgres:postgres@localhost:5432/attune_e2e"
sqlx migrate run
```

### Service Configuration

**Config File**: `config.e2e.yaml`
```yaml
environment: test
packs_base_dir: ./tests/fixtures/packs

database:
  url: "postgresql://postgres:postgres@localhost:5432/attune_e2e"
  max_connections: 5

message_queue:
  url: "amqp://guest:guest@localhost:5672/%2F"
  
security:
  jwt_secret: "test-secret-for-e2e-testing-only"
  
server:
  host: "127.0.0.1"
  port: 18080  # Different port for E2E tests

worker:
  runtimes:
    - name: "python3"
      type: "python"
      python_path: "/usr/bin/python3"
    - name: "shell"
      type: "shell"
      shell_path: "/bin/bash"

executor:
  default_execution_timeout: 300
  
sensor:
  poll_interval_seconds: 5
  timer_precision_seconds: 1
```

---

## Running Tests

### Option 1: Manual Service Start

**Terminal 1 - API:**
```bash
cd crates/api
ATTUNE__CONFIG_FILE=../../config.e2e.yaml cargo run
```

**Terminal 2 - Executor:**
```bash
cd crates/executor
ATTUNE__CONFIG_FILE=../../config.e2e.yaml cargo run
```

**Terminal 3 - Worker:**
```bash
cd crates/worker
ATTUNE__CONFIG_FILE=../../config.e2e.yaml cargo run
```

**Terminal 4 - Sensor:**
```bash
cd crates/sensor
ATTUNE__CONFIG_FILE=../../config.e2e.yaml cargo run
```

**Terminal 5 - Notifier:**
```bash
cd crates/notifier
ATTUNE__CONFIG_FILE=../../config.e2e.yaml cargo run
```

**Terminal 6 - Run Tests:**
```bash
cd tests
cargo test --test e2e_*
```

---

### Option 2: Automated Test Runner (TODO)

```bash
# Start all services in background
./tests/scripts/start-services.sh

# Run tests
./tests/scripts/run-e2e-tests.sh

# Stop services
./tests/scripts/stop-services.sh
```

---

### Option 3: Docker Compose

```bash
# Full lifecycle: build, start stack, run tests, tear down
make e2e-test

# Run specific tier
make e2e-test-tier1
make e2e-test-tier2
make e2e-test-tier3

# Keep stack running after tests (for debugging failures)
make e2e-test-debug

# Pass additional pytest args
make e2e-test ARGS='-k "timer" -x'

# Or use the script directly
./scripts/run-integration-tests.sh --tier 1 --no-teardown
./scripts/run-integration-tests.sh -k "webhook" -x
```

**How it works:**
1. `docker-compose.e2e.yaml` adds an `e2e-tests` service (Python 3.12 container with test deps)
2. `scripts/run-integration-tests.sh` orchestrates the full lifecycle:
   - Builds the test runner image
   - Starts the entire Attune stack via Docker Compose
   - Waits for API health check
   - Runs pytest inside the container on the `attune-network`
   - Reports results and tears down (unless `--no-teardown`)
3. The test container has access to all services via Docker networking

---

## Test Implementation

### Test Structure

```
tests/
├── README.md                    # This file
├── config.e2e.yaml             # E2E test configuration
├── fixtures/                   # Test data
│   ├── packs/                  # Test packs
│   │   ├── test_pack/
│   │   │   ├── pack.yaml
│   │   │   ├── actions/
│   │   │   │   ├── echo.yaml
│   │   │   │   └── echo.py
│   │   │   └── workflows/
│   │   │       └── simple.yaml
│   └── seed_data.sql          # Initial test data
├── helpers/                    # Test utilities
│   ├── mod.rs
│   ├── api_client.rs          # API client wrapper
│   ├── service_manager.rs     # Start/stop services
│   └── assertions.rs          # Custom assertions
└── integration/               # Test files
    ├── test_timer_automation.rs
    ├── test_workflow_execution.rs
    ├── test_fifo_ordering.rs
    ├── test_secret_management.rs
    ├── test_inquiry_flow.rs
    ├── test_error_handling.rs
    ├── test_notifications.rs
    └── test_dependency_isolation.rs
```

---

## Debugging Failed Tests

### Check Service Logs

```bash
# API logs
tail -f logs/api.log

# Executor logs
tail -f logs/executor.log

# Worker logs
tail -f logs/worker.log

# Sensor logs
tail -f logs/sensor.log

# Notifier logs
tail -f logs/notifier.log
```

### Check Database State

```sql
-- Check executions
SELECT id, action_ref, status, created, updated 
FROM attune.execution 
ORDER BY created DESC 
LIMIT 10;

-- Check events
SELECT id, trigger, payload, created 
FROM attune.event 
ORDER BY created DESC 
LIMIT 10;

-- Check enforcements
SELECT id, rule, event, status, created 
FROM attune.enforcement 
ORDER BY created DESC 
LIMIT 10;

-- Check queue stats
SELECT action_id, queue_length, active_count, max_concurrent
FROM attune.queue_stats;
```

### Check Message Queue

```bash
# RabbitMQ Management UI
open http://localhost:15672
# Login: guest/guest

# Check queues
rabbitmqadmin list queues name messages

# Purge queue (if needed)
rabbitmqadmin purge queue name=executor.enforcement
```

---

## Common Issues

### Issue: Services can't connect to database
**Solution:**
- Verify PostgreSQL is running: `psql -U postgres -c "SELECT 1"`
- Check DATABASE_URL in config
- Ensure migrations ran: `sqlx migrate info`

### Issue: Services can't connect to RabbitMQ
**Solution:**
- Verify RabbitMQ is running: `rabbitmqctl status`
- Check message_queue URL in config
- Verify RabbitMQ user/vhost exists

### Issue: Worker can't execute actions
**Solution:**
- Check Python path in config
- Verify test pack exists: `ls tests/fixtures/packs/test_pack`
- Check worker logs for runtime errors

### Issue: Tests timeout
**Solution:**
- Increase timeout in test
- Check if services are actually running
- Verify message queue messages are being consumed

### Issue: Timer doesn't fire
**Solution:**
- Verify sensor service is running
- Check sensor poll interval in config
- Look for timer trigger in database: `SELECT * FROM attune.trigger WHERE type = 'timer'`

---

## Success Criteria

A successful integration test run should show:

✅ All services start without errors  
✅ Services establish database connections  
✅ Services connect to message queue  
✅ API endpoints respond correctly  
✅ Timer triggers fire on schedule  
✅ Events generate from triggers  
✅ Rules evaluate correctly  
✅ Enforcements create executions  
✅ Executions reach workers  
✅ Workers execute actions successfully  
✅ Results propagate back through system  
✅ Notifications delivered in real-time  
✅ All 8 test scenarios pass  
✅ No errors in service logs  
✅ Clean shutdown of all services  

---

## Next Steps

### Phase 1: Setup (Current)
- [x] Document test plan
- [ ] Create config.e2e.yaml
- [ ] Create test fixtures
- [ ] Set up test infrastructure

### Phase 2: Basic Tests
- [ ] Implement timer automation test
- [ ] Implement workflow execution test
- [ ] Implement FIFO ordering test

### Phase 3: Advanced Tests
- [ ] Implement secret management test
- [ ] Implement inquiry flow test
- [ ] Implement error handling test

### Phase 4: Real-time & Performance
- [ ] Implement notification test
- [ ] Implement dependency isolation test
- [ ] Add performance benchmarks

### Phase 5: Automation
- [ ] Create service start/stop scripts
- [ ] Create automated test runner
- [ ] Set up CI/CD integration

---

## Contributing

When adding new integration tests:

1. **Document the scenario** in this README
2. **Create test fixtures** if needed
3. **Write the test** with clear assertions
4. **Test locally** with all services running
5. **Update CI configuration** if needed

---

## Resources

- [Architecture Documentation](../docs/architecture.md)
- [Service Documentation](../docs/)
- [API Documentation](../docs/api-*.md)
- [Workflow Documentation](../docs/workflow-orchestration.md)
- [Queue Documentation](../docs/queue-architecture.md)

---

**Status**: 🔄 In Progress  
**Current Phase**: Phase 1 - Setup  
**Next Milestone**: First test scenario passing
