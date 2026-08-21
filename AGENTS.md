# Attune Project Rules

<!-- 2026-06-30: Leaned AGENTS.md, removed duplicated deep detail, and kept only task-critical invariants. -->

## Project Overview
Attune is a pre-production, event-driven automation/orchestration platform built primarily in Rust. It is StackStorm-like: sensors create events, rules create enforcements, and the executor schedules actions and workflows.

### Status / Change Policy
- **Pre-production**: no stable release or backward-compatibility promise yet.
- **Breaking changes are allowed** when they improve architecture, APIs, or developer experience.
- **Internal contracts still matter**: keep API ↔ web UI and service ↔ service expectations coherent.

## Critical Guardrails (Read First)
1. **NEVER** run `git commit` from agent work. Staging is fine; commits are for the user.
2. **ALWAYS** use the repository layer for DB access. Do **not** bypass it with ad hoc service queries.
3. **NEVER** hardcode schema prefixes in SQL. Rely on PostgreSQL `search_path`.
4. **ALWAYS** apply `RequireAuth` to protected Axum routes.
5. **REMEMBER** all primary IDs are `i64` / `BIGINT`.
6. **NEVER** use `SELECT *` for SQLx `FromRow` models on evolving tables. Use repository `SELECT_COLUMNS` constants (for example in `execution.rs`, `pack.rs`, `runtime_version.rs`).
7. **REMEMBER** `event`, `enforcement`, and `execution` are Timescale hypertables and **cannot be FK targets**. Referencing columns are plain `BIGINT` values and may dangle.
8. **ALWAYS** keep schema definitions flat: `param_schema`, `out_schema`, and `conf_schema` use Attune's flat per-field format, not raw JSON Schema.
9. **ALWAYS** keep `execution.config` flat: the object itself is the parameters map. Never wrap parameters under `{"parameters": ...}`.
10. **ALWAYS** deliver action parameters via **stdin JSON**, not environment variables.
11. **REMEMBER** execution API access is opt-in via `permission_set_refs`. The reserved ref `standard` grants only scoped key/artifact access for the executing action/pack (and containing workflow action/pack for workflow child executions).
12. **REMEMBER** workflow actions are stored as **two files**: action metadata YAML in `actions/` plus graph YAML in `actions/workflows/`, linked by `workflow_file`.
13. **ALWAYS** compare runtime names case-insensitively via `normalize_runtime_name()` (for example, `node`, `nodejs`, and `Node.js` should resolve equivalently).
14. **NEVER** use `PgListener::listen()` in a loop in the notifier. Use **`PgListener::listen_all()`** once, or the listener can stop receiving notifications.
15. **ALWAYS** maintain a zero-warning workspace. Run `cargo check --all-targets --workspace` and fix warnings you introduce.
16. **REMEMBER** schema changes require `cargo sqlx prepare`.
17. **REMEMBER** when adding mutable `execution` or `worker` columns, update the history trigger with `IS DISTINCT FROM` checks; for large JSONB values, store digest summaries with `_jsonb_digest_summary()` instead of raw payloads.

## Core Stack
- **Rust** 2021
- **Database**: PostgreSQL **16+** with TimescaleDB 2.17+
- **Queue**: RabbitMQ 3.12+
- **API**: Axum 0.8 + SQLx
- **Web UI**: React 19 + TypeScript + Vite
- **Async**: Tokio

## Repo Orientation
```text
attune/
├── Cargo.toml
├── Makefile
├── config.{development,test,docker}.yaml
├── crates/
│   ├── common/      shared models, repos, config, MQ, workflow helpers
│   ├── api/         REST API
│   ├── executor/    scheduling, workflow orchestration, queue dispatch
│   ├── worker/      action execution + universal agent binary
│   ├── sensor/      managed sensor runtime
│   ├── notifier/    LISTEN/NOTIFY + WebSocket fanout
│   ├── supervisor/  retention and corrective maintenance
│   └── cli/         CLI + MCP binary
├── migrations/
├── packs/
├── web/
├── docs/
├── scripts/
└── tests/
```

