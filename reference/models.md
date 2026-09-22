# Attune Data Models Reference

This document describes the SQLAlchemy ORM models that define the Attune automation platform's database schema.

## Overview

Attune is an event-driven automation and orchestration platform with built-in multi-tenancy, RBAC, and workflow capabilities. The data models are organized into several functional areas:

- **Packaging & Distribution**: Organization of automation components
- **Runtime Environment**: Execution environments and workers
- **Event-Driven Architecture**: Triggers, sensors, and events
- **Actions & Automation**: Executable operations and rules
- **Execution & Orchestration**: Action execution and workflow management
- **User Interactions**: Asynchronous user input and approvals in workflows
- **Policy & Permissions**: Access control and execution policies
- **Identity & Access**: User and service account management
- **Configuration & Secrets**: Secure key-value storage
- **Notifications**: Real-time event streaming
- **Artifacts & Storage**: Execution output management

---

## Core Infrastructure

### `Base`
**Purpose**: SQLAlchemy declarative base class for all models.
- Configured with "attune" schema metadata
- Parent class for all database models

---

## Packaging & Distribution

### `Pack`
**Purpose**: A package/bundle of automation components (sensors, actions, rules, policies, etc.).

**Key Fields**:
- `ref`: Unique reference (e.g., "slack", "github")
- `label`: Human-readable name
- `version`: Semantic version string (validated)
- `conf_schema`: JSON schema for configuration
- `config`: Configuration values
- `runtime_deps`: List of required runtime references
- `is_standard`: Whether this is a core/built-in pack

**Relationships**:
- Contains: runtimes, triggers, sensors, actions, rules, permission_sets, policies

**Purpose**: Primary organizational unit for distributing and managing related automation components.

---

## Runtime Environment

### `Runtime`
**Purpose**: Defines a unified execution environment for actions and sensors.

**Key Fields**:
- `ref`: Unique reference (format: `pack.name`, e.g., `core.python`, `core.shell`)
- `name`: Runtime name (e.g., "Python", "Shell", "Node.js")
- `distributions`: JSON describing available distributions and verification metadata
- `installation`: JSON describing installation requirements
- `execution_config`: JSON describing how to execute code (interpreter, environment setup, dependencies). Runtimes without an `execution_config` (e.g., `core.builtin`) cannot execute actions — the worker skips them.
- `pack`: Parent pack ID

**Relationships**:
- Belongs to: pack
- Used by: workers, sensors, actions

**Purpose**: Defines how to install and execute code (Python, Node.js, containers, etc.). Runtimes are shared between actions and sensors — there is no type distinction.

### `WorkerType` (Enum)
**Values**: `local`, `remote`, `container`

**Purpose**: Categorizes worker deployment types.

### `WorkerStatus` (Enum)
**Values**: `active`, `inactive`, `busy`, `error`

**Purpose**: Tracks current operational state of workers.

### `Worker`
**Purpose**: Represents a worker process/container that executes actions or monitors sensors.

**Key Fields**:
- `name`: Worker identifier
- `worker_type`: Deployment type
- `runtime`: Associated runtime ID
- `host`, `port`: Connection details for remote workers
- `status`: Current operational state
- `capabilities`: JSON describing worker capabilities
- `last_heartbeat`: Last health check timestamp

**Relationships**:
- Uses: runtime

**Purpose**: Manages the actual compute resources that run automation code.

---

## Event-Driven Architecture

### `Trigger`
**Purpose**: Defines an event type that can activate rules.

**Key Fields**:
- `ref`: Unique reference (format: `pack.name`)
- `label`: Display name
- `enabled`: Whether trigger is active
- `param_schema`: JSON schema for trigger parameters
- `out_schema`: JSON schema for trigger output/payload
- `pack`: Parent pack ID

**Relationships**:
- Belongs to: pack
- Referenced by: sensors, rules, events

**Examples**: "webhook_received", "file_modified", "schedule_fired"

### `Sensor`
**Purpose**: Monitors for trigger conditions and generates events.

