# End-to-End Test Plan

**Status**: 📋 Planning Phase  
**Last Updated**: 2026-01-27  
**Purpose**: Comprehensive test plan for validating complete automation flows across all Attune services

---

## Executive Summary

This document outlines the end-to-end (E2E) test strategy for the Attune automation platform. E2E tests validate the complete event flow from trigger detection through action execution, ensuring all five microservices work together correctly.

**Critical Event Flow:**
```
Sensor → Trigger fires → Event created → Rule evaluates → 
Enforcement created → Execution scheduled → Worker executes Action → 
Results captured → Notifications sent
```

**Test Priorities:**
- **Tier 1** (Core Flows): 8 tests - Basic automation lifecycle, essential for MVP
- **Tier 2** (Orchestration): 13 tests - Workflows, data flow, error handling
- **Tier 3** (Advanced): 19 tests - Edge cases, performance, advanced features

**Total Test Scenarios**: 40 comprehensive tests covering all platform capabilities

---

## Test Infrastructure Requirements

### Services Required

All E2E tests require the following services to be running:

1. **PostgreSQL 14+** - Main data store
2. **RabbitMQ 3.12+** - Message queue for inter-service communication
3. **attune-api** - REST API gateway (port 18080 for tests)
4. **attune-executor** - Execution orchestration
5. **attune-worker** - Action execution engine
6. **attune-sensor** - Event monitoring and trigger detection
7. **attune-notifier** - Real-time notifications (WebSocket)

### Test Environment Configuration

**Config File**: `config.e2e.yaml`
- Separate database: `attune_e2e`
- Test-specific ports to avoid conflicts
- Reduced timeouts for faster test execution
- Verbose logging for debugging
- Test fixtures directory: `tests/fixtures/packs/`

### Test Fixtures

**Location**: `tests/fixtures/`
- `packs/test_pack/` - Simple test pack with echo action
- `packs/workflow_pack/` - Pack with workflow definitions
- `packs/timer_pack/` - Pack with timer triggers
- `packs/webhook_pack/` - Pack with webhook triggers
- `secrets/` - Test secrets for secure value injection
- `seed_data.sql` - Baseline test data

---

## Test Tier Breakdown

### Tier 1: Core Automation Flows (MVP Essential)

These tests validate the fundamental automation lifecycle and must pass before MVP release.

#### T1.1: Interval Timer Automation
**Priority**: Critical  
**Duration**: ~30 seconds  
**Description**: Action executes repeatedly on interval timer

**Test Steps:**
1. Register test pack via API
2. Create interval timer trigger (every 5 seconds)
3. Create simple echo action
4. Create rule linking timer → action
5. Wait for 3 trigger events (15 seconds)
6. Verify 3 enforcements created
7. Verify 3 executions completed successfully

**Success Criteria:**
- ✅ Timer fires every 5 seconds (±500ms tolerance)
- ✅ Each timer event creates enforcement
- ✅ Each enforcement creates execution
- ✅ All executions reach 'succeeded' status
- ✅ Action output captured in execution results
- ✅ No errors in any service logs

**Dependencies**: Sensor, Executor, Worker services functional

---

#### T1.2: Date Timer (One-Shot Execution)
**Priority**: Critical  
**Duration**: ~10 seconds  
**Description**: Action executes once at specific future time

**Test Steps:**
1. Create date timer trigger (5 seconds from now)
2. Create action with unique marker output
3. Create rule linking timer → action
4. Wait 7 seconds
5. Verify exactly 1 execution occurred
6. Wait additional 10 seconds
7. Verify no additional executions

**Success Criteria:**
- ✅ Timer fires once at scheduled time (±1 second)
- ✅ Exactly 1 enforcement created
- ✅ Exactly 1 execution created
- ✅ No duplicate executions after timer expires
- ✅ Timer marked as expired/completed

**Edge Cases Tested:**
- Date in past (should execute immediately)
- Date timer cleanup after firing

---

#### T1.3: Cron Timer Execution
**Priority**: Critical  
**Duration**: ~70 seconds  
**Description**: Action executes on cron schedule

**Test Steps:**
1. Create cron timer trigger (at 0, 3, 6, 12 seconds of each minute)
2. Create action with timestamp output
3. Create rule linking timer → action
4. Wait for one minute + 15 seconds
5. Verify executions at correct second marks

**Success Criteria:**
- ✅ Executions occur at seconds: 0, 3, 6, 12 (first minute)
- ✅ Executions occur at seconds: 0, 3, 6, 12 (second minute if test runs long)
- ✅ No executions at other second marks
- ✅ Cron expression correctly parsed
- ✅ Timezone handling correct

**Cron Expression Examples to Test:**
- `*/5 * * * * *` - Every 5 seconds
- `0,3,6,12 * * * * *` - At specific seconds
- `0 * * * * *` - Top of every minute

---

#### T1.4: Webhook Trigger with Payload
**Priority**: Critical  
**Duration**: ~15 seconds  
**Description**: Webhook POST triggers action with payload data

**Test Steps:**
1. Create webhook trigger (generates unique URL)
2. Create action that echoes webhook payload
3. Create rule linking webhook → action
4. POST JSON payload to webhook URL
5. Verify event created with correct payload
6. Verify execution receives payload as parameters
7. Verify action output includes webhook data

**Success Criteria:**
- ✅ Webhook trigger generates unique URL (`/api/v1/webhooks/{trigger_id}`)
- ✅ POST to webhook creates event immediately
- ✅ Event payload matches POST body
- ✅ Rule evaluates and creates enforcement
- ✅ Execution receives webhook data as input
- ✅ Action can access webhook payload fields

**Test Payloads:**
```json
{
  "event_type": "user.signup",
  "user_id": 12345,
  "email": "test@example.com",
  "metadata": {
    "source": "web",
    "campaign": "spring2024"
  }
}
```

---

#### T1.5: Workflow with Array Iteration (with-items)
**Priority**: Critical  
**Duration**: ~20 seconds  
**Description**: Workflow action spawns child executions for array items

**Test Steps:**
1. Create workflow action with `with-items` on array parameter
2. Create rule to trigger workflow
3. Execute workflow with array: `["apple", "banana", "cherry"]`
4. Verify parent execution created
5. Verify 3 child executions created (one per item)
6. Verify each child receives single item as input
7. Verify parent completes after all children succeed

**Success Criteria:**
- ✅ Parent execution status: 'running' while children execute
- ✅ Exactly 3 child executions created
- ✅ Each child execution has `parent_execution_id` set
- ✅ Each child receives single item: "apple", "banana", "cherry"
- ✅ Children can run in parallel
- ✅ Parent status becomes 'succeeded' after all children succeed
- ✅ Child execution count matches array length