## Architecture Map
### Services
- **api**: JWT auth, REST endpoints, pack registration/upload, artifacts, dashboard APIs.
- **executor**: creates/schedules executions, orchestrates workflows, dispatches work queues.
- **worker**: runs actions in configured runtimes; manages pack runtime environments.
- **agent**: musl-linked worker variant for injection into arbitrary containers; auto-detects runtimes.
- **sensor**: runs managed sensor processes and emits events.
- **notifier**: PostgreSQL LISTEN/NOTIFY to authenticated WebSocket subscriptions.
- **supervisor**: retention, stale-state remediation, alerting, and maintenance loops.
- **cli / attune-mcp**: CLI plus MCP transport over stdio/HTTP.

### High-level Event Flow
- **Sensor → Event → Rule → Enforcement → Execution**
- Workflow actions are orchestrated by the **executor**; workflow tasks become normal child executions.
- Work queues create normal executions through executor dispatch.
- Cache consumers—including actions, sensors, operators, and external SDKs—use
  the authenticated HTTP API documented by OpenAPI. The executor's native
  workflow `iterate_cache` source is the intentional internal exception: it
  authorizes through execution permission snapshots and streams pinned
  generations through the repository layer. Attune does not maintain a bespoke
  in-tree Rust cache SDK.

### Deep-Dive Docs
- Service architecture: `docs/architecture/*.md`
- Notifier: `docs/architecture/notifier-service.md`
- Sensor lifecycle: `docs/sensors/sensor-lifecycle-management.md`
- Supervisor: `docs/deployment/supervisor.md`
- Workflow engine: `docs/workflows/workflow-orchestration.md`, `docs/workflows/workflow-execution-engine.md`

## Docker / Deployment Reality
- Attune runs primarily via **Docker Compose** (`docker-compose.yaml`).
- Main named volumes:
  - `packs_data` for packs
  - `runtime_envs` for generated runtime environments
  - `artifacts_data` for file-backed artifacts
  - `agent_bin` for injected musl-linked binaries
- Packs are **mounted/shared**, not copied into service images.
- Pack updates are typically applied with a **service restart**, not an image rebuild.
- Generated runtime environments live **outside** packs under `runtime_envs_dir`.
- Default local Docker user: `test@attune.local` / `TestPass123!`
- Production secrets come from env vars, especially `JWT_SECRET` and `ENCRYPTION_KEY`.

### Musl / Cross-Compilation Guidance
Use the shared pattern for both agent binaries and pack binaries:
- Dockerfiles: `docker/Dockerfile.agent`, `docker/Dockerfile.pack-binaries`
- Build style: **musl + cargo-zigbuild** for statically linked multi-arch binaries
- Common commands:
  - `make docker-build-agent`
  - `make docker-build-agent-arm64`
  - `make docker-build-pack-binaries`
  - `make docker-build-pack-binaries-arm64`
- Compose init services copy built binaries into shared volumes before app services start.

For more detail, use:
- `docs/docker-layer-optimization.md`
- `docs/QUICKREF-buildkit-cache-strategy.md`
- `docs/QUICKREF-packs-volumes.md`
- `docs/QUICKREF-unified-runtime-detection.md`

## Canonical Data / DB Rules
### Database Access
- Repository layer is the source of truth for queries and persistence.
- All SQL uses unqualified table names and depends on `search_path`.
- Use transactions for multi-table work.
- Use PostgreSQL enum mappings in Rust for custom enums.

### Hypertables and History (Canonical)
- `event`, `enforcement`, and `execution` are Timescale hypertables.
- Because hypertables cannot be FK targets, references such as `execution.parent`, `execution.enforcement`, `workflow_execution.execution`, and `inquiry.execution` are plain `BIGINT` columns.
- `event` is immutable after insert.
- `enforcement` has a narrow lifecycle and no separate history table.
- `execution` is mutable and has an `execution_history` hypertable; `worker` also has history tracking.
- History is trigger-driven. If you add mutable `execution`/`worker` columns, keep trigger diffs in sync with `IS DISTINCT FROM` checks.
- For large JSONB fields in history, store `_jsonb_digest_summary()` output instead of raw content.
- `migrations/` is the source of truth for current schema shape.

### Artifact / File Storage Essentials
- File artifacts live on the shared artifact storage volume; metadata stays in PostgreSQL.
- Artifact transport supports shared-volume and API-backed modes.
- Runtime logs are classified as private `runtime_log` artifacts.

