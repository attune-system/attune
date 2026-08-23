# Attune Database Migrations

This directory contains SQL migrations for the Attune automation platform database schema.

## Overview

Migrations are numbered and executed in order. Each migration file is named with a timestamp prefix to ensure proper ordering:

```
YYYYMMDDHHMMSS_description.sql
```

## Migration Files

An operational runner-claim migration precedes the 5 foundational schema
migrations:

| File | Description |
|------|-------------|
| `20240101000000_migration_runner_claim.sql` | Detects and persistently claims either SQLx or Docker migration history before schema DDL runs |
| `20250101000001_initial_setup.sql` | Creates schema, service role, all enum types, and shared functions |
| `20250101000002_core_tables.sql` | Creates pack, runtime, worker, identity, permission_set, permission_assignment, policy, and key tables |
| `20250101000003_event_system.sql` | Creates trigger, sensor, event, and enforcement tables |
| `20250101000004_execution_system.sql` | Creates action, rule, execution, inquiry, workflow orchestration tables (workflow_definition, workflow_execution, workflow_task_execution), and workflow views |
| `20250101000005_supporting_tables.sql` | Creates notification, artifact, and queue_stats tables with performance indexes |

### Migration Dependencies

The migrations must be run in order due to foreign key dependencies:

1. **Initial Setup** - Foundation (schema, enums, functions)
2. **Core Tables** - Base entities (pack, runtime, worker, identity, permissions, policy, key)
3. **Event System** - Event monitoring (trigger, sensor, event, enforcement)
4. **Execution System** - Action execution (action, rule, execution, inquiry)
5. **Supporting Tables** - Auxiliary features (notification, artifact)

## Running Migrations

### Using SQLx CLI

```bash
# Install sqlx-cli if not already installed
cargo install sqlx-cli --no-default-features --features postgres

# Run all pending migrations
sqlx migrate run

# Check migration status
sqlx migrate info

# Revert last migration (if needed)
sqlx migrate revert
```

### Migration Runners

Use exactly one supported runner for a database: `attune-api --migrate`/SQLx or
the Docker migration container. The runner claim rejects attempts to mix their
history formats. Do not execute migration files directly with `psql`; the first
migration rejects execution without a supported history table.

The Docker runner recognizes a legacy `public._migrations` table and moves it
atomically to `attune` before applying files. It rejects ambiguous histories:
both `attune` and `public` copies of either runner's table, or any combination
of SQLx and Docker history. Detection and the persistent runner claim share one
transaction, so a rejected setup does not leave a new claim behind.

Docker history stores the SHA-384 checksum of every newly applied migration and
refuses to skip a filename whose current bytes do not match. A filename-only
legacy history receives one compatibility pass: files still present in the
distribution are baselined without rerunning them, and the adoption state is
closed only after the complete pass succeeds. Legacy entries for files no
longer shipped remain nullable historical records and cannot be silently
adopted if a file with that name later reappears.

### Released SQLx Checksum Bridges

When upgrading a database whose `_sqlx_migrations` history contains a released
historical checksum, run `attune-api --migrate` once. This includes v0.2.1
histories and the short-lived v0.4.0 build that rewrote migration 26. The
embedded runner recognizes those exact checksums before SQLx validates history.
The standalone `sqlx migrate run` command cannot perform that pre-validation
bridge and will report checksum mismatches. After the one-time API migration,
normal SQLx tooling can read the updated history.

The embedded runner keeps an existing `_sqlx_migrations` table in either the
`attune` or `public` schema. Fresh embedded migrations create history in
`attune`; databases with history in both schemas are rejected as ambiguous.

## Database Setup

### Prerequisites

1. PostgreSQL 14 or later installed
2. Create the database:

```bash
createdb attune
```

3. Set environment variable:

```bash
export DATABASE_URL="postgresql://postgres:postgres@localhost:5432/attune"
```

### Initial Setup

```bash
# Navigate to workspace root
cd /path/to/attune

# Run migrations
sqlx migrate run

# Verify tables were created
psql -U postgres -d attune -c "\dt attune.*"
```