**Workflow Definition:**
```yaml
actions:
  - name: process_items
    runner_type: python3
    entry_point: actions/process.py
    parameters:
      items:
        type: array
        required: true
    with_items: "{{ items }}"
```

---

#### T1.6: Action Reads from Key-Value Store
**Priority**: Critical  
**Duration**: ~10 seconds  
**Description**: Action retrieves configuration value from datastore

**Test Steps:**
1. Create key-value pair via API: `{"key": "api_url", "value": "https://api.example.com"}`
2. Create action that reads from datastore
3. Execute action with datastore key parameter
4. Verify action retrieves correct value
5. Verify action output includes retrieved value

**Success Criteria:**
- ✅ Action can read from `attune.datastore_item` table
- ✅ Scoped to tenant/user (multi-tenancy)
- ✅ Non-existent keys return null (no error)
- ✅ Action receives value in expected format
- ✅ Encrypted values decrypted before passing to action

**API Endpoints Used:**
- `POST /api/v1/datastore` - Create key-value
- `GET /api/v1/datastore/{key}` - Retrieve value
- Action reads via worker's datastore helper

---

#### T1.7: Multi-Tenant Isolation
**Priority**: Critical  
**Duration**: ~20 seconds  
**Description**: Users cannot access other tenant's resources

**Test Steps:**
1. Create User A (tenant_id=1) and User B (tenant_id=2)
2. User A creates pack, action, rule
3. User B attempts to list User A's packs
4. Verify User B sees empty list
5. User B attempts to execute User A's action by ID
6. Verify request returns 404 or 403 error
7. User A can see and execute their own resources

**Success Criteria:**
- ✅ All API endpoints filter by tenant_id
- ✅ Cross-tenant resource access returns 404 (not 403 to avoid info leak)
- ✅ Executions scoped to tenant
- ✅ Events scoped to tenant
- ✅ Enforcements scoped to tenant
- ✅ Datastore scoped to tenant
- ✅ Secrets scoped to tenant

**Security Test Cases:**
- Direct ID manipulation (guessing IDs)
- SQL injection attempts
- JWT token manipulation

---

#### T1.8: Action Execution Failure Handling
**Priority**: Critical  
**Duration**: ~15 seconds  
**Description**: Failed action execution handled gracefully

**Test Steps:**
1. Create action that always exits with error (exit code 1)
2. Create rule to trigger action
3. Execute action
4. Verify execution status becomes 'failed'
5. Verify error message captured
6. Verify exit code recorded
7. Verify execution doesn't retry (no retry policy)

**Success Criteria:**
- ✅ Execution status: 'requested' → 'scheduled' → 'running' → 'failed'
- ✅ Exit code captured: `exit_code = 1`
- ✅ stderr captured in execution result
- ✅ Execution result includes error details
- ✅ Worker marks execution as failed
- ✅ Executor updates enforcement status
- ✅ System remains stable (no crashes)

**Test Action:**
```python
#!/usr/bin/env python3
import sys
print("Starting action...", file=sys.stderr)
sys.exit(1)  # Force failure
```

---

### Tier 2: Orchestration & Data Flow

These tests validate workflow orchestration, data passing, and error recovery mechanisms.

#### T2.1: Nested Workflow Execution
**Priority**: High  
**Duration**: ~30 seconds  
**Description**: Parent workflow calls child workflow (multi-level)

**Test Steps:**
1. Create child workflow with 2 tasks
2. Create parent workflow that calls child workflow
3. Execute parent workflow
4. Verify parent creates child execution
5. Verify child creates its own task executions
6. Verify all executions complete in correct order

**Success Criteria:**
- ✅ 3 execution levels: parent → child → grandchild tasks
- ✅ `parent_execution_id` chain correct
- ✅ Execution tree structure maintained
- ✅ Results propagate up from grandchildren to parent
- ✅ Parent waits for all descendants to complete

**Execution Hierarchy:**
```
Parent Workflow (execution_id=1)
└─ Child Workflow (execution_id=2, parent=1)
   ├─ Task 1 (execution_id=3, parent=2)
   └─ Task 2 (execution_id=4, parent=2)
```

---

#### T2.2: Workflow with Failure Handling
**Priority**: High  
**Duration**: ~25 seconds  
**Description**: Child execution fails, parent handles error

**Test Steps:**
1. Create workflow with 3 child actions
2. Configure second child to fail
3. Configure `on-failure` behavior (continue vs. abort)
4. Execute workflow
5. Verify second child fails
6. Verify first and third children succeed
7. Verify parent status based on policy

**Success Criteria:**
- ✅ First child completes successfully
- ✅ Second child fails as expected
- ✅ Policy `continue`: third child still executes
- ✅ Policy `abort`: third child never starts
- ✅ Parent status reflects policy: 'failed' (abort) or 'succeeded_with_errors' (continue)
- ✅ All execution statuses correct

**Failure Policies to Test:**
- `on_failure: abort` - Stop all subsequent tasks
- `on_failure: continue` - Continue with remaining tasks
- `on_failure: retry` - Retry failed task N times

---

#### T2.3: Action Writes to Key-Value Store
**Priority**: High  
**Duration**: ~15 seconds  
**Description**: Action writes value, subsequent action reads it

**Test Steps:**
1. Create Action A that writes to datastore
2. Create Action B that reads from datastore
3. Create workflow: Action A → Action B
4. Execute workflow with test data
5. Verify Action A writes value
6. Verify Action B reads same value
7. Verify data persists in database

**Success Criteria:**
- ✅ Action A can write to datastore via API or helper
- ✅ Value persisted to `attune.datastore_item` table
- ✅ Action B retrieves exact value written by Action A
- ✅ Values scoped to tenant
- ✅ Encryption applied if marked as secret
- ✅ TTL honored if specified

**Test Data Flow:**
```
Action A: write("config.api_url", "https://api.production.com")
Action B: url = read("config.api_url")  # Returns "https://api.production.com"
```

---

#### T2.4: Parameter Templating and Context
**Priority**: High  
**Duration**: ~20 seconds  
**Description**: Action uses Jinja2 templates to access execution context

**Test Steps:**
1. Create Action A that returns structured output
2. Create Action B with templated parameters: `{{ task_1.result.api_key }}`
3. Create workflow: Action A → Action B
4. Execute workflow
5. Verify Action B receives resolved parameter values
6. Verify template variables replaced correctly