## Core Domain Notes
- **Pack**: bundle of actions, triggers, rules, sensors, runtimes, workflows, queues, etc.
- **Action**: executable unit; may declare default `permission_set_refs`, timeouts, runtime requirements, and worker placement constraints.
- **Trigger / Event / Rule / Enforcement**: event-ingress pipeline.
- **Execution**: one action run; may be part of a workflow or queue dispatch.
- **WorkQueue**: durable business queue that dispatches normal executions.
- **Key**: structured or string secret/config storage; encryption uses `attune_common::crypto`.
- **Artifact**: execution/sensor outputs, files, logs, and progress state.

## Workflows: Model-Level Rules Only
### Storage and Authoring
- Workflow actions use **two files**:
  1. `actions/<name>.yaml` for action metadata
  2. `actions/workflows/<name>.workflow.yaml` for graph definition
- The action YAML must reference the workflow file via `workflow_file`.
- Do **not** embed the workflow graph directly in the action YAML.

### Graph Model
- Canonical task transition model is `next: []`.
- Conditions belong on **transitions**, not tasks.
- The UI does not rely on task `type`; tasks are action invocations.
- `with_items`, retry, timeout, publish, and child execution lineage are executor concerns.
- `iterate_cache` pins one cache generation and lazily materializes bounded
  item/batch child executions; `require_fresh` defaults to `false`.

### Template / Context Rules
- Use canonical namespaces in workflow expressions: `parameters`, `workflow`, `task`, `config`, `keystore`, `item`, `index`, `system`.
- Pure `{{ ... }}` expressions are type-preserving.
- Prefer `event.payload.*` in rule/action-param templates; the old `trigger.payload.*` form is legacy.

Workflow details live in:
- `docs/workflows/*.md`
- `docs/api/api-workflows.md`
- `docs/examples/*.yaml`

## Runtime / Execution Rules
- Worker runtimes come from DB-backed runtime definitions; native runtimes are represented by empty interpreter config.
- The worker/agent sets up isolated environments under `runtime_envs_dir/{pack_ref}/{runtime_name...}`.
- When runtime versions exist, execution selects the highest available matching version.
- `ATTUNE_API_TOKEN` is present only when the execution snapshot has non-empty `permission_set_refs`.
- Important execution env vars include `ATTUNE_ACTION`, `ATTUNE_PACK_REF`, `ATTUNE_EXEC_ID`, `ATTUNE_API_URL`, `ATTUNE_ARTIFACTS_DIR`, and `ATTUNE_RUNTIME_ENVS_DIR`.

## Notifier / Sensor / Supervisor Contracts
### Notifier
- Authenticated WebSocket service on port 8081.
- Subscribe with filters like `all`, `entity_type:<type>`, `entity:<type>:<id>`, `user:<id>`, `notification_type:<type>`.
- JWT is required at connect time; do **not** use query-string tokens.
- **Critical invariant**: use `PgListener::listen_all()` once.
- Outbound notifications must stay wrapped in the tagged client-message enum so serialized messages retain their `type` field.

### Sensor
- Managed sensors run as processes selected by worker capabilities / placement constraints.
- Sensor stdout/stderr are exposed via sensor log APIs and retained as artifact-backed runtime logs.
- Repeated managed sensor failures escalate via `core.alert`.

### Supervisor
- Owns retention, stale-state cleanup, corrective remediation, and related audit/alert emission.
- Uses advisory locking so only one maintenance leader acts at a time.
- Runtime retention config is persisted in DB and reloaded without restart.

Use docs for specifics instead of expanding this file:
- `docs/architecture/notifier-service.md`
- `docs/sensors/*.md`
- `docs/deployment/supervisor.md`

## Authentication / Security Essentials
- Access auth is JWT-based; integration tokens, OIDC, and LDAP are supported.
- Protected Axum routes use `RequireAuth`.
- Secrets encryption/decryption goes through shared `attune_common::crypto` helpers.
- Never put raw secrets, token values, decrypted key material, or artifact content into audit-event details.

## Web UI Essentials
- Generated API client lives under `web/src/api/`; regenerate with `npm run generate:api` when needed.
- Dashboard runtime + authoring exist; use the dashboard types/contracts codepaths already in the repo.
- Workflow builder edits the two-file workflow/action representation.
- Execution detail pages include workflow timeline visualization driven by workflow definition + child executions.