**Key Fields**:
- `ref`: Unique reference (format: `pack.name`)
- `entrypoint`: Code entry point (e.g., "sensors/webhook.py")
- `runtime`: Execution environment
- `trigger`: Associated trigger
- `enabled`: Whether sensor is active
- `param_schema`: JSON schema for configuration parameters

**Relationships**:
- Belongs to: pack
- Uses: runtime
- Monitors for: trigger

**Purpose**: Active monitoring component that detects when triggers fire and creates events.

### `Event`
**Purpose**: An instance of a trigger firing.

**Key Fields**:
- `trigger`: Trigger ID (with SET NULL on delete)
- `trigger_ref`: Trigger reference (preserved)
- `config`: Snapshot of trigger/sensor configuration at event time
- `payload`: Event data
- `source`: Sensor that generated the event
- `source_ref`: Source sensor reference

**Relationships**:
- Instance of: trigger
- Generated by: sensor
- Triggers: enforcements

**Purpose**: Records that a trigger occurred, including all context needed for rule evaluation.

---

## Actions & Automation

### `Action`
**Purpose**: An executable task/operation.

**Key Fields**:
- `ref`: Unique reference (format: `pack.name`)
- `label`: Display name
- `entrypoint`: Code entry point (e.g., "actions/send_email.py")
- `runtime`: Execution environment
- `param_schema`: JSON schema for input parameters
- `out_schema`: JSON schema for output/results
- `pack`: Parent pack ID

**Relationships**:
- Belongs to: pack
- Uses: runtime
- Executed by: executions
- Used in: rules

**Examples**: "send_email", "create_ticket", "deploy_service"

### `Rule`
**Purpose**: Connects triggers to actions with conditional logic.

**Key Fields**:
- `ref`: Unique reference (format: `pack.name`)
- `trigger`: Trigger that activates this rule
- `action`: Action to execute when conditions are met
- `conditions`: JSON array of condition expressions
- `enabled`: Whether rule is active
- `pack`: Parent pack ID

**Relationships**:
- Belongs to: pack
- When: trigger
- Then: action
- Creates: enforcements

**Purpose**: Automation logic that says "when trigger X fires, if conditions Y are met, execute action Z".

### `EnforcementStatus` (Enum)
**Values**: `created`, `processed`, `disabled`

**Purpose**: Tracks processing state of rule enforcements.

### `EnforcementCondition` (Enum)
**Values**: `any`, `all`

**Purpose**: Defines logical operator for multiple conditions (OR vs AND).

### `Enforcement`
**Purpose**: An instance of a rule being triggered by an event.

**Key Fields**:
- `rule`: Rule being enforced
- `rule_ref`: Rule reference (preserved)
- `trigger_ref`: Trigger reference (preserved)
- `event`: Event that triggered this enforcement
- `config`: Snapshot of rule/trigger configuration
- `status`: Processing state
- `payload`: Event payload for rule evaluation
- `condition`: Logical operator (any/all)
- `conditions`: Condition expressions to evaluate

**Relationships**:
- Instance of: rule
- Triggered by: event
- Creates: executions

**Purpose**: Records that a rule was triggered and tracks whether it should execute its action.

---

## Execution & Orchestration

### `ExecutionStatus` (Enum)
**Values**: `requested`, `scheduling`, `scheduled`, `running`, `completed`, `failed`, `canceling`, `cancelled`, `timeout`, `abandoned`

**Purpose**: Tracks detailed lifecycle state of action executions.

### `Execution`
**Purpose**: Represents a single action execution (can be part of a workflow).

**Key Fields**:
- `action`: Action being executed
- `action_ref`: Action reference (preserved)
- `config`: Snapshot of action configuration
- `parent`: Parent execution ID (for workflows)
- `enforcement`: Enforcement that triggered this execution
- `executor`: Identity that initiated execution
- `status`: Current execution state
- `result`: JSON output from the action

**Relationships**:
- Executes: action
- Triggered by: enforcement (if rule-driven) or executor (if manual)
- Parent/child: execution (for workflows)
- Executed by: identity

**Purpose**: Tracks individual action runs, supports nested workflows, and stores results.