**Success Criteria:**
- ✅ Context includes: `trigger.data`, `execution.params`, `task_N.result`
- ✅ Jinja2 expressions evaluated correctly
- ✅ Nested JSON paths resolved: `{{ event.data.user.email }}`
- ✅ Missing values handled gracefully (null or error)
- ✅ Template errors fail execution with clear message

**Template Examples:**
```yaml
parameters:
  user_email: "{{ trigger.data.user.email }}"
  api_url: "{{ datastore.config.api_url }}"
  previous_result: "{{ task_1.result.status }}"
  iteration_item: "{{ item }}"  # In with-items context
```

---

#### T2.5: Rule Criteria Evaluation
**Priority**: High  
**Duration**: ~20 seconds  
**Description**: Rule only fires when criteria match

**Test Steps:**
1. Create webhook trigger
2. Create rule with criteria: `{{ trigger.data.status == "critical" }}`
3. POST webhook with `status: "info"` → No execution
4. POST webhook with `status: "critical"` → Execution created
5. Verify only second webhook triggered action

**Success Criteria:**
- ✅ Rule criteria evaluated as Jinja2 expression
- ✅ Event created for both webhooks
- ✅ Enforcement only created when criteria true
- ✅ No execution for non-matching events
- ✅ Complex criteria work: `{{ trigger.data.value > datastore.threshold }}`

**Criteria Examples:**
```yaml
criteria: "{{ trigger.data.severity == 'high' }}"
criteria: "{{ trigger.data.count > 100 }}"
criteria: "{{ trigger.data.environment in ['prod', 'staging'] }}"
```

---

#### T2.6: Approval Workflow (Inquiry)
**Priority**: High  
**Duration**: ~30 seconds  
**Description**: Action creates inquiry, execution pauses until response

**Test Steps:**
1. Create action that creates inquiry (approval request)
2. Execute action
3. Verify execution status becomes 'paused'
4. Verify inquiry created with status 'pending'
5. Submit inquiry response via API
6. Verify execution resumes
7. Verify action receives response data
8. Verify execution completes successfully

**Success Criteria:**
- ✅ Execution pauses with status 'paused'
- ✅ Inquiry created in `attune.inquiry` table
- ✅ Inquiry timeout set (TTL)
- ✅ Response submission updates inquiry status
- ✅ Execution resumes after response
- ✅ Action receives response in structured format
- ✅ Timeout causes default action if no response

**Inquiry Types to Test:**
- Simple yes/no approval
- Multi-field form input
- Multiple choice selection
- Inquiry timeout with default value

---

#### T2.7: Inquiry Timeout Handling
**Priority**: Medium  
**Duration**: ~35 seconds  
**Description**: Inquiry expires after TTL, execution proceeds with default

**Test Steps:**
1. Create action with inquiry (TTL=5 seconds)
2. Set default response for timeout
3. Execute action
4. Do NOT respond to inquiry
5. Wait 7 seconds
6. Verify inquiry status becomes 'expired'
7. Verify execution resumes with default value
8. Verify execution completes successfully

**Success Criteria:**
- ✅ Inquiry expires after TTL seconds
- ✅ Status changes: 'pending' → 'expired'
- ✅ Execution receives default response
- ✅ Execution proceeds without user input
- ✅ Timeout event logged

---

#### T2.8: Retry Policy Execution
**Priority**: High  
**Duration**: ~30 seconds  
**Description**: Failed action retries with exponential backoff

**Test Steps:**
1. Create action that fails first 2 times, succeeds on 3rd
2. Configure retry policy: `max_retries=3, delay=2s, backoff=2.0`
3. Execute action
4. Verify execution fails twice
5. Verify delays between retries: ~2s, ~4s
6. Verify third attempt succeeds
7. Verify execution status becomes 'succeeded'

**Success Criteria:**
- ✅ Execution retried 3 times total
- ✅ Exponential backoff applied: 2s, 4s, 8s
- ✅ Each retry logged separately
- ✅ Execution succeeds on final retry
- ✅ Retry count tracked in execution metadata
- ✅ Max retries honored (stops after limit)

**Retry Configuration:**
```yaml
retry:
  max_attempts: 3
  delay_seconds: 2
  backoff_multiplier: 2.0
  max_delay_seconds: 60
```

---

#### T2.9: Execution Timeout Policy
**Priority**: High  
**Duration**: ~25 seconds  
**Description**: Long-running action killed after timeout

**Test Steps:**
1. Create action that sleeps for 60 seconds
2. Configure timeout policy: 5 seconds
3. Execute action
4. Verify execution starts
5. Wait 7 seconds
6. Verify worker kills action process
7. Verify execution status becomes 'failed'
8. Verify timeout error message recorded

**Success Criteria:**
- ✅ Action process killed after timeout
- ✅ Execution status: 'running' → 'failed'
- ✅ Error message indicates timeout
- ✅ Exit code indicates SIGTERM/SIGKILL
- ✅ Worker remains stable after kill
- ✅ No zombie processes

**Timeout Levels:**
- Action-level timeout (per action)
- Workflow-level timeout (entire workflow)
- System default timeout (fallback)

---

#### T2.10: Parallel Execution (with-items)
**Priority**: Medium  
**Duration**: ~20 seconds  
**Description**: Multiple child executions run concurrently

**Test Steps:**
1. Create action with 5-second sleep
2. Configure workflow with `with-items` on array of 5 items
3. Configure `concurrency: 5` (all parallel)
4. Execute workflow
5. Measure total execution time
6. Verify ~5 seconds total (not 25 seconds sequential)
7. Verify all 5 children ran concurrently

**Success Criteria:**
- ✅ All 5 child executions start immediately
- ✅ Total time ~5 seconds (parallel) not ~25 seconds (sequential)
- ✅ Worker handles concurrent executions
- ✅ No resource contention issues
- ✅ All children complete successfully

**Concurrency Limits to Test:**
- `concurrency: 1` - Sequential execution
- `concurrency: 3` - Limited parallelism
- `concurrency: unlimited` - No limit

---

#### T2.11: Sequential Workflow with Dependencies
**Priority**: Medium  
**Duration**: ~20 seconds  
**Description**: Tasks execute in order with `on-success` transitions

**Test Steps:**
1. Create workflow with 3 tasks:
   - Task A: outputs `{"step": 1}`
   - Task B: depends on A, outputs `{"step": 2}`
   - Task C: depends on B, outputs `{"step": 3}`
2. Execute workflow
3. Verify execution order: A → B → C
4. Verify B waits for A to complete
5. Verify C waits for B to complete