For UI implementation details, read:
- `docs/web-ui/*.md`
- `docs/architecture/web-ui-architecture.md`

## Development Workflow
### Common Commands
```bash
make build
make test
make lint
make run-api
make run-executor
make run-worker
make run-agent
make run-sensor
make run-notifier
make run-supervisor
make db-migrate
```

### Testing / Validation
- Tests use **schema-per-test** isolation.
- Use `make db-test-setup` before integration tests.
- Use `cargo test -- --nocapture --test-threads=1` for detailed failures.
- Full validation can be slow: allow at least 40 minutes for `cargo test`, up
  to 2 hours for `make test-integration`, and up to 2 hours for `make e2e-test`.
  Set the command timeout before starting so a passing run is not terminated
  and repeated only because the agent timeout was too short.
- After schema changes, run `cargo sqlx prepare`.
- Before finishing code work, run targeted validation plus `cargo check --all-targets --workspace`.

### Zero-Warnings Policy
- New warnings are regressions.
- Prefer fixing dead code / unused imports / unused variables over suppressing warnings.
- If `#[allow(dead_code)]` is necessary, keep it narrow and explain why.

### CLI: Representative Commands
For full CLI coverage, see `docs/cli/cli.md` and `docs/cli/cli-profiles.md`.

```bash
cargo install --path crates/cli
attune auth login
attune pack list
attune pack upload ./packs/my_pack
attune action execute core.echo --param message=hi
attune workflow upload actions/deploy.yaml --force
attune execution list
attune execution trace-report core.timer.1234
attune key list
attune key show my_token -d
attune artifact list --execution 42
attune-mcp
```

CLI rules worth remembering:
- Prefer `pack upload` for local-to-Docker installs; `pack register` only works for API-visible paths.
- `workflow upload` expects an action YAML with `workflow_file` pointing at the companion graph file.
- `attune-mcp` reuses CLI auth/profile state.

## File Placement Conventions
- **New API endpoint**: `crates/api/src/routes/<domain>.rs` + DTO in `crates/api/src/dto/<domain>.rs`
- **New model / table**: model in `crates/common/src/models.rs`, repo in `crates/common/src/repositories/`, migration in `migrations/`
- **New config**: add to `crates/common/src/config.rs`
- **New docs**: put usage/system docs in `docs/`; put work summaries in `work-summary/`

## Important Files / Directories
- `crates/common/src/models.rs`
- `crates/common/src/repositories/`
- `crates/common/src/config.rs`
- `crates/common/src/workflow/`
- `crates/api/src/routes/mod.rs`
- `crates/executor/src/workflow/`
- `crates/worker/src/agent_main.rs`
- `crates/worker/src/runtime_detect.rs`
- `crates/sensor/src/sensor_log.rs`
- `crates/notifier/src/websocket_server.rs`
- `crates/supervisor/src/main.rs`
- `docker/`
- `docs/`

## Documentation Pointers
Use retrieval, not memory. Start here instead of expanding AGENTS again:
- Architecture: `docs/architecture/*.md`
- API: `docs/api/*.md`
- Workflows: `docs/workflows/*.md`
- Sensors: `docs/sensors/*.md`
- Deployment / ops: `docs/deployment/*.md`
- CLI: `docs/cli/*.md`
- Quick starts / guides: `docs/guides/*.md`
- Agent/doc index: `docs/development/agents-md-index.md`
- To regenerate the doc index: `make generate-agents-index`

## Reporting / Work Summary Rules
- Report progress briefly while working.
- Summarize changes **once** at the end.
- Do not create ad hoc docs in the repo root.
- Put enduring system docs in `docs/`.
- Put work summaries in `work-summary/` only when that output is actually desired.

## Keeping This File Updated
Update `AGENTS.md` when any of these change materially:
- repo structure
- major architecture / service boundaries
- build/deployment/tooling approach
- core data-model invariants
- workflow storage/authoring rules
- major developer guardrails

When updating:
- keep it concise and scannable
- prefer pointers to docs over deep implementation detail
- preserve the guardrails in the **Critical Guardrails** section
- avoid duplicating the same invariant in multiple sections

## Agent skills

### Issue tracker

Issues are tracked in a local SQLite store at `.scratch/issues.db`, managed through the `issues` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Five canonical roles with default label strings: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context layout: one `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