### `InquiryStatus` (Enum)
**Values**: `pending`, `responded`, `timeout`, `cancelled`

**Purpose**: Tracks lifecycle state of user inquiries in workflows.

### `Inquiry`
**Purpose**: Represents an asynchronous user interaction within a workflow execution.

**Key Fields**:
- `created_by_execution`: Execution that created this inquiry
- `workflow_execution`: Workflow instance that contains the creator execution
- `prompt`: Question or prompt text for the user
- `response_schema`: Optional Attune flat schema defining expected response fields
- `response_options`: Immutable provider-neutral response choices
- `assigned_to`: Identity who should respond to this inquiry
- `status`: Current state of the inquiry
- `response`: JSON response data from the user
- `timeout_at`: When this inquiry expires
- `timeout_seconds`: Immutable relative timeout used for idempotent creation
- `responded_by`: Attune identity that submitted the accepted response
- `external_actor`: Non-secret attribution for an accepted integration response
- `responded_at`: When the response was received

**Relationships**:
- Created by: execution
- Scoped to: workflow execution
- Assigned to: identity

**Purpose**: Enables workflows to pause and wait for human input/approval. When an action needs user interaction (e.g., approval, additional information, decision-making), it creates an Inquiry. The workflow execution pauses until the inquiry is responded to, times out, or is cancelled. This allows for human-in-the-loop automation patterns.

**Use Cases**:
- Approval workflows (deploy approval, expense approval)
- Information gathering (incident details, configuration choices)
- Decision points (which path to take in a workflow)
- Manual verification steps

---

## Policy & Permissions

### `PermissionGrant` (TypedDict)
**Purpose**: Defines structure for permission grants.

**Fields**:
- `type`: `system`, `pack`, or `user`
- `scope`: Pack name for pack-scoped permissions
- `components`: List of component references

**Purpose**: Structured permission definition for RBAC.

### `PermissionSet`
**Purpose**: A named collection of permissions (like a role).

**Key Fields**:
- `ref`: Unique reference (format: `pack.name`)
- `label`: Display name
- `grants`: Array of permission grants
- `pack`: Parent pack (optional, for pack-defined roles)

**Relationships**:
- Belongs to: pack (optional)
- Assigned to: identities via permission_assignment

**Purpose**: Groups permissions together for assignment to users/services.

### `PermissionAssignment`
**Purpose**: Links identities to permission sets (many-to-many).

**Key Fields**:
- `identity`: Identity ID
- `permset`: Permission set ID

**Purpose**: Grants permissions to users/services.

### `PolicyMethod` (Enum)
**Values**: `cancel`, `enqueue`

**Purpose**: Defines how to handle policy violations.

### `Policy`
**Purpose**: Defines execution policies for actions (rate limiting, concurrency control, etc.).

**Key Fields**:
- `ref`: Unique reference (format: `pack.name`)
- `action`: Action this policy applies to
- `parameters`: List of parameter names used for policy grouping
- `method`: How to handle policy violations
- `threshold`: Numeric limit (e.g., max concurrent executions)
- `pack`: Parent pack

**Relationships**:
- Belongs to: pack
- Applies to: action

**Examples**: "Max 5 concurrent executions", "Rate limit to 100/hour"

---

## Identity & Access

### `Identity`
**Purpose**: Represents a user or service account.

**Key Fields**:
- `login`: Unique login identifier
- `display_name`: Human-readable name
- `attributes`: JSON for custom attributes (email, groups, etc.)

**Relationships**:
- Has: permission_assignments
- Executes: executions
- Owns: keys
- Receives: inquiries (assigned tasks requiring response)

**Purpose**: User/service account management for authentication and authorization.

---

## Configuration & Secrets

### `OwnerType` (Enum)
**Values**: `system`, `identity`, `pack`, `action`, `sensor`

**Purpose**: Defines ownership scope for keys.

### `Key`
**Purpose**: Stores configuration values and secrets with ownership scoping.