**Success Criteria:**
- ✅ Tasks execute in correct order
- ✅ No task starts before dependency completes
- ✅ Each task accesses previous task results
- ✅ Total execution time = sum of individual times
- ✅ Workflow status reflects sequential progress

**Workflow Definition:**
```yaml
tasks:
  - name: task_a
    action: core.echo
  - name: task_b
    action: core.echo
    depends_on: [task_a]
  - name: task_c
    action: core.echo
    depends_on: [task_b]
```

---

#### T2.12: Python Action with Dependencies
**Priority**: Medium  
**Duration**: ~30 seconds  
**Description**: Python action uses third-party packages

**Test Steps:**
1. Create pack with `requirements.txt`: `requests==2.28.0`
2. Create action that imports and uses requests library
3. Worker creates isolated virtualenv for pack
4. Execute action
5. Verify venv created at expected path
6. Verify action successfully imports requests
7. Verify action executes HTTP request

**Success Criteria:**
- ✅ Virtualenv created in `venvs/{pack_name}/`
- ✅ Dependencies installed from requirements.txt
- ✅ Action imports third-party packages
- ✅ Isolation prevents conflicts with other packs
- ✅ Venv cached for subsequent executions

**Pack Structure:**
```
test_pack/
├── pack.yaml
├── requirements.txt  # requests==2.28.0
└── actions/
    └── http_call.py  # import requests
```

---

#### T2.13: Node.js Action Execution
**Priority**: Medium  
**Duration**: ~25 seconds  
**Description**: JavaScript action executes with Node.js runtime

**Test Steps:**
1. Create pack with `package.json`: `{"dependencies": {"axios": "^1.0.0"}}`
2. Create Node.js action that requires axios
3. Worker installs npm dependencies
4. Execute action
5. Verify node_modules created
6. Verify action successfully requires axios
7. Verify action completes successfully

**Success Criteria:**
- ✅ npm install runs for pack dependencies
- ✅ node_modules created in pack directory
- ✅ Action can require packages
- ✅ Dependencies isolated per pack
- ✅ Worker supports Node.js runtime type

**Action Example:**
```javascript
const axios = require('axios');

async function run(params) {
  const response = await axios.get(params.url);
  return response.data;
}

module.exports = { run };
```

---

### Tier 3: Advanced Features & Edge Cases

These tests cover advanced scenarios, edge cases, and performance requirements.

#### T3.1: Date Timer with Past Date
**Priority**: Low  
**Duration**: ~5 seconds  
**Description**: Timer with past date executes immediately or fails gracefully

**Test Steps:**
1. Create date timer trigger with date 1 hour in past
2. Create action
3. Create rule linking timer → action
4. Verify behavior (execute immediately OR fail with clear error)

**Success Criteria:**
- ✅ Either: execution created immediately
- ✅ Or: rule creation fails with clear error message
- ✅ No silent failures
- ✅ Behavior documented and consistent

---

#### T3.2: Timer Cancellation
**Priority**: Low  
**Duration**: ~15 seconds  
**Description**: Disabled rule stops timer from executing

**Test Steps:**
1. Create interval timer (every 5 seconds)
2. Create rule (enabled=true)
3. Wait for 2 executions
4. Disable rule via API
5. Wait 15 seconds
6. Verify no additional executions occurred

**Success Criteria:**
- ✅ Disabling rule stops future executions
- ✅ In-flight executions complete normally
- ✅ Sensor stops generating events for disabled rules
- ✅ Re-enabling rule resumes executions

---

#### T3.3: Multiple Concurrent Timers
**Priority**: Low  
**Duration**: ~30 seconds  
**Description**: Multiple rules with different timers run independently

**Test Steps:**
1. Create 3 interval timers: 3s, 5s, 7s
2. Create 3 rules with unique actions
3. Wait 21 seconds (LCM of intervals)
4. Verify Timer A fired 7 times (every 3s)
5. Verify Timer B fired 4-5 times (every 5s)
6. Verify Timer C fired 3 times (every 7s)

**Success Criteria:**
- ✅ Timers don't interfere with each other
- ✅ Each timer fires on its own schedule
- ✅ Sensor handles multiple concurrent timers
- ✅ No timer drift over time

---

#### T3.4: Webhook with Multiple Rules
**Priority**: Low  
**Duration**: ~15 seconds  
**Description**: Single webhook trigger fires multiple rules

**Test Steps:**
1. Create 1 webhook trigger
2. Create 3 rules, all using same webhook trigger
3. POST to webhook URL
4. Verify 1 event created
5. Verify 3 enforcements created (one per rule)
6. Verify 3 executions created
7. Verify all executions succeed

**Success Criteria:**
- ✅ Single event triggers multiple rules
- ✅ Rules evaluated independently
- ✅ Execution count = rule count
- ✅ All rules see same event payload

---

#### T3.5: Webhook with Rule Criteria Filtering
**Priority**: Medium  
**Duration**: ~20 seconds  
**Description**: Multiple rules with different criteria on same trigger

**Test Steps:**
1. Create webhook trigger
2. Create Rule A: criteria `{{ trigger.data.level == 'info' }}`
3. Create Rule B: criteria `{{ trigger.data.level == 'error' }}`
4. POST webhook with `level: 'info'` → only Rule A fires
5. POST webhook with `level: 'error'` → only Rule B fires
6. POST webhook with `level: 'debug'` → no rules fire

**Success Criteria:**
- ✅ Event created for all webhooks
- ✅ Only matching rules create enforcements
- ✅ Non-matching rules don't execute
- ✅ Multiple criteria evaluated correctly

---

#### T3.6: Sensor-Generated Custom Event
**Priority**: Low  
**Duration**: ~30 seconds  
**Description**: Custom sensor monitors external system and generates events

**Test Steps:**
1. Create custom sensor (polls file for changes)
2. Deploy sensor code to sensor service
3. Create trigger for sensor event type
4. Create rule linked to trigger
5. Modify monitored file
6. Verify sensor detects change
7. Verify event generated
8. Verify execution triggered

**Success Criteria:**
- ✅ Custom sensor code loaded dynamically
- ✅ Sensor polls on configured interval
- ✅ Sensor generates event when condition met
- ✅ Event payload includes sensor data
- ✅ Rule evaluates and triggers execution

---

#### T3.7: Complex Workflow Orchestration
**Priority**: Medium  
**Duration**: ~45 seconds  
**Description**: Full automation loop with multiple stages

**Test Steps:**
1. Webhook triggers initial action
2. Action checks datastore for threshold
3. If threshold exceeded, create inquiry (approval)
4. After approval, execute multi-step workflow
5. Workflow updates datastore with results
6. Final action sends notification