Embedded `attune-api --migrate` migrations support only the `attune` schema
because historical migration files set that search path explicitly. Custom
schemas are supported by the schema-rewriting integration-test fixture, not by
production embedded migration. Do not mix Docker `_migrations` history with
SQLx `_sqlx_migrations` history.

Migration 26 removes query strings from HTTP(S) pack provenance because they
may contain credentials. Affected `pack` rows retain all other provenance and
receive the reserved `meta._attune_source_query_redacted = true` marker without
changing `source_ref` or other provenance fields. Legacy non-object metadata is
preserved under `meta._attune_legacy_meta` while the marker is added. Registry
index groups affected by query stripping are deduplicated by canonical URL
using lowest `position`, then lowest `id`; that survivor keeps its id, order,
name, and headers. Clean-only canonical-equivalent rows are not deduplicated.
Headers from discarded lower-priority tainted rows are not combined because
conflicting authorization values cannot be merged safely. Every group affected
by query stripping is disabled until an administrator reviews it. The query
text itself is intentionally and unavoidably discarded rather than persisted
elsewhere as a potential secret.

## Schema Overview

The Attune schema includes 22 tables organized into logical groups:

### Core Tables (Migration 2)
- **pack**: Automation component bundles
- **runtime**: Execution environments (Python, Node.js, containers)
- **worker**: Execution workers
- **identity**: Users and service accounts
- **permission_set**: Permission groups (like roles)
- **permission_assignment**: Identity-permission links (many-to-many)
- **policy**: Execution policies (rate limiting, concurrency)
- **key**: Secure configuration and secrets storage

### Event System (Migration 3)
- **trigger**: Event type definitions
- **sensor**: Event monitors that watch for triggers
- **event**: Event instances (trigger firings)
- **enforcement**: Rule activation instances

### Execution System (Migration 4)
- **action**: Executable operations (can be workflows)
- **rule**: Trigger-to-action automation logic
- **execution**: Action execution instances (supports workflows)
- **inquiry**: Human-in-the-loop interactions (approvals, inputs)
- **workflow_definition**: YAML-based workflow definitions (composable action graphs)
- **workflow_execution**: Runtime state tracking for workflow executions
- **workflow_task_execution**: Individual task executions within workflows

### Supporting Tables (Migration 5)
- **notification**: Real-time system notifications (uses PostgreSQL LISTEN/NOTIFY)
- **artifact**: Execution outputs (files, logs, progress data)
- **queue_stats**: Real-time execution queue statistics for FIFO ordering

## Key Features

### Automatic Timestamps
All tables include `created` and `updated` timestamps that are automatically managed by the `update_updated_column()` trigger function.

### Reference Preservation
Tables use both ID foreign keys and `*_ref` text columns. The ref columns preserve string references even when the referenced entity is deleted, maintaining complete audit trails.

### Soft Deletes
Foreign keys strategically use:
- `ON DELETE CASCADE` - For dependent data that should be removed
- `ON DELETE SET NULL` - To preserve historical records while breaking the link

### Validation Constraints
- **Reference format validation** - Lowercase, specific patterns (e.g., `pack.name`)
- **Semantic version validation** - For pack versions
- **Ownership validation** - Custom trigger for key table ownership rules
- **Range checks** - Port numbers, positive thresholds, etc.

### Performance Optimization
- **B-tree indexes** - On frequently queried columns (IDs, refs, status, timestamps)
- **Partial indexes** - For filtered queries (e.g., `enabled = TRUE`)
- **GIN indexes** - On JSONB and array columns for fast containment queries
- **Composite indexes** - For common multi-column query patterns

### PostgreSQL Features
- **JSONB** - Flexible schema storage for configurations, payloads, results
- **Array types** - Multi-value fields (tags, parameters, dependencies)
- **Custom enum types** - Constrained string values with type safety
- **Triggers** - Data validation, timestamp management, notifications
- **pg_notify** - Real-time notifications via PostgreSQL's LISTEN/NOTIFY

## Service Role

The migrations create a `svc_attune` role with appropriate permissions. **Change the password in production:**

```sql
ALTER ROLE svc_attune WITH PASSWORD 'secure_password_here';
```

The default password is `attune_service_password` (only for development).

## Rollback Strategy

### Complete Reset

To completely reset the database:

```bash
# Drop and recreate
dropdb attune
createdb attune
sqlx migrate run
```