**Key Fields**:
- `ref`: Unique reference (format: `[owner.]name`)
- `owner_type`: Type of owner
- `owner`: Owner identifier (auto-populated)
- `owner_identity`, `owner_pack`, `owner_action`, `owner_sensor`: Foreign keys to owners
- `name`: Key name
- `encrypted`: Whether value is encrypted
- `encryption_key_hash`: Hash of encryption key used
- `value`: The actual value (encrypted if `encrypted=true`)

**Constraints**:
- Unique on: (owner_type, owner, name)
- Exactly one owner FK must be set (validated by trigger)

**Purpose**: Secure, scoped key-value storage for configuration and secrets.

---

## Notifications

### `NotificationState` (Enum)
**Values**: `created`, `queued`, `processing`, `error`

**Purpose**: Tracks notification delivery state.

### `Notification`
**Purpose**: System notifications about entity changes.

**Key Fields**:
- `channel`: Notification channel (typically table name)
- `entity_type`: Type of entity (table name)
- `entity`: Entity identifier (typically ID or ref)
- `activity`: Activity type (e.g., "created", "updated", "completed")
- `state`: Processing state
- `content`: JSON payload with notification details

**Behavior**:
- Automatically sends PostgreSQL `pg_notify` on insert
- Used by triggers throughout the system to notify about changes

**Purpose**: Real-time event streaming for UI updates and system integration.

---

## Artifacts & Storage

### `ArtifactType` (Enum)
**Values**: 
- `file_binary`: Binary file for download
- `file_datatable`: Tabular data for display
- `file_image`: Image for display
- `file_text`: Text file for display
- `url`: URL reference
- `progress`: Progress tracking data
- `other`: Other types

**Purpose**: Categorizes artifact display/handling behavior.

### `RetentionPolicyType` (Enum)
**Values**: `versions`, `days`, `hours`, `minutes`

**Purpose**: Defines retention policy type.

### `Artifact`
**Purpose**: Manages execution output artifacts with retention policies.

**Key Fields**:
- `ref`: Artifact reference/path
- `scope`: Owner type (system, identity, pack, action, sensor)
- `owner`: Owner identifier
- `type`: Artifact type
- `retention_policy`: How to retain artifacts
- `retention_limit`: Numeric limit for retention

**Purpose**: Tracks files, logs, and other outputs from executions with automatic cleanup.

---

## Database Functions & Triggers

The models file defines several PostgreSQL functions and triggers that provide:

1. **Auto-population**: Automatically fill pack references, foreign keys, and refs
2. **Validation**: Ensure referential integrity and format constraints
3. **Configuration Snapshots**: Capture configuration at event/enforcement/execution creation time
4. **Notifications**: Automatically create notification records on entity changes
5. **Ownership Validation**: Enforce key ownership constraints

These ensure data consistency and provide audit trails throughout the system.

---

## Common Patterns

### Reference Format
All components use a `ref` field with format `pack.name` (e.g., `slack.webhook_trigger`, `core.python`, `core.shell`).

### Ref vs ID
- Foreign key relationships use IDs
- `*_ref` fields store the string reference and are preserved even if the referenced entity is deleted
- Triggers auto-populate IDs from refs and vice versa

### Configuration Snapshots
Events, enforcements, and executions capture configuration snapshots in their `config` field to maintain execution context even if definitions change later.

### Soft Cascades
Many foreign keys use `SET NULL` on delete and preserve `*_ref` fields, allowing historical tracking even after components are removed.

---

## System Architecture Flow

1. **Pack Installation**: Packs are installed, containing triggers, sensors, actions, rules, and policies
2. **Sensor Monitoring**: Sensors watch for trigger conditions
3. **Event Generation**: When a trigger fires, an event is created
4. **Rule Evaluation**: Events trigger enforcements of matching rules
5. **Execution**: Enforcements create executions of actions
6. **Worker Processing**: Workers pick up and execute actions
7. **Human Interaction** (optional): Actions can create inquiries to pause workflows and wait for user input
8. **Result Storage**: Execution results and artifacts are stored
9. **Notifications**: Changes flow through the notification system for real-time updates

This architecture supports both rule-driven automation and manual action execution, with full audit trails, workflow orchestration capabilities, and human-in-the-loop patterns via inquiries.