**Success Criteria:**
- ✅ All stages execute in correct order
- ✅ Data flows through entire pipeline
- ✅ Conditional logic works correctly
- ✅ Inquiry pauses execution
- ✅ Datastore updates persist
- ✅ Notification delivered

**Flow Diagram:**
```
Webhook → Check Threshold → Inquiry → Multi-Step Workflow → Update Datastore → Notify
```

---

#### T3.8: Chained Webhook Triggers
**Priority**: Low  
**Duration**: ~20 seconds  
**Description**: Action completion triggers webhook, which triggers next action

**Test Steps:**
1. Create Action A that POSTs to webhook URL on completion
2. Create Webhook Trigger B
3. Create Rule B: Webhook B → Action B
4. Execute Action A
5. Verify Action A completes
6. Verify Action A POSTs to Webhook B
7. Verify Webhook B creates event
8. Verify Action B executes

**Success Criteria:**
- ✅ Action can trigger webhooks programmatically
- ✅ Webhook event created from action POST
- ✅ Downstream rule fires correctly
- ✅ No circular dependencies causing infinite loops

---

#### T3.9: Multi-Step Approval Workflow
**Priority**: Low  
**Duration**: ~60 seconds  
**Description**: Workflow pauses twice for different approvals

**Test Steps:**
1. Create workflow with 2 inquiry steps:
   - Inquiry A: Manager approval
   - Inquiry B: Security approval
2. Execute workflow
3. Verify first pause at Inquiry A
4. Respond to Inquiry A
5. Verify workflow continues
6. Verify second pause at Inquiry B
7. Respond to Inquiry B
8. Verify workflow completes

**Success Criteria:**
- ✅ Workflow pauses at each inquiry
- ✅ Workflow resumes after each response
- ✅ Multiple inquiries handled correctly
- ✅ Inquiry responses accessible in subsequent tasks

---

#### T3.10: RBAC Permission Checks
**Priority**: Medium  
**Duration**: ~20 seconds  
**Description**: User with viewer role cannot create/execute actions

**Test Steps:**
1. Create User A with role 'admin'
2. Create User B with role 'viewer'
3. User A creates pack and action successfully
4. User B attempts to create action → 403 Forbidden
5. User B attempts to execute action → 403 Forbidden
6. User B can view (GET) actions successfully

**Success Criteria:**
- ✅ Role permissions enforced on all endpoints
- ✅ Viewer role: GET only, no POST/PUT/DELETE
- ✅ Admin role: Full CRUD access
- ✅ Clear error messages for permission denials
- ✅ Permissions checked before processing request

**Roles to Test:**
- `admin` - Full access
- `editor` - Create/update resources
- `viewer` - Read-only access
- `executor` - Execute actions only

---

#### T3.11: System vs User Packs
**Priority**: Medium  
**Duration**: ~15 seconds  
**Description**: System packs available to all tenants

**Test Steps:**
1. Install system pack (tenant_id=NULL or special marker)
2. Create User A (tenant_id=1)
3. Create User B (tenant_id=2)
4. Both users list packs
5. Verify both see system pack
6. Verify users only see their own user packs
7. Both users can execute system pack actions

**Success Criteria:**
- ✅ System packs visible to all tenants
- ✅ System packs executable by all tenants
- ✅ User packs isolated per tenant
- ✅ System pack actions use shared venv
- ✅ Core pack is system pack

---

#### T3.12: Worker Crash Recovery
**Priority**: Medium  
**Duration**: ~30 seconds  
**Description**: Killed worker process triggers execution rescheduling

**Test Steps:**
1. Start execution of long-running action (30 seconds)
2. After 5 seconds, kill worker process (SIGKILL)
3. Verify execution stuck in 'running' state
4. Executor detects timeout or heartbeat failure
5. Verify executor marks execution as 'failed'
6. Verify execution can be retried

**Success Criteria:**
- ✅ Executor detects worker failure
- ✅ Execution marked as failed with clear error
- ✅ No executions lost due to crash
- ✅ New worker can pick up work
- ✅ System recovers automatically

**Recovery Mechanisms:**
- Execution heartbeat monitoring
- Timeout detection
- Queue message redelivery

---

#### T3.13: Invalid Action Parameters
**Priority**: Medium  
**Duration**: ~5 seconds  
**Description**: Missing required parameter fails execution immediately

**Test Steps:**
1. Create action with required parameter: `url`
2. Create rule with action parameters missing `url`
3. Execute action
4. Verify execution fails immediately (not sent to worker)
5. Verify clear validation error message

**Success Criteria:**
- ✅ Parameter validation before worker scheduling
- ✅ Clear error: "Missing required parameter: url"
- ✅ Execution status: 'requested' → 'failed' (skips worker)
- ✅ No resources wasted on invalid execution
- ✅ Validation uses JSON Schema from action definition

---

#### T3.14: Execution Completion Notification
**Priority**: Medium  
**Duration**: ~20 seconds  
**Description**: WebSocket client receives real-time execution updates

**Test Steps:**
1. Connect WebSocket client to notifier
2. Subscribe to execution events
3. Create and execute action
4. Verify WebSocket receives messages:
   - Execution created
   - Execution scheduled
   - Execution running
   - Execution succeeded
5. Verify message format and payload

**Success Criteria:**
- ✅ WebSocket connection established
- ✅ All status transitions notified
- ✅ Notification latency <100ms
- ✅ Message includes full execution object
- ✅ Notifications scoped to tenant

**Notification Format:**
```json
{
  "event": "execution.status_changed",
  "entity_type": "execution",
  "entity_id": 123,
  "data": {
    "execution_id": 123,
    "status": "succeeded",
    "action_ref": "core.echo"
  },
  "timestamp": "2026-01-27T10:30:00Z"
}
```

---

#### T3.15: Inquiry Creation Notification
**Priority**: Low  
**Duration**: ~15 seconds  
**Description**: Real-time notification when inquiry created

**Test Steps:**
1. Connect WebSocket client
2. Subscribe to inquiry events
3. Execute action that creates inquiry
4. Verify WebSocket receives inquiry.created message
5. Respond to inquiry via API
6. Verify WebSocket receives inquiry.responded message

**Success Criteria:**
- ✅ Inquiry creation notified immediately
- ✅ Inquiry response notified immediately
- ✅ Notification includes inquiry details
- ✅ UI can show real-time approval requests

---

#### T3.16: Rule Trigger Notification (Optional)
**Priority**: Low  
**Duration**: ~15 seconds  
**Description**: Optional notification when specific rule fires