Or drop just the schema:

```sql
psql -U postgres -d attune -c "DROP SCHEMA attune CASCADE;"
```

Then re-run migrations.

### Individual Migration Revert

With SQLx CLI:

```bash
sqlx migrate revert
```

Or manually remove from tracking:

```sql
DELETE FROM _sqlx_migrations WHERE version = 20250101000001;
```

## Best Practices

1. **Never edit existing migrations** - Create new migrations to modify schema
2. **Test migrations** - Always test on a copy of production data first
3. **Backup before migrating** - Backup production database before applying migrations
4. **Review changes** - Review all migrations before applying to production
5. **Version control** - Keep migrations in version control (they are!)
6. **Document changes** - Add comments to complex migrations

## Development Workflow

1. Create new migration file with timestamp:
   ```bash
   touch migrations/$(date +%Y%m%d%H%M%S)_description.sql
   ```

2. Write migration SQL (follow existing patterns)

3. Test migration:
   ```bash
   sqlx migrate run
   ```

4. Verify changes:
   ```bash
   psql -U postgres -d attune
   \d+ attune.table_name
   ```

5. Commit to version control

## Production Deployment

1. **Backup** production database
2. **Review** all pending migrations
3. **Test** migrations on staging environment with production data copy
4. **Schedule** maintenance window if needed
5. **Apply** migrations:
   ```bash
   sqlx migrate run
   ```
6. **Verify** application functionality
7. **Monitor** for errors in logs

## Troubleshooting

### Migration already applied

If you need to re-run a migration:

```bash
# Remove from migration tracking (SQLx)
psql -U postgres -d attune -c "DELETE FROM _sqlx_migrations WHERE version = 20250101000001;"

# Then re-run
sqlx migrate run
```

### Permission denied

Ensure the PostgreSQL user has sufficient permissions:

```sql
GRANT ALL PRIVILEGES ON DATABASE attune TO postgres;
GRANT ALL PRIVILEGES ON SCHEMA attune TO postgres;
```

### Connection refused

Check PostgreSQL is running:

```bash
# Linux/macOS
pg_ctl status
sudo systemctl status postgresql

# Check if listening
psql -U postgres -c "SELECT version();"
```

### Foreign key constraint violations

Ensure migrations run in correct order. The consolidated migrations handle forward references correctly:
- Migration 2 creates tables with forward references (commented as such)
- Migration 3 and 4 add the foreign key constraints back

## Schema Diagram

```
┌─────────────┐
│    pack     │◄──┐
└─────────────┘   │
       ▲          │
       │          │
┌──────┴──────────┴──────┐
│ runtime │ trigger │ ... │  (Core entities reference pack)
└─────────┴─────────┴─────┘
       ▲          ▲
       │          │
┌──────┴──────┐  │
│   sensor    │──┘  (Sensors reference both runtime and trigger)
└─────────────┘
       │
       ▼
┌─────────────┐     ┌──────────────┐
│    event    │────►│ enforcement  │  (Events trigger enforcements)
└─────────────┘     └──────────────┘
                           │
                           ▼
                    ┌──────────────┐
                    │  execution   │  (Enforcements create executions)
                    └──────────────┘
```

## Workflow Orchestration

Migration 4 includes comprehensive workflow orchestration support:
- **workflow_definition**: Stores parsed YAML workflow definitions with tasks, variables, and transitions
- **workflow_execution**: Tracks runtime state including current/completed/failed tasks and variables
- **workflow_task_execution**: Individual task execution tracking with retry and timeout support
- **Action table extensions**: `workflow_def` links actions to workflow definitions
- **Helper views**: Three views for querying workflow state (summary, task detail, action links)

## Queue Statistics

Migration 5 includes the queue_stats table for execution ordering:
- Tracks per-action queue length, active executions, and concurrency limits
- Enables FIFO queue management with database persistence
- Supports monitoring and API visibility of execution queues

## Additional Resources

- [SQLx Documentation](https://github.com/launchbadge/sqlx)
- [PostgreSQL Documentation](https://www.postgresql.org/docs/)
- [Attune Architecture Documentation](../docs/architecture.md)
- [Attune Data Model Documentation](../docs/data-model.md)