**Test Steps:**
1. Create rule with `notify_on_trigger: true`
2. Connect WebSocket client
3. Trigger rule via webhook
4. Verify WebSocket receives rule.triggered notification

**Success Criteria:**
- ✅ Notification only sent if enabled on rule
- ✅ Notification includes event details
- ✅ Notification scoped to tenant
- ✅ High-frequency rules don't flood notifications

---

#### T3.17: Container Runner Execution
**Priority**: Low  
**Duration**: ~40 seconds  
**Description**: Action executes inside Docker container

**Test Steps:**
1. Create action with runner_type: 'container'
2. Specify Docker image: 'python:3.11-slim'
3. Execute action
4. Worker pulls image if not cached
5. Worker starts container with action code
6. Verify action executes in container
7. Verify container cleaned up after execution

**Success Criteria:**
- ✅ Docker image pulled (cached for future runs)
- ✅ Container started with correct image
- ✅ Action code mounted into container
- ✅ Execution succeeds in container
- ✅ Container stopped and removed after execution
- ✅ No container leaks

**Security Considerations:**
- Container resource limits (CPU, memory)
- Network isolation
- No privileged mode

---

#### T3.18: HTTP Runner Execution
**Priority**: Medium  
**Duration**: ~10 seconds  
**Description**: HTTP action makes REST API call

**Test Steps:**
1. Create action with runner_type: 'http'
2. Configure action: method=POST, url, headers, body
3. Set up mock HTTP server to receive request
4. Execute action
5. Verify worker makes HTTP request
6. Verify response captured in execution result

**Success Criteria:**
- ✅ Worker makes HTTP request with correct method
- ✅ Headers passed correctly
- ✅ Body templated with parameters
- ✅ Response status code captured
- ✅ Response body captured
- ✅ HTTP errors handled gracefully

**Action Configuration:**
```yaml
name: api_call
runner_type: http
http_config:
  method: POST
  url: "https://api.example.com/users"
  headers:
    Content-Type: "application/json"
    Authorization: "Bearer {{ secret.api_token }}"
  body: "{{ params | tojson }}"
```

---

#### T3.19: Dependency Conflict Isolation
**Priority**: Low  
**Duration**: ~50 seconds  
**Description**: Two packs with conflicting dependencies run successfully

**Test Steps:**
1. Create Pack A: requires `requests==2.25.0`
2. Create Pack B: requires `requests==2.28.0`
3. Create actions in both packs that import requests
4. Execute Action A
5. Verify requests 2.25.0 used
6. Execute Action B
7. Verify requests 2.28.0 used
8. Execute both concurrently
9. Verify no conflicts

**Success Criteria:**
- ✅ Separate virtualenvs per pack
- ✅ Pack A uses requests 2.25.0
- ✅ Pack B uses requests 2.28.0
- ✅ Concurrent executions don't interfere
- ✅ Dependencies isolated completely

---

#### T3.20: Secret Injection Security
**Priority**: High  
**Duration**: ~20 seconds  
**Description**: Secrets passed via stdin, not environment variables

**Test Steps:**
1. Create secret via API: `{"key": "api_key", "value": "secret123"}`
2. Create action that uses secret
3. Execute action
4. Verify secret passed to action via stdin
5. Inspect worker process environment
6. Verify secret NOT in environment variables
7. Verify secret NOT in execution logs

**Success Criteria:**
- ✅ Secret passed via stdin (secure channel)
- ✅ Secret NOT in env vars (`/proc/{pid}/environ`)
- ✅ Secret NOT in process command line
- ✅ Secret NOT in execution output/logs
- ✅ Secret retrieved from database encrypted
- ✅ Action receives secret securely

**Security Rationale:**
- Environment variables visible in `/proc/{pid}/environ`
- Stdin not exposed to other processes
- Prevents secret leakage via ps/top

---

#### T3.21: Action Log Size Limits
**Priority**: Low  
**Duration**: ~15 seconds  
**Description**: Large action output truncated to prevent database bloat

**Test Steps:**
1. Create action that outputs 10MB of data
2. Configure log size limit: 1MB
3. Execute action
4. Verify execution captures first 1MB
5. Verify truncation marker added
6. Verify database record reasonable size

**Success Criteria:**
- ✅ Output truncated at configured limit
- ✅ Truncation indicator added: "... (output truncated)"
- ✅ Execution doesn't fail due to large output
- ✅ Database write succeeds
- ✅ Worker memory usage bounded

**Configuration:**
```yaml
worker:
  max_output_size_bytes: 1048576  # 1MB
  output_truncation_message: "... (output truncated after 1MB)"
```

---

#### T3.22: Execution History Pagination
**Priority**: Low  
**Duration**: ~30 seconds  
**Description**: Large execution lists paginated correctly

**Test Steps:**
1. Create 100 executions rapidly
2. Query executions with limit=20
3. Verify first page returns 20 executions
4. Verify pagination metadata (total, next_page)
5. Request next page
6. Verify next 20 executions returned
7. Iterate through all pages

**Success Criteria:**
- ✅ Pagination parameters: limit, offset
- ✅ Total count accurate
- ✅ No duplicate executions across pages
- ✅ No missing executions
- ✅ Consistent ordering (created_desc)

**API Query:**
```
GET /api/v1/executions?limit=20&offset=0
GET /api/v1/executions?limit=20&offset=20
```

---

#### T3.23: Execution Cancellation
**Priority**: Medium  
**Duration**: ~20 seconds  
**Description**: User cancels running execution

**Test Steps:**
1. Start long-running action (60 seconds)
2. After 5 seconds, cancel via API: `POST /api/v1/executions/{id}/cancel`
3. Verify executor sends cancel message to worker
4. Verify worker kills action process (SIGTERM)
5. Verify execution status becomes 'canceled'
6. Verify graceful shutdown (cleanup runs)

**Success Criteria:**
- ✅ Cancel request accepted while execution running
- ✅ Worker receives cancel message
- ✅ Action process receives SIGTERM
- ✅ Execution status: 'running' → 'canceled'
- ✅ Partial results captured
- ✅ Resources cleaned up

**Graceful Shutdown:**
- SIGTERM sent first (30 second grace period)
- SIGKILL sent if process doesn't exit

---

#### T3.24: High-Frequency Trigger Performance
**Priority**: Low  
**Duration**: ~60 seconds  
**Description**: Timer firing every second handled efficiently

**Test Steps:**
1. Create interval timer: every 1 second
2. Create simple action (echo)
3. Create rule
4. Let system run for 60 seconds
5. Verify ~60 executions created
6. Verify no backlog buildup
7. Verify system remains responsive

**Success Criteria:**
- ✅ 60 executions in 60 seconds (±5)
- ✅ Message queue doesn't accumulate backlog
- ✅ Worker keeps up with execution rate
- ✅ API remains responsive
- ✅ No memory leaks
- ✅ CPU usage reasonable

**Performance Targets:**
- Queue latency <100ms
- Execution scheduling latency <500ms
- API p95 response time <100ms

---

#### T3.25: Large Workflow (100+ Tasks)
**Priority**: Low  
**Duration**: ~60 seconds  
**Description**: Workflow with many tasks executes correctly

**Test Steps:**
1. Create workflow with 100 sequential tasks
2. Each task echoes its task number
3. Execute workflow
4. Monitor execution tree creation
5. Verify all 100 tasks execute in order
6. Verify workflow completes successfully
7. Verify reasonable memory usage

**Success Criteria:**
- ✅ All 100 tasks execute
- ✅ Correct sequential order maintained
- ✅ Execution tree correct (parent-child relationships)
- ✅ Memory usage scales linearly
- ✅ Database handles large execution count
- ✅ No stack overflow or recursion issues

---

#### T3.26: Pack Update/Reload
**Priority**: Low  
**Duration**: ~30 seconds  
**Description**: Pack update reloads actions without restart

**Test Steps:**
1. Register pack version 1.0.0
2. Execute action from pack
3. Update pack to version 1.1.0 (modify action code)
4. Reload pack via API: `POST /api/v1/packs/{id}/reload`
5. Execute action again
6. Verify updated code executed
7. Verify no service restart required

**Success Criteria:**
- ✅ Pack reload picks up code changes
- ✅ Virtualenv updated with new dependencies
- ✅ In-flight executions complete with old code
- ✅ New executions use new code
- ✅ No downtime during reload

---

#### T3.27: Datastore Encryption at Rest
**Priority**: Low  
**Duration**: ~10 seconds  
**Description**: Encrypted datastore values stored encrypted

**Test Steps:**
1. Create encrypted datastore value: `{"key": "password", "value": "secret", "encrypted": true}`
2. Query database directly
3. Verify value column contains encrypted blob (not plaintext)
4. Read value via API
5. Verify API returns decrypted value
6. Verify action receives decrypted value

**Success Criteria:**
- ✅ Encrypted values not visible in database
- ✅ Encryption key not stored in database
- ✅ API decrypts transparently
- ✅ Actions receive plaintext values
- ✅ Encryption algorithm documented (AES-256-GCM)

---

#### T3.28: Execution Audit Trail
**Priority**: Low  
**Duration**: ~15 seconds  
**Description**: Complete audit trail for execution lifecycle

**Test Steps:**
1. Execute action
2. Query audit log API
3. Verify audit entries for:
   - Execution created (by user X)
   - Execution scheduled (by executor)
   - Execution started (by worker Y)
   - Execution completed (by worker Y)
4. Verify each entry has timestamp, actor, action

**Success Criteria:**
- ✅ All lifecycle events audited
- ✅ Actor identified (user, service, worker)
- ✅ Timestamps accurate
- ✅ Audit log immutable
- ✅ Audit log queryable by execution_id

---

#### T3.29: Rate Limiting
**Priority**: Low  
**Duration**: ~30 seconds  
**Description**: API rate limiting prevents abuse

**Test Steps:**
1. Configure rate limit: 10 requests/second per user
2. Make 100 requests rapidly
3. Verify first 10 succeed
4. Verify subsequent requests return 429 Too Many Requests
5. Wait 1 second
6. Verify next 10 requests succeed

**Success Criteria:**
- ✅ Rate limit enforced per user
- ✅ 429 status code returned
- ✅ Retry-After header provided
- ✅ Rate limit resets after window
- ✅ Admin users exempt from rate limiting

---

#### T3.30: Graceful Service Shutdown
**Priority**: Low  
**Duration**: ~30 seconds  
**Description**: Services shutdown cleanly without data loss

**Test Steps:**
1. Start execution of long-running action
2. Send SIGTERM to worker service
3. Verify worker finishes current execution
4. Verify worker stops accepting new executions
5. Verify worker exits cleanly after completion
6. Verify execution results saved

**Success Criteria:**
- ✅ SIGTERM triggers graceful shutdown
- ✅ In-flight work completes
- ✅ No new work accepted
- ✅ Message queue messages requeued
- ✅ Database connections closed cleanly
- ✅ Exit code 0

---

## Test Execution Strategy

### Test Ordering

**Phase 1: Foundation (Run First)**
- T1.1-T1.8: Core flows - Must all pass before proceeding

**Phase 2: Orchestration**
- T2.1-T2.13: Workflow and data flow tests

**Phase 3: Integration**
- T3.1-T3.15: Advanced features and edge cases

**Phase 4: Performance & Scale**
- T3.16-T3.30: Performance, security, and operational tests

### Test Environment Setup

**Prerequisites:**
1. PostgreSQL 14+ running
2. RabbitMQ 3.12+ running
3. Test database created: `attune_e2e`
4. Migrations applied
5. Test configuration: `config.e2e.yaml`
6. Test fixtures loaded

**Service Startup Order:**
1. API service (port 18080)
2. Executor service
3. Worker service
4. Sensor service
5. Notifier service

### Automated Test Runner

**Script**: `tests/run_e2e_tests.sh`

```bash
#!/bin/bash
set -e

echo "=== Attune E2E Test Suite ==="

# 1. Setup
echo "[1/7] Setting up test environment..."
./tests/scripts/setup-test-env.sh

# 2. Start services
echo "[2/7] Starting services..."
./tests/scripts/start-services.sh

# 3. Wait for services
echo "[3/7] Waiting for services to be ready..."
./tests/scripts/wait-for-services.sh

# 4. Run Tier 1 tests
echo "[4/7] Running Tier 1 tests (Core Flows)..."
pytest tests/e2e/tier1/ -v

# 5. Run Tier 2 tests
echo "[5/7] Running Tier 2 tests (Orchestration)..."
pytest tests/e2e/tier2/ -v

# 6. Run Tier 3 tests
echo "[6/7] Running Tier 3 tests (Advanced)..."
pytest tests/e2e/tier3/ -v

# 7. Cleanup
echo "[7/7] Cleaning up..."
./tests/scripts/stop-services.sh

echo "=== Test Suite Complete ==="
```

### CI/CD Integration

**GitHub Actions Workflow:**
```yaml
name: E2E Tests

on: [push, pull_request]

jobs:
  e2e:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:14
        env:
          POSTGRES_PASSWORD: postgres
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5
      
      rabbitmq:
        image: rabbitmq:4.3.6-management-alpine
        options: >-
          --health-cmd "rabbitmq-diagnostics -q ping"
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5
    
    steps:
      - uses: actions/checkout@v3
      
      - name: Setup Rust
        uses: actions-rs/toolchain@v1
      
      - name: Build services
        run: cargo build --release
      
      - name: Run migrations
        run: sqlx migrate run
      
      - name: Run E2E tests
        run: ./tests/run_e2e_tests.sh
        timeout-minutes: 30
```

---

## Test Reporting

### Success Metrics

**Per Test:**
- Pass/Fail status
- Execution time
- Resource usage (CPU, memory)
- Service logs (if failed)

**Overall Suite:**
- Total tests: 40
- Passed: X
- Failed: Y
- Skipped: Z
- Total time: N minutes
- Success rate: X%

### Test Report Format

```
=== Attune E2E Test Report ===
Date: 2026-01-27 10:30:00
Duration: 15 minutes 32 seconds

Tier 1: Core Flows (8 tests)
  ✅ T1.1 Interval Timer Automation (28.3s)
  ✅ T1.2 Date Timer Execution (9.1s)
  ✅ T1.3 Cron Timer Execution (68.5s)
  ✅ T1.4 Webhook Trigger (14.2s)
  ✅ T1.5 Workflow with Array Iteration (19.8s)
  ✅ T1.6 Key-Value Store Read (8.9s)
  ✅ T1.7 Multi-Tenant Isolation (18.3s)
  ✅ T1.8 Action Failure Handling (13.7s)
  
Tier 2: Orchestration (13 tests)
  ✅ T2.1 Nested Workflow (29.4s)
  ✅ T2.2 Failure Handling (24.1s)
  ❌ T2.3 Datastore Write (FAILED - timeout)
  ...

Tier 3: Advanced (19 tests)
  ⏭️  T3.17 Container Runner (SKIPPED - Docker not available)
  ...

Summary:
  Total: 40 tests
  Passed: 38 (95%)
  Failed: 1 (2.5%)
  Skipped: 1 (2.5%)
  Success Rate: 95%

Failed Tests:
  T2.3: Datastore Write
    Error: Execution timeout after 30 seconds
    Logs: /tmp/attune-e2e/logs/t2.3-failure.log
```

---

## Maintenance and Updates

### Adding New Tests

1. **Document test in this plan** with:
   - Priority tier
   - Duration estimate
   - Description and steps
   - Success criteria

2. **Create test fixture** if needed:
   - Add to `tests/fixtures/`
   - Document fixture setup

3. **Implement test** in appropriate tier:
   - `tests/e2e/tier1/test_*.py`
   - Use test helpers from `tests/helpers/`

4. **Update test count** in summary

### Updating Existing Tests

When platform features change:
1. Review affected tests
2. Update test steps and criteria
3. Update expected outcomes
4. Re-run test to validate

### Deprecating Tests

When features are removed:
1. Mark test as deprecated
2. Move to `tests/e2e/deprecated/`
3. Update test count in summary

---

## Troubleshooting

### Common Test Failures

**Symptom**: Test timeout  
**Causes**:
- Service not running
- Message queue not connected
- Database migration issue
**Solution**: Check service logs, verify connectivity

**Symptom**: Execution stuck in 'scheduled' status  
**Causes**:
- Worker not consuming queue
- Worker crashed
- Queue message not delivered
**Solution**: Check worker logs, verify RabbitMQ queues

**Symptom**: Multi-tenant test fails  
**Causes**:
- Missing tenant_id filter in query
- JWT token for wrong tenant
**Solution**: Verify repository filters, check JWT claims

### Debug Mode

Run tests with verbose logging:
```bash
RUST_LOG=debug ./tests/run_e2e_tests.sh
```

Capture service logs:
```bash
./tests/scripts/start-services.sh --log-dir=/tmp/attune-logs
```

### Test Data Cleanup

Reset test database between runs:
```bash
./tests/scripts/reset-test-db.sh
```

---

## Appendix

### Test Fixture Catalog

**Packs:**
- `test_pack` - Simple echo action for basic tests
- `timer_pack` - Timer trigger examples
- `webhook_pack` - Webhook trigger examples
- `workflow_pack` - Multi-task workflows
- `failing_pack` - Actions that fail for error testing

**Users:**
- `test_admin` - Admin role, tenant_id=1
- `test_viewer` - Viewer role, tenant_id=1
- `test_user_2` - Admin role, tenant_id=2

**Secrets:**
- `test_api_key` - For secret injection tests
- `test_password` - Encrypted datastore value

### API Endpoints Reference

All tests use these core endpoints:

**Authentication:**
- `POST /auth/register` - Create test user
- `POST /auth/login` - Get JWT token
- `POST /auth/refresh` - Refresh token

**Packs:**
- `GET /api/v1/packs` - List packs
- `POST /api/v1/packs` - Register pack
- `POST /api/v1/packs/{id}/reload` - Reload pack

**Actions:**
- `POST /api/v1/actions` - Create action
- `GET /api/v1/actions` - List actions

**Triggers:**
- `POST /api/v1/triggers` - Create trigger
- `POST /api/v1/webhooks/{id}` - Fire webhook

**Rules:**
- `POST /api/v1/rules` - Create rule
- `PATCH /api/v1/rules/{id}` - Update rule (enable/disable)

**Executions:**
- `GET /api/v1/executions` - List executions
- `GET /api/v1/executions/{id}` - Get execution details
- `POST /api/v1/executions/{id}/cancel` - Cancel execution

**Inquiries:**
- `GET /api/v1/inquiries` - List pending inquiries
- `POST /api/v1/inquiries/{id}/respond` - Respond to inquiry

**Datastore:**
- `GET /api/v1/datastore/{key}` - Read value
- `POST /api/v1/datastore` - Write value

### Performance Benchmarks

**Target Latencies:**
- API response time (p95): <100ms
- Webhook to event: <50ms
- Event to enforcement: <100ms
- Enforcement to execution: <500ms
- Total trigger-to-execution: <1000ms

**Throughput Targets:**
- Executions per second: 100+
- Concurrent workflows: 50+
- Timer precision: ±500ms

---

**Document Version**: 1.0  
**Last Review**: 2026-01-27  
**Next Review**: After Tier 1 tests implemented  
**Owner**: Attune Development Team
