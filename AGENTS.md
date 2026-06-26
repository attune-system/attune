# Attune Project Rules

<!-- 2026-06-26: Added dashboard data source planning/query-safety/watermark module references. -->

## Project Overview
Attune is an **event-driven automation and orchestration platform** built in Rust, similar to StackStorm. It enables building complex workflows triggered by events with multi-tenancy, RBAC, and human-in-the-loop capabilities.

## Development Status: Pre-Production

**This project is under active development with no users, deployments, or stable releases.**

### Breaking Changes Policy
- **Breaking changes are explicitly allowed and encouraged** when they improve the architecture, API design, or developer experience
- **No backward compatibility required** - there are no existing versions to support
- **Database migrations can be modified or consolidated** - no production data exists
- **API contracts can change freely** - no external integrations depend on them, only internal interfaces with other services and the web UI must be maintained.
- **Configuration formats can be redesigned** - no existing config files need migration
- **Service interfaces can be refactored** - no live deployments to worry about

When this project reaches v1.0 or gets its first production deployment, this section should be removed and replaced with appropriate stability guarantees and versioning policies.

## Languages & Core Technologies
- **Primary Language**: Rust 2021 edition
- **Database**: PostgreSQL 16+ with TimescaleDB 2.17+ (primary data store + LISTEN/NOTIFY pub/sub + time-series history)
- **Message Queue**: RabbitMQ 3.12+ (via lapin)
- **Web UI**: TypeScript + React 19 + Vite
- **Async Runtime**: Tokio
- **Web Framework**: Axum 0.8
- **ORM**: SQLx (compile-time query checking)

## Project Structure (Cargo Workspace)

```
attune/
├── Cargo.toml                    # Workspace root
├── config.{development,test}.yaml # Environment configs
├── Makefile                      # Common dev tasks
├── crates/                       # Rust services
│   ├── common/                   # Shared library (models, db, repos, mq, config, error, template_resolver)
│   ├── api/                      # REST API service (8080)
│   ├── executor/                 # Execution orchestration service
│   ├── worker/                   # Action execution service (multi-runtime)
│   ├── sensor/                   # Event monitoring service
│   ├── notifier/                 # Real-time notification service
│   ├── supervisor/               # Platform maintenance service (retention, cleanup, monitoring)
│   └── cli/                      # Command-line interface + MCP server binary (stdio/HTTP)
├── migrations/                   # SQLx database migrations (19 tables)
├── web/                          # React web UI (Vite + TypeScript)
├── packs/                        # Pack bundles
│   └── core/                     # Core pack (timers, HTTP, etc.)
├── docs/                         # Technical documentation
├── scripts/                      # Helper scripts (DB setup, testing)
└── tests/                        # Integration tests
```

## Service Architecture (Distributed Microservices)

1. **attune-api**: REST API gateway, JWT auth, all client interactions
2. **attune-executor**: Manages execution lifecycle, scheduling, policy enforcement, workflow orchestration, and work-queue dispatch polling
3. **attune-worker**: Executes actions in multiple runtimes (Python/Node.js/containers). The worker now maintains short-lived in-process metadata caches for action/runtime/pack reads used during execution and environment setup, and runs an ephemeral metadata invalidation consumer bound to `metadata.action.changed`, `metadata.runtime.changed`, and `metadata.pack.changed`.
4. **attune-agent**: Universal worker agent — statically-linked (musl) binary injected into any container to auto-detect runtimes and execute actions. Functionally identical to `attune-worker` but packaged for universal deployment. Lives in the same crate (`crates/worker`) as a second binary target (`src/agent_main.rs`). Uses runtime auto-detection (`src/runtime_detect.rs`) instead of `ATTUNE_WORKER_RUNTIMES` manual config. Supports `--detect-only` flag for probing container environments. The injected agent bundle also includes statically-linked `attune-mcp` and `attune` CLI binaries under `/opt/attune/agent/` so agent-driven actions can talk back to Attune without relying on stock container utilities like `curl`.
5. **attune-sensor**: Monitors triggers, generates events. Sensor workers register in the shared `worker` table with `worker_role = sensor`; they can advertise `sensor.labels` and `sensor.taints` under `worker.capabilities.labels` / `worker.capabilities.taints` for sensor placement. Pack sensors may declare `worker_selector`, `worker_tolerations`, and `worker_affinity`; `SensorManager` evaluates those constraints before starting/restarting sensor processes. Managed sensor processes receive active rule instances via `ATTUNE_SENSOR_TRIGGERS` entries containing `id`, `ref`, `trigger_ref`, and `config`. The internal `attune-sensor` lifecycle path remains RabbitMQ-backed, while managed sensor processes now receive ongoing rule activate/deactivate deltas from `attune-notifier` over authenticated WebSocket subscriptions (sensor manager injects `ATTUNE_NOTIFIER_WS_URL`). The rule lifecycle listener also subscribes to `metadata.trigger.changed` and maintains a short-lived trigger-ref→id cache to reduce repeated trigger lookup queries during rule lifecycle handling. Sensor stdout/stderr are written to rotating files under `{artifacts_dir}/sensors/{sensor_ref}/` and exposed via `/api/v1/sensors/{sensor_ref}/logs/{stdout|stderr}?tail=N`. Managed sensor process health is persisted in `sensor_process` with history in `sensor_process_history`; unexpected exits are detected with non-blocking `try_wait`, moved to backoff, restarted with capped exponential delay while active rules still reference the sensor, and escalated via `core.alert` after repeated failures.
6. **attune-notifier**: Real-time notifications via PostgreSQL LISTEN/NOTIFY + WebSocket (port 8081)
   - **PostgreSQL listener**: Uses `PgListener::listen_all()` (single batch command) to subscribe to all configured channels, including execution, enforcement, inquiry, artifact, and work-queue notifications. **Do NOT use individual `listen()` calls in a loop** — this leaves the listener in a broken state where it stops receiving after the last call.
   - **Artifact notifications**: `artifact_created` and `artifact_updated` channels. Notification payloads include `classification` (`general` or `runtime_log`) so consumers can distinguish private raw stdout/stderr source-of-truth artifacts from normal artifacts without inspecting content. The `artifact_updated` trigger extracts a progress summary (`progress_percent`, `progress_message`, `progress_entries`) from the last entry in the `data` JSONB array for progress-type artifacts, and artifact-version notifications include the producing `execution` id so execution-scoped artifact lists refresh reliably for stdout/stderr log versions. The Web UI uses `useArtifactStream` hook to subscribe to `entity_type:artifact` notifications and invalidate React Query caches + push progress summaries to a `artifact_progress` cache key.
   - **Work queue notifications**: `work_queue_created`, `work_queue_updated`, `work_queue_item_created`, and `work_queue_item_updated` channels. Queue pages subscribe via `useQueueStream` and invalidate queue definition/detail/item queries when definitions or items change.
   - **WebSocket protocol** (client → server): `{"type":"subscribe","filter":"entity:execution:<id>"}` — filter formats: `all`, `entity_type:<type>`, `entity:<type>:<id>`, `user:<id>`, `notification_type:<type>`
   - **WebSocket protocol** (server → client): All messages use `#[serde(tag="type")]` — `{"type":"welcome","client_id":"...","message":"..."}` on connect; `{"type":"notification","notification_type":"...","entity_type":"...","entity_id":...,"payload":{...},"user_id":null,"timestamp":"..."}` for notifications; `{"type":"error","message":"..."}` for errors
   - **Key invariant**: The outgoing task in `websocket_server.rs` MUST wrap `Notification` in `ClientMessage::Notification(notification)` before serializing — bare `Notification` serialization omits the `"type"` field and breaks clients
   - **Authentication (required)**: WebSocket connections MUST present a JWT at connect time without putting it in the URL. Non-browser clients use `Authorization: Bearer <jwt>`. Browser clients, which cannot set arbitrary WebSocket headers, send two subprotocol values: `attune.v1` and `attune.jwt.<jwt>`; the server selects `attune.v1` and extracts the token from the secondary protocol value. Query-string tokens (`?token=...` / `?access_token=...`) are intentionally not accepted because URLs are commonly logged by proxies and access logs. The notifier verifies the token against `security.jwt_secret` (same secret as the API) before calling `ws.on_upgrade(...)`; missing, invalid, or expired tokens are rejected with **HTTP 401** before the WebSocket handshake completes. Only `TokenType::Access` and `TokenType::Execution` are accepted — `Refresh`, `Sensor`, and `Worker` tokens are rejected. The verified `identity_id` (from `claims.sub`) is attached to the `Subscriber` and used for filter ACL. **Mid-connection token expiration is enforced**: each per-connection task ticks every 30 seconds and, once `now >= claims.exp`, sends a Close frame with code **4401** and reason `"token expired"` then tears down the connection. Clients (the web UI already does this) should refresh their token and reconnect on close code 4401.
   - **Role lookup at connect**: After token verification, the handler calls `IdentityRoleAssignmentRepository::find_role_names_by_identity(&pool, identity_id)` against a small dedicated `PgPool` (max 4 connections — the notifier's primary DB workload is the `PostgresListener`'s LISTEN/NOTIFY connection, role lookups are infrequent). DB errors fail-closed with HTTP 500 — a flaky DB must not silently grant admin. The resulting `Vec<String>` is stored on the `Subscriber` and used by the filter ACL. Roles are captured **once at connect time** and not refreshed mid-connection; identities that gain or lose `admin` must reconnect for the change to apply. Mid-connection JWT expiration enforcement (above) bounds the staleness window to one access-token lifetime.
   - **Server-side filter ACL** (in `websocket_server::filter_allowed_for_identity`): `SubscriptionFilter::User(other_id)` is allowed only when `other_id == identity_id`, or when the connecting identity holds the `admin` role (admin status is determined via `IdentityRoleAssignmentRepository::find_role_names_by_identity` at connect time; identities with role name `admin` bypass User-filter ACL). Operational subscriptions are RBAC-gated: `entity_type:event` / `entity:event:<id>` require `events:read`, `entity_type:enforcement` / `entity:enforcement:<id>` require `enforcements:read`, and `entity_type:execution` / `entity:execution:<id>` require `executions:read`; entity-id filters load the target record so constrained grants can match IDs/refs/pack refs. `all` requires admin or broad read grants for events, enforcements, and executions. Rejected subscribes do **not** add the filter — the server replies with `{"type":"error","message":"Unauthorized to subscribe to requested filter"}` and a `warn!` log entry. Malformed JSON and unparseable filters are also reported via `Error` frames without closing the connection.


7. **attune-supervisor**: Platform maintenance service for runtime database retention and cross-cutting cleanup/monitoring. Owns configurable purging of events, enforcements, executions, history hypertables, audit events, notifications, webhook logs, inquiries, work-queue runtime rows, pack test execution rows, stale admission state, inactive workers, and stopped sensor processes. It also enforces time-based artifact version retention (`minutes`/`hours`/`days`) by deleting expired `artifact_version` rows and file-backed content, emits deduplicated `core.alert` events for stuck non-terminal executions / stale queue leases / stale queue dispatches, and alerts when retention-eligible rows remain beyond their policy plus a grace window. Corrective remediation is enabled by default: after a larger grace window the supervisor terminalizes stale executions (`canceling` -> `cancelled`, stale requested/scheduling/scheduled/running -> `abandoned`), releases stale work-queue leases/dispatches, reconciles execution-admission entries, promotes queued admission entries when capacity opens, and synchronizes stale workflow rows whose parent/children are already terminal. The supervisor records durable lifecycle rows in `supervisor_run`; a row is inserted with `clean_shutdown = false` after the maintenance advisory lock is acquired and marked clean only on graceful shutdown, so the next lock-owning boot can detect a prior dirty shutdown and label the first cycle as `dirty_shutdown_recovery`. When RabbitMQ is configured, it publishes lifecycle wakeups for corrected executions/promoted admission entries; if MQ is unavailable it still corrects DB state and emits alerts/audit records. The supervisor reloads the persisted `runtime_retention_config` / `runtime_retention_target_config` rows each cycle so API/web retention changes apply without restart, uses PostgreSQL advisory locks so accidental multiple replicas skip duplicate cycles safely, and writes audit events (`maintenance.retention.target_completed` / `maintenance.retention.target_failed`, `maintenance.artifact.cleanup_completed`, `maintenance.corrective_action.applied`) for purge/corrective activity plus `maintenance.retention.config_updated` for authenticated config changes.

**Communication**: Services communicate via RabbitMQ for async operations; `attune-supervisor` is primarily DB-driven but uses RabbitMQ when configured to publish lifecycle wakeups after corrective remediation. MQ exchanges include `attune.events`, `attune.executions`, `attune.notifications`, and `attune.metadata` (metadata-change/invalidation events such as `metadata.action.changed`, `metadata.trigger.changed`, `metadata.runtime.changed`, `metadata.pack.changed`, `metadata.permission_set.changed`, and `metadata.identity_authorization.changed`).

**Optional integration services**:
- **attune-mcp**: MCP adapter over the existing API for AI agents and external harnesses. The `attune-mcp` binary lives in `crates/cli`, supports both stdio and HTTP transports, exposes `POST /mcp` plus `GET /health` in HTTP mode, and can authenticate non-interactively with `ATTUNE_API_TOKEN` (execution-scoped), `ATTUNE_AUTH_TOKEN` / `ATTUNE_REFRESH_TOKEN`, or `ATTUNE_LOGIN` / `ATTUNE_PASSWORD`.

## Docker Compose Orchestration

**All Attune services run via Docker Compose.**

- **Compose file**: `docker-compose.yaml` (root directory)
- **Configuration**: `config.docker.yaml` (Docker-specific settings, including `artifacts_dir: /opt/attune/artifacts`)
- **Logging defaults**: Docker/distributable configs default `log.format` to `json` for structured stdout; `config.development.yaml` uses `pretty` for readable local runs. `log.console` / `log.file` remain reserved config fields and do not currently change log sinks. The canonical operator contract is documented in `docs/deployment/structured-logging.md`: forwarded service logs come from container `stdout`/`stderr` plus Compose labels, while raw execution/sensor stdout/stderr remains in private artifact-backed `runtime_log` artifacts.
- **Default user**: `test@attune.local` / `TestPass123!` (auto-created)

**Services**:
- **Infrastructure**: postgres (TimescaleDB), rabbitmq
- **Init** (run-once): migrations, init-user, init-pack-binaries, init-packs, init-agent
- **Application**: api (8080), executor, supervisor, worker-{shell,python,node,full}, sensor, notifier (8081), web (3000)
- **Optional profile**: mcp (8090) — `docker compose --profile mcp up -d mcp`

**Volumes** (named):
- `postgres_data`, `rabbitmq_data` — infrastructure state
- `packs_data` — pack files (shared across all services)
- `runtime_envs` — isolated runtime environments (virtualenvs, node_modules)
- `artifacts_data` — file-backed artifact storage (shared between API, workers, sensor, and executor with read-write access; the executor writes workflow activity logs)
- `agent_bin` — statically-linked injected binaries populated by `init-agent` (`attune`, `attune-agent`, `attune-sensor-agent`, and `attune-mcp`), mounted read-only by agent workers and API for binary download / execution-local MCP access
- `*_logs` — per-service log volumes

**Commands**:
```bash
docker compose up -d          # Start all services
docker compose down           # Stop all services
docker compose logs -f <svc>  # View logs
docker compose --profile mcp up -d mcp  # Start optional MCP HTTP service
docker compose -f docker-compose.yaml -f docker-compose.agent.yaml up -d  # Start with agent workers
```

**Key environment overrides**: `JWT_SECRET`, `ENCRYPTION_KEY` (required for production)

### Docker Build Optimization
- **Active Dockerfiles**: `docker/Dockerfile.optimized`, `docker/Dockerfile.mcp`, `docker/Dockerfile.agent`, `docker/Dockerfile.web`, and `docker/Dockerfile.pack-binaries`
- **Agent Dockerfile** (`docker/Dockerfile.agent`): Builds statically-linked `attune-agent`, `attune-sensor-agent`, `attune-mcp`, and `attune` CLI binaries using musl. Uses `cargo-zigbuild` (zig as the cross-compilation backend) so that any target architecture can be built from any host — e.g., building `aarch64-unknown-linux-musl` on an x86_64 host or vice versa. The `RUST_TARGET` build arg controls the output architecture; when omitted, the Dockerfile auto-selects the native Linux musl target from the build/container architecture (`x86_64-unknown-linux-musl` on amd64, `aarch64-unknown-linux-musl` on arm64). Three stages: `builder` (cross-compile with cargo-zigbuild), `agent-binary` (scratch — just the binaries), `agent-init` (busybox — for volume population via `cp`). The binaries have zero runtime dependencies (no glibc, no libssl). Build with `make docker-build-agent` (native arch by default), `make docker-build-agent-arm64` (arm64), or `make docker-build-agent-all` (both). In `docker-compose.yaml`, set `AGENT_RUST_TARGET` explicitly only when you need to override the auto-detected native target.
- **Pack Binaries Dockerfile** (`docker/Dockerfile.pack-binaries`): Builds statically-linked pack binaries (sensors, etc.) using musl + cargo-zigbuild for cross-compilation. The `RUST_TARGET` build arg controls the output architecture; when omitted, the Dockerfile auto-selects the native Linux musl target from the build/container architecture (`x86_64-unknown-linux-musl` on amd64, `aarch64-unknown-linux-musl` on arm64). Three stages: `builder` (cross-compile with cargo-zigbuild), `output` (scratch — just the binaries for `docker cp` extraction), `pack-binaries-init` (busybox — for Docker Compose volume population via `cp`). Build with `make docker-build-pack-binaries` (native arch by default), `make docker-build-pack-binaries-arm64` (arm64), or `make docker-build-pack-binaries-all` (both). In `docker-compose.yaml`, set `PACK_BINARIES_RUST_TARGET` explicitly only when you need to override the auto-detected native target. The `init-pack-binaries` Docker Compose service automatically builds and copies pack binaries into the `packs_data` volume before `init-packs` runs.
- **GitHub Actions binary bundle publishing** (`.github/workflows/publish.yml`): Rust binary bundles are exchanged between publish jobs with `actions/upload-artifact` / `actions/download-artifact`, avoiding dependency on Gitea generic packages. Tag builds attach the per-architecture `attune-binaries-{amd64,arm64}.tar.gz` bundles directly to the GitHub Release.
- **GitHub Actions image and Helm publishing** (`.github/workflows/publish.yml`): Images publish to GHCR by default (`ghcr.io`, namespace from `CONTAINER_REGISTRY_NAMESPACE` or the repository owner). Workflows set `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24=true` and use Node 24-compatible first-party action majors (`checkout@v6`, `cache@v5`, `upload-artifact@v7`, `download-artifact@v8`, `setup-node@v6`, `setup-python@v6`). Publish jobs run on pinned GitHub-hosted `ubuntu-24.04` runners, Rust bundles are explicitly cross-targeted (`x86_64-*` for amd64, `aarch64-*` for arm64) and verified with `file`, and Docker image builds pass explicit Buildx platforms (`linux/amd64` / `linux/arm64`) with QEMU enabled for cross-platform builds. Per-architecture Docker Buildx pushes keep `--provenance=false --sbom=false` for lean registry manifests. The Helm chart is published as an OCI chart under `ghcr.io/<namespace>/attune/charts`. The Helm chart depends on bootstrap hook images (`attune/migrations`, `attune/init-user`, and `attune/init-packs`) as well as app images; keep these in the publish matrix and manifest matrix so Kubernetes installs can complete migrations, default-user bootstrap, and core-pack bootstrap.
- **GitHub Actions Linux package publishing** (`.github/workflows/publish.yml`): Debian and RPM packages are uploaded directly to Nexus Repository Manager 3 hosted repositories using `NEXUS_URL`, `NEXUS_USERNAME`, `NEXUS_PASSWORD`, and optional repository-name variables (`NEXUS_APT_REPOSITORY`, `NEXUS_YUM_REPOSITORY`). Arch `.pkg.tar.zst` packages are uploaded to `NEXUS_RAW_REPOSITORY` when that optional raw repository variable is set. Branch-build Linux package versions must start with a digit and sort monotonically for package-manager compatibility, so SHA image tags like `sha-...` are converted to package versions like `0.0.0.git.<run-number>.sha.<short-sha>`. The package set includes split packages (`attune-api`, `attune-executor`, `attune-notifier`, `attune-supervisor`, `attune-cli`, `attune-agent`) plus a self-contained all-in-one `attune` package that installs API/executor/worker/sensor/notifier/supervisor service binaries, CLI/MCP binaries, and agent binaries together under `/opt/attune-system/` and ships matching systemd units. The all-in-one package conflicts with split packages to avoid duplicated file ownership.
- **Strategy**: Selective crate copying - only copy crates needed for each service (not entire workspace)
- **Performance**: 90% faster incremental builds (~30 sec vs ~5 min for code changes)
- **BuildKit cache mounts**: Persist cargo registry and compilation artifacts between builds
  - **Cache strategy**: `sharing=shared` for registry/git (concurrent-safe), service-specific IDs for target caches
  - **Parallel builds**: 4x faster than old `sharing=locked` strategy - no serialization overhead
- **Rustc stack size**: All Rust Dockerfiles set `ENV RUST_MIN_STACK=67108864` (64 MiB) in the build stage to prevent `rustc` SIGSEGV crashes during release compilation. The `Makefile` also exports this variable for local builds.
- **Documentation**: See `docs/docker-layer-optimization.md`, `docs/QUICKREF-docker-optimization.md`, `docs/QUICKREF-buildkit-cache-strategy.md`

### Docker Runtime Standardization
- **Base image**: All worker and sensor runtime stages use `debian:bookworm-slim` (or `debian:bookworm` for worker-full)
- **Python**: Always installed via `apt-get install python3 python3-pip python3-venv` → binary at `/usr/bin/python3`
- **Node.js**: Always installed via NodeSource apt repo (`setup_${NODE_VERSION}.x`) → binary at `/usr/bin/node`
- **NEVER** use `python:` or `node:` Docker images as base — they install binaries at `/usr/local/bin/` which causes broken venv symlinks when multiple containers share the `runtime_envs` volume
- **UID**: All containers use UID 1000 for the `attune` user
- **Venv creation**: Uses `--copies` flag (`python3 -m venv --copies`) to avoid cross-container broken symlinks
- **Worker targets**: `worker-base` (shell), `worker-python` (shell+python), `worker-node` (shell+node), `worker-full` (all)
- **Sensor targets**: `sensor-base` (native only), `sensor-full` (native+python+node)

### Packs Volume Architecture
- **Key Principle**: Packs are NOT copied into Docker images - they are mounted as volumes
- **Volume Flow**: Host `./packs/` → `init-packs` service → `packs_data` volume → mounted in all services
- **Benefits**: Update packs with restart (~5 sec) instead of rebuild (~5 min)
- **Pack Binaries**: Automatically built and deployed via the `init-pack-binaries` Docker Compose service (statically-linked musl binaries via cargo-zigbuild, supports cross-compilation via `PACK_BINARIES_RUST_TARGET` env var). Can also be built manually with `./scripts/build-pack-binaries.sh` or `make docker-build-pack-binaries`. The `init-packs` service depends on `init-pack-binaries` and preserves any ELF binaries already present in the target `sensors/` directory (detected via ELF magic bytes with `od`) — it backs them up before copying host pack files and restores them afterward, preventing the host's stale dynamically-linked binary from overwriting the freshly-built static one.
- **Development**: Use `./packs.dev/` for instant testing (direct bind mount, no restart needed)
- **Documentation**: See `docs/QUICKREF-packs-volumes.md`

### Runtime Environments Volume
- **Key Principle**: Runtime environments (virtualenvs, node_modules) are stored OUTSIDE pack directories
- **Volume**: `runtime_envs` named volume mounted at `/opt/attune/runtime_envs` in worker, sensor, and API containers
- **Path Pattern**: `{runtime_envs_dir}/{pack_ref}/{runtime_name}` (e.g., `/opt/attune/runtime_envs/python_example/python`)
- **Creation**: Worker creates environments proactively at startup and via `pack.registered` MQ events; lightweight existence check at execution time
- **Broken venv auto-repair**: Worker detects broken interpreter symlinks (e.g., from mismatched container python paths) and automatically recreates the environment
- **API best-effort**: API attempts environment setup during pack registration but logs and defers to worker on failure (Docker API containers lack interpreters)
- **Pack directories remain read-only**: Packs mounted `:ro` in workers; all generated env files go to `runtime_envs` volume
- **Config**: `runtime_envs_dir` setting in config YAML (default: `/opt/attune/runtime_envs`)

## Domain Model & Event Flow

**Critical Event Flow**:
```
Sensor → Trigger fires → Event created → Rule evaluates →
Enforcement created → Execution scheduled → Worker executes Action

For workflows:
Execution requested → Scheduler detects workflow_def → Loads definition →
Creates workflow_execution record → Dispatches entry-point tasks as child executions →
Completion listener advances workflow → Schedules successor tasks → Completes workflow
```

**Key Entities** (all in `public` schema, IDs are `i64`):
- **Pack**: Bundle of automation components (actions, sensors, rules, triggers, runtimes). API-created non-standard packs are scoped to the creating identity via `pack.installed_by`; system/standard packs remain globally visible. Pack permission specs are `read`, `create` (empty packs), `install` (pack upload/register/remote or index install), `configure` (pack metadata/config/index configuration), and `delete`. Cross-owner pack list/detail/configure/delete access requires a meaningful constrained `packs:*` grant (for example owner, pack ref, ref, or id constraints) rather than a broad unconstrained admin grant.
- **Runtime**: Unified execution environment definition (Python, Shell, Node.js, etc.) — used by both actions and sensors. Configured via `execution_config` JSONB (interpreter, environment setup, dependency management, env_vars). No type distinction; whether a runtime is executable is determined by its `execution_config` content.
- **RuntimeVersion**: A specific version of a runtime (e.g., Python 3.12.1, Node.js 20.11.0). Each version has its own `execution_config` and `distributions` for version-specific interpreter paths, verification commands, and environment setup. Actions and sensors can declare an optional `runtime_version_constraint` (semver range) to select a compatible version at execution time.
- **Trigger**: Event type definition (e.g., "webhook_received"). Disabled triggers remain registered but reject all new event ingress: direct sensor/execution-token event creation and webhook event creation both fail before inserting an `event` row. Sensor lifecycle queries also treat enabled rules behind disabled triggers as inactive, so disabling a trigger can stop/reconcile its backing sensor. Triggers have `reference_visibility` (`public`, `private`, or `restricted`) plus `reference_allowed_pack_refs` to control which packs may create rules that subscribe to them; omitted visibility defaults to `public`, same-pack rules are always allowed, `private` allows only the trigger's own pack, and `restricted` allows the trigger's own pack plus the allow-list. API and pack-loader create/update paths validate rule subscriptions against this policy, trigger list/detail APIs accept `referencing_pack_ref` to reveal allowed restricted triggers for rule selection, and visibility updates are rejected if they would invalidate existing rules.
- **Sensor**: Monitors for trigger conditions, creates events. Disabled sensors remain registered but are excluded from startup/restart; API enabled-state changes publish lifecycle wakeups through existing rule lifecycle messages so running sensor processes stop when a sensor is disabled and start/reconcile when re-enabled.
- **Event**: Instance of a trigger firing with payload. Event payload/config fields marked `secret: true` by the trigger schema are redacted at direct sensor/execution-token ingress and webhook ingress before insertion; the event row stores `$attune_secret` markers while encrypted values are stored in `execution_secret_value` under `event_payload` / `event_config`. New events always persist a `trace_tag`: explicit request values are normalized, execution-token requests inherit the parent execution trace when present, and any remaining missing trace is auto-generated (`event-<trigger_ref>-<timestamp_ms>`). `events:read` returns redacted operational metadata; `GET /api/v1/events/{id}?include_secret_values=true` requires `events:decrypt`, restores the side-table values, and emits `secret.event_values.decrypted`.
- **Worker**: Action and sensor workers share the `worker` table. `worker.status` is the observed lifecycle state, while `worker.cordoned`, `cordon_reason`, `cordoned_by`, and `cordoned_at` represent operator intent. Cordoned workers may still be active/heartbeating but are excluded from new scheduling and suppress unexpected-offline `core.alert` noise.
- **SensorProcess**: Durable live state for managed pack sensor processes in `sensor_process`, plus field-level `sensor_process_history` hypertable entries. Tracks owning sensor/worker, process status (`starting`, `running`, `stopped`, `failed`, `backoff`), pid, consecutive failures, last exit code/signal, start/stop timestamps, next restart time, stderr excerpt, active-rule count, log artifact ref, and alert bookkeeping. `SensorManager` updates this table on start/stop/exit/restart and uses alert markers to avoid repeated `core.alert` events for the same failure count.
- **Action**: Executable task with parameters. Actions have an `enabled` flag (pack YAML / API / DB) that defaults to `true` for new records; pack reloads preserve the existing enabled state when YAML omits the field, and disabled actions are rejected for manual execution / failed as unschedulable by the executor. Actions have `reference_visibility` (`public`, `private`, or `restricted`) plus `reference_allowed_pack_refs` to control which other packs may reference them from rules, workflow tasks, and work queues; omitted visibility defaults to `public`, same-pack references are always allowed, `private` allows only the action's own pack, and `restricted` allows the action's own pack plus the allow-list. API and pack-loader create/update paths validate rule, workflow, and queue references against this policy, and action list/search/detail APIs hide private/restricted actions unless the caller has management access (`actions:update`) on the action-owning pack or supplies an allowed `referencing_pack_ref` context. Actions can declare `default_execution_permission_set_refs` (pack YAML / API / DB) to define the token access refs snapped onto execution-scoped API tokens by default. Omitted or empty defaults mean executions receive no Attune API token unless the execution request explicitly supplies refs. The reserved ref `standard` is not a database permission set; it grants execution tokens access to keys and artifacts scoped to the executing action and its pack. Actions may also declare an optional `timeout_seconds` default (pack YAML / API / DB; nullable, positive) that seeds `execution.timeout_seconds` when no explicit override or workflow task timeout applies; pack reloads preserve/clear it like other nullable action fields, and the API `UpdateActionRequest` uses a `TimeoutSecondsPatch` (`{op:set,value}` / `{op:clear}`) to set or clear it.
- **Policy**: First-class execution control resource scoped globally, to a pack, or to an action. The executor resolves **one effective policy** per execution: action-scoped policies override pack-scoped policies, pack-scoped policies override global policies, and same-scope matches use `priority DESC, created DESC`. Policies may configure optional concurrency (`threshold`/`limit`, `method = enqueue|cancel`, parameter grouping paths), rate limits (`rate_limit_max_executions`, `rate_limit_window_seconds`), and supported quota checks stored in `quotas` JSONB (`running_executions`, `executions_total`). Only enabled policies participate in enforcement. The API exposes `/api/v1/policies` plus pack/action convenience list routes and gates access through the `policies` RBAC resource. The web UI has first-class policy list/detail/create/edit routes with typed form sections for each policy feature and no bare JSON inputs. The CLI exposes structured `attune policy` commands with flags such as `--scope`, `--concurrency-limit`, `--on-concurrency`, `--group-by`, `--rate-limit-max`, `--rate-limit-window`, and quota flags.
- **Rule**: Links triggers to actions with conditional logic. Carries an optional `owner_identity` column referencing `identity(id)` (`ON DELETE SET NULL`) — set to the authenticated user's id by `POST /api/v1/rules`, NULL for system-loaded rules from the init pack loader. Rules may define nullable `permission_set_refs` to override the target action's default execution API-token permission refs for rule-triggered executions; omitted/NULL inherits the action default and an empty array forces no execution token. Rules may also define nullable `trace_tag_template`; when unset, rule-triggered executions default to `<trigger_ref>.<event_id>`. The executor uses `rule.owner_identity` to attribute rule-triggered executions; legacy/system-loaded rules with NULL `owner_identity` fall back to the system identity (id 1) at execution-creation time.
- **Enforcement**: Represents a rule activation. Enforcement payload is a copy of the stored event payload, so event-secret fields remain redacted and are not restored by the enforcement decrypt path. Enforcement-triggered execution trace tags prefer the source event trace tag first (to preserve end-to-end lineage), then fall back to rendered `trace_tag_template` only when the event trace is unavailable, then `<trigger_ref>.<event_id>` as a final fallback. Enforcement config redaction/reveal remains action-schema based; `GET /api/v1/enforcements/{id}?include_secret_values=true` requires `enforcements:decrypt` and restores only enforcement config secrets.
- **Execution**: Single action run; supports parent-child relationships for workflows. The `execution.permission_set_refs` column stores the token access refs snapshotted at creation time for the execution-scoped API token. Empty refs mean the worker omits `ATTUNE_API_TOKEN`; non-empty refs are embedded into the execution JWT metadata and authorization uses only those refs, not the triggering identity's full role/permission assignments. Named refs load database permission sets. The reserved `standard` ref expands at authorization time to action/pack-scoped key and artifact grants. Executions snapshot a `trace_tag` for end-to-end correlation: caller-provided/inherited tags are preserved, and missing tags are auto-generated at creation (`execution-<action_ref>-<timestamp_ms>`). Worker processes receive it via `ATTUNE_TRACE_TAG`.
  - **Dead-worker reconciliation**: The executor timeout monitor reconciles `running` executions on stale/inactive/error workers to `ExecutionStatus::Abandoned` with structured result metadata and publishes the normal `ExecutionCompleted` message. These executions are not restarted automatically.
  - **Execution timeouts**: The `execution.timeout_seconds` column stores the resolved per-execution timeout (seconds), **snapshotted at creation time** for auditability/reproducibility. Resolution order: explicit per-execution override → workflow task `timeout_seconds` (for workflow child executions) → `action.timeout_seconds` default → app-level `default_execution_timeout_seconds` config (default 600). All `CreateExecutionInput` creation sites (manual API, rules/enforcements, queue dispatch, workflow children/retries) perform this snapshot; the executor reads the process-global `attune_common::config::app_default_execution_timeout_seconds()` (set at each service startup). The column is immutable after creation (not in `UpdateExecutionInput` nor the `execution_history` trigger). The worker reads the snapshot for `ExecutionContext.timeout` and execution-token TTL, with a defensive fallback (`execution.timeout_seconds` → `workflow_task.timeout_seconds` → `action.timeout_seconds` → app default) only for legacy/NULL rows. On timeout the worker terminates the process group with SIGTERM, waits 10 seconds, then SIGKILL (both process and native runtimes), and persists `ExecutionStatus::Timeout` (not `Failed`).
  - **Workflow Tasks**: Workflow-specific metadata stored in `execution.workflow_task` JSONB field
- **Inquiry**: Human-in-the-loop async interaction (approvals, inputs). `assigned_to` is **enforced** — only the assignee may respond/cancel. Tokens without a resolvable identity are rejected with 403. Additionally, an execution that created an inquiry (e.g., via `core.ask`) cannot respond to it using its own execution-scoped token (privilege-loop guard, comparing the token's `execution_id` claim against `inquiry.execution`).
- **Identity**: User/service account with RBAC permissions
- **Key**: Secrets/config storage. The `value` column is JSONB — keys can store strings, objects, arrays, numbers, or booleans. Keys are **unencrypted by default**; use `--encrypt`/`-e` (CLI) or `"encrypted": true` (API) to encrypt. When encrypted, the JSON value is serialised to a compact string, encrypted with AES-256-GCM, and stored as a JSON string; decryption reverses this. The `encrypt_json`/`decrypt_json` helpers in `attune_common::crypto` handle this — **all services use this single shared implementation** (the worker's `SecretManager` delegates directly to `attune_common::crypto::decrypt_json`; it no longer has its own bespoke encryption code). The ciphertext format is `BASE64(nonce_bytes ++ ciphertext_bytes)` everywhere. Identity-owned keys are visible/decryptable/mutable only by the owning identity unless a meaningful constrained `keys:*` grant matches the key owner/scope; broad unconstrained grants do not reveal another identity's keys. Execution tokens are subject to the same key authorization path as access tokens. The reserved execution access ref `standard` grants `keys:read` and `keys:decrypt` only for Pack- and Action-scoped keys whose owner ref matches the executing action/pack or the containing workflow action/pack. The worker's `SecretManager` returns `HashMap<String, JsonValue>` and secrets are merged directly into action parameters (no `Value::String` wrapping). The workflow `keystore` namespace already uses `JsonValue`, so structured secrets are natively accessible (e.g., `{{ keystore.db_credentials.password }}`). The CLI `key show` command displays a SHA-256 hash of the value by default; pass `--decrypt`/`-d` to reveal the actual value.
- **Artifact**: Tracked output from executions (files, logs, progress indicators). Metadata + optional structured `data` (JSONB). Linked to execution via plain BIGINT (no FK). Supports retention policies (version-count or time-based). File-type artifacts (FileBinary, FileDataTable, FileImage, FileText) use disk-based storage on a shared volume; Progress and Url artifacts use DB storage. Each artifact has both a `visibility` field (`ArtifactVisibility` enum: `public` or `private`, DB default `private`) and a `classification` field (`ArtifactClassification` enum: `general` or `runtime_log`, DB default `general`). Runtime log classification is used for execution stdout/stderr and sensor stdout/stderr artifacts so operators and downstream observability can discover them by metadata without exporting raw content; classified `runtime_log` artifacts are forced to remain private. **Type-aware API default**: when `visibility` is omitted from `POST /api/v1/artifacts`, the API defaults to `public` for Progress artifacts (informational status indicators anyone watching an execution should see) and `private` for all other types; runtime-log refs remain private even if a caller attempts to set them public. **Non-log artifact retention defaults**: action and sensor rows may set nullable `artifact_retention_policy` / `artifact_retention_limit`; manual execution requests may set per-execution `artifact_retention_policy` / `artifact_retention_limit`, snapshotted onto the `execution` row and propagated to workflow children/retries. Artifact create/upsert routes use explicit request retention first, then execution override, then action/sensor default, then API fallback (`versions` / `5` for normal create, `versions` / `10` for by-ref upsert helpers). Execution stdout/stderr and sensor stdout/stderr log artifacts are intentionally controlled by the separate log-retention fields. **Authorization** (enforced in `routes/artifacts.rs`): Public artifacts are viewable by any identity holding a matching `artifacts:<action>` grant. For private artifacts, an *unconstrained* `artifacts:<action>` grant is **not** sufficient on its own — access requires (a) a *constrained* `artifacts:<action>` grant whose constraints (`owner_types`, `owner_refs`, `pack_refs`, `refs`, `ids`, `visibility`) match the artifact, or (b) for `Identity`-scoped artifacts: the caller is the owner, or (c) for `Pack`/`Action`/`Sensor`-scoped artifacts: the caller holds `packs:read` (reads) or `packs:configure` (writes) on the derived pack. Execution tokens do not get artifact access merely because a token exists. The reserved execution access ref `standard` grants `artifacts:read/create/update/delete` only for Pack- and Action-scoped artifacts whose owner ref matches the executing action/pack or the containing workflow action/pack. List/search endpoints can filter by both `visibility` and `classification`, and they still post-filter results so unauthorized rows never leak.
- **ArtifactVersion**: Immutable content snapshot for an artifact. File-type versions store a `file_path` (relative path on shared volume) with `content` BYTEA left NULL. DB-stored versions use `content` BYTEA and/or `content_json` JSONB. Version number auto-assigned via `next_artifact_version()`. Retention trigger auto-deletes oldest versions beyond limit. Invariant: exactly one of `content`, `content_json`, or `file_path` should be non-NULL per row.
- **WorkQueue**: First-class business queue definition for durable user-visible work items. Supports both pack-owned declarative queues (`pack`, `pack_ref`, `is_adhoc = false`) and API/UI-managed queues (`is_adhoc = true`). Stores dispatch target action metadata, default priority, pending-update policy, batch mode, nullable `permission_set_refs` for dispatch execution API-token permission overrides, nullable `trace_tag_template`, and typed JSON config for tunable resolution, optional batch coalescing, sequential dispatch cooldown, queue-item retry limits, and ack-contract metadata. Omitted/NULL queue `permission_set_refs` inherits the dispatch action default and an empty array forces no execution token. Queue dispatch execution trace tags default to `<queue_ref>.<work_item_id>` in single mode and `<queue_ref>.<dispatch_id>` in batch mode when `trace_tag_template` is unset. Queues have `reference_visibility` (`public`, `private`, or `restricted`) plus `reference_allowed_pack_refs` to control which packs may discover/target the queue for item submission; omitted visibility defaults to `public`, same-pack targeting is always allowed, `private` allows only the queue's own pack, and `restricted` allows the queue's own pack plus the allow-list. Queue list/detail APIs accept `referencing_pack_ref` for discovery, but item write endpoints do not trust caller-supplied pack context: execution-scoped calls use server-derived execution pack context, and direct API callers need explicit constrained `queue_items:*` grants or queue-management access for private/restricted queues. Batch coalescing lives at `config.dispatch.coalescing` with fields `enabled`, `group_by_path`, and `across_priorities`. Sequential cooldown lives at `config.dispatch.inter_execution_delay_seconds` and is only enforced when resolved concurrency is `1`; the cooldown window begins after the prior queue dispatch reaches a terminal state. Queue retry limiting lives at `config.dispatch.retry_limit` and defaults to `0`, meaning any would-be retry result is promoted to `failed` after the first unsuccessful attempt.
- **WorkQueueItem**: Durable queued business record linked to a `work_queue`. Stores `item_key`, priority, payload, metadata, trace-tag provenance, enqueue-source provenance, optional initiating identity/execution/enforcement references, lease bookkeeping (`lease_token`, `lease_expires_at`, `leased_execution`), attempt count, and final ack/error summaries. Queue items persist a source `trace_tag`: explicit request values are normalized, execution-token enqueue requests inherit the parent execution trace when present, and any remaining missing trace is auto-generated (`queue-item-<queue_ref>-<timestamp_ms>`). Queue item creation via the public API stamps `enqueue_source = "api"` server-side; callers cannot set it explicitly. Queue item APIs use the dedicated RBAC resource `queue_items` (`read`, `create`, `update`, `delete`), separate from queue definition permissions on `queues`. Selector-based queue item maintenance uses PostgreSQL SQL/JSONPath (`jsonb_path_exists`) against a document containing `payload`, `metadata`, `item_key`, `priority`, `status`, `enqueue_source`, and `attempt_count`; preview/apply endpoints operate only on mutable pending items (`queued`/`retry`), with bulk cancel setting `status = cancelled`, bulk payload changes using JSON Merge Patch, and bulk priority changes assigning one static priority.
- **WorkQueueDispatch**: Lineage record connecting a leased queue batch to the execution that consumes it. Uses a plain BIGINT `execution` column (no FK — execution is a hypertable) and tracks dispatch status plus leased item count. Queue lifecycle triggers are emitted by the executor: `core.queue_started` after the first published dispatch for a queue whose latest lifecycle state was empty/unknown, and `core.queue_empty` after the last queue-processing execution terminates with no active dispatches and no queued/retry items remaining.
- **System Alert**: The core pack defines canonical trigger `core.alert` (`packs/core/triggers/alert.yaml`) for operational exceptions. Components use `attune_common::system_alert` to create an `event` row with `trigger_ref = 'core.alert'` and, when RabbitMQ is configured, publish `EventCreated`; current emitters include unexpected non-cordoned worker loss, execution abandonment caused by worker loss, repeated managed sensor-process failures while active rules depend on the sensor, and supervisor stuck-state / retention-lag / corrective-remediation alerts. Alert payloads follow the core trigger contract: severity, category, failure type, component type/id/ref, worker role, observed timestamp, summary, details, and correlation id.

## Key Tools & Libraries

### Shared Dependencies (workspace-level)
- **Async**: tokio, async-trait, futures
- **Web**: axum, tower, tower-http
- **Database**: sqlx (with postgres, json, chrono, uuid features)
- **Serialization**: serde, serde_json, serde_yaml_ng
- **Version Matching**: semver (with serde feature)
- **Logging**: tracing, tracing-subscriber
- **Error Handling**: anyhow, thiserror
- **Config**: config crate (YAML + env vars)
- **Validation**: validator
- **Auth**: jsonwebtoken, argon2
- **CLI**: clap
- **OpenAPI**: utoipa, utoipa-swagger-ui
- **Message Queue**: lapin (RabbitMQ)
- **HTTP Client**: reqwest
- **Archive/Compression**: tar, flate2 (used for pack upload/extraction)
- **Testing**: mockall, tempfile, serial_test

### Web UI Dependencies
- **Framework**: React 19 + react-router-dom
- **State**: Zustand, @tanstack/react-query
- **HTTP**: axios (with generated OpenAPI client)
- **Styling**: Tailwind CSS
- **Icons**: lucide-react
- **Charting primitives**: D3 (`d3`) for scales, shapes, layouts, and color interpolation
- **Build**: Vite, TypeScript

## Configuration System
- **Primary**: YAML config files (`config.yaml`, `config.{env}.yaml`)
- **Overrides**: Environment variables with prefix `ATTUNE__` and separator `__`
  - Example: `ATTUNE__DATABASE__URL`, `ATTUNE__SERVER__PORT`, `ATTUNE__RUNTIME_ENVS_DIR`
- **Loading Priority**: Base config → env-specific config → env vars
- **Required for Production**: `JWT_SECRET`, `ENCRYPTION_KEY` (32+ chars)
- **Location**: Root directory or `ATTUNE_CONFIG` env var path
- **Key Settings**:
  - `default_execution_timeout_seconds` - Top-level app config for the fallback execution timeout (seconds, default `600`, must be `> 0`). Used as the last tier when snapshotting `execution.timeout_seconds` at creation time. Each service publishes this value into a process-global accessor (`attune_common::config::app_default_execution_timeout_seconds()`) at startup so executor static scheduling paths can resolve it. The legacy `worker.task_timeout` setting is deprecated in favor of per-execution snapshots.
  - `packs_base_dir` - Where pack files are stored (default: `/opt/attune/packs`)
  - `runtime_envs_dir` - Where isolated runtime environments are created (default: `/opt/attune/runtime_envs`)
  - `artifacts_dir` - Where file-backed artifacts are stored (default: `/opt/attune/artifacts`). Shared volume between API and workers.
  - Runtime database retention is stored in `runtime_retention_config` and `runtime_retention_target_config`, seeded by migration `20250101000014_runtime_retention_supervisor.sql` (30 days for runtime targets, 90 days for audit events), managed through `GET/PUT /api/v1/retention-config` and the web `/retention` page, and enforced by `attune-supervisor` without restart. Per-target purging is disabled by setting `max_age_seconds` to `null` (keep forever). The legacy YAML `retention` block is only a startup fallback for the supervisor loop if database config cannot be loaded.
  - `maintenance` - Supervisor maintenance thresholds for non-retention jobs: time-based artifact cleanup, stuck execution/queue monitoring, corrective execution/workflow/queue/admission remediation, retention-lag alerting, alert limits, and alert cooldowns. These are YAML/env settings and require supervisor restart when changed.
  - `worker.execution_log_retention_policy` / `worker.execution_log_retention_limit` - Retention settings for per-execution action stdout/stderr log artifacts. Defaults to `days` / `7`; policy supports `versions`, `days`, `hours`, or `minutes`. Individual action and sensor rows may override log retention via nullable `log_retention_policy` / `log_retention_limit` fields, configurable from the web action/sensor detail pages. Sensor log artifacts have an independent default of `versions` / `4`. Non-log artifact defaults use separate nullable `artifact_retention_policy` / `artifact_retention_limit` fields on action/sensor rows and optional execution-level overrides.
  - `agent.binary_dir` - Directory containing agent binary files for download endpoint (default: `/opt/attune/agent`). Optional — only needed if serving agent binaries via `GET /api/v1/agent/binary`.
  - `agent.bootstrap_token` - Optional shared secret for authenticating agent binary downloads. If set, requests must provide it via the `X-Agent-Token` header. Tokens are not accepted in query strings.

## Authentication & Security
- **Auth Type**: JWT (access tokens: 1h, refresh tokens: 7d, worker tokens: 24h, sensor tokens: 24h, execution tokens: 5min) plus revokable passwordless integration tokens for external integrations.
- **Password Hashing**: Argon2id
- **Protected Routes**: Use `RequireAuth(user)` extractor in Axum
- **API authz metadata caches**: `AuthorizationService` maintains short-lived in-process caches for identity role names, effective grants, and permission-set-by-ref lookups. Global kill switch: `ATTUNE_AUTHZ_CACHE_ENABLED` (default true). Per-cache toggles: `ATTUNE_AUTHZ_ROLE_CACHE_ENABLED`, `ATTUNE_AUTHZ_GRANTS_CACHE_ENABLED`, `ATTUNE_AUTHZ_PERMISSION_SET_CACHE_ENABLED` (default true). Optional shadow-read sampling compares cache hits against fresh DB reads with mismatch warnings via `ATTUNE_AUTHZ_CACHE_SHADOW_SAMPLE_RATE` (`0.0..=1.0`, default `0.0`).
- **Integration Tokens**: Administrators can create revokable opaque tokens for identities via `/api/v1/identities/{id}/integration-tokens`, the web Access Control identity detail page, or `attune auth token ...`. Plaintext tokens are shown only once; `integration_token.token_hash` stores a SHA-256 hash plus safe prefix/suffix metadata, expiry, last-use, and revocation fields. `POST /auth/token-login` exchanges the opaque token for normal session JWTs. Access JWTs keep `sub = identity.id` so existing RBAC works unchanged. Refresh JWTs for this flow use `sub = integration_token.id` and `scope = "integration_token"`; `/auth/refresh` resolves the token record and fails closed when it is revoked, expired, deleted, or the owning identity is frozen. Revocation is immediate for refresh and bounded by the access-token TTL for already-issued access JWTs.
- **External Identity Providers**: OIDC and LDAP are supported as optional login methods alongside local username/password. Both upsert an `identity` row on first login and store provider-specific claims under `attributes.oidc` or `attributes.ldap` respectively. The web UI login page adapts dynamically based on the `GET /auth/settings` response, showing/hiding each method. The `?auth=<provider_name>` query parameter overrides which method is displayed (e.g., `?auth=direct`, `?auth=sso`, `?auth=ldap`).
  - **OIDC** (`crates/api/src/auth/oidc.rs`): Browser-redirect flow using the `openidconnect` crate. Config: `security.oidc` in YAML. Routes: `GET /auth/oidc/login` (redirect to provider), `GET /auth/callback` (authorization code exchange). Identity matched by `attributes->'oidc'->>'issuer'` + `attributes->'oidc'->>'sub'`. Supports PKCE, ID token verification via JWKS, userinfo endpoint enrichment, and provider-initiated logout via `end_session_endpoint`.
  - **LDAP** (`crates/api/src/auth/ldap.rs`): Server-side bind flow using the `ldap3` crate. Config: `security.ldap` in YAML. Route: `POST /auth/ldap/login` (accepts `{login, password}`, returns `TokenResponse`). Two authentication modes: **direct bind** (construct DN from `bind_dn_template` with `{login}` placeholder) or **search-and-bind** (bind as service account → search `user_search_base` with `user_filter` → re-bind as discovered DN). Identity matched by `attributes->'ldap'->>'server_url'` + `attributes->'ldap'->>'dn'`. Supports STARTTLS, TLS cert skip (`danger_skip_tls_verify`), and configurable attribute mapping (`login_attr`, `email_attr`, `display_name_attr`, `group_attr`).
  - **Login Page Config** (`security.login_page`): `show_local_login`, `show_oidc_login`, `show_ldap_login` — all default to `true`. Controls which methods are visible by default on the web UI login page.
- **Secrets Storage**: AES-GCM encrypted in `key` table (JSONB `value` column) with scoped ownership. Supports structured values (objects, arrays) in addition to plain strings. All encryption/decryption goes through `attune_common::crypto` (`encrypt_json`/`decrypt_json`) — the worker's `SecretManager` no longer has its own crypto implementation, eliminating a prior ciphertext format incompatibility between the API (`BASE64(nonce++ciphertext)`) and the old worker code (`BASE64(nonce):BASE64(ciphertext)`). The worker stores the raw encryption key string and passes it to the shared crypto module, which derives the AES-256 key internally via SHA-256.
- **Audit Log**: Security/compliance events are stored in the `audit_event` TimescaleDB hypertable (migration `20250101000013_audit_log.sql`) and queried through `/api/v1/audit-events` with `audit_log:read`. Generic HTTP request auditing remains a fallback, but sensitive operations should emit semantic events using canonical names from `attune_common::audit::event_type` and include actor, outcome, resource id/ref, request/correlation metadata when available, and redacted details. Current semantic coverage includes RBAC denials, admin identity/role/permission changes, key read/decrypt/create/update/delete, artifact create/update/delete/download, pack create/update/delete/upload/register/install, audit-log reads, supervisor runtime-retention target completion/failure events, runtime retention config updates, manual execution requests, and DB-triggered execution lifecycle status changes. Never store raw passwords, tokens, key values, decrypted secrets, or artifact content in `audit_event.details`.
- **User Info**: Stored in `identity` table

## Code Conventions & Patterns

### General
- **Error Handling**: Use `attune_common::error::Error` and `Result<T>` type alias
- **Async Everywhere**: All I/O operations use async/await with Tokio
- **Module Structure**: Public API exposed via `mod.rs` with `pub use` re-exports

### Database Layer
- **Schema**: All tables use unqualified names; schema determined by PostgreSQL `search_path`
- **Production/Docker**: Uses the dedicated `attune` schema (configured via `database.schema: "attune"` or `ATTUNE__DATABASE__SCHEMA=attune`)
- **Tests**: Each test uses isolated schema (e.g., `test_a1b2c3d4`) for true parallel execution
- **Schema Resolution**: PostgreSQL `search_path` mechanism, NO hardcoded schema prefixes in queries
- **Models**: Defined in `common/src/models.rs` with `#[derive(FromRow)]` for SQLx
- **Repositories**: One per entity in `common/src/repositories/`, provides CRUD + specialized queries
- **Pattern**: Services MUST interact with DB only through repository layer (no direct queries)
- **Transactions**: Use SQLx transactions for multi-table operations
- **IDs**: All IDs are `i64` (BIGSERIAL in PostgreSQL)
- **Timestamps**: `created`/`updated` columns auto-managed by DB triggers
- **JSON Fields**: Use `serde_json::Value` for flexible attributes/parameters, including `execution.workflow_task` JSONB
- **Enums**: PostgreSQL enum types mapped with `#[sqlx(type_name = "...")]`
- **Workflow Tasks**: Stored as JSONB in `execution.workflow_task` (consolidated from separate table 2026-01-27)
- **FK ON DELETE Policy**: Historical records (executions) use `ON DELETE SET NULL` so they survive entity deletion while preserving text ref fields (`action_ref`, `trigger_ref`, etc.) for auditing. The `event`, `enforcement`, and `execution` tables are TimescaleDB hypertables, so they **cannot be the target of FK constraints** — `enforcement.event`, `execution.enforcement`, `inquiry.execution`, `workflow_execution.execution`, `execution.parent`, and `execution.original_execution` are plain BIGINT columns (no FK) and may become dangling references if the referenced row is deleted. Pack-owned entities (actions, triggers, sensors, rules, runtimes) use `ON DELETE CASCADE` from pack. Workflow executions cascade-delete with their workflow definition. `rule.owner_identity` references `identity(id)` with `ON DELETE SET NULL` so rule rows survive identity deletion (the executor falls back to the system identity when `owner_identity` is NULL).
- **Event Table (TimescaleDB Hypertable)**: The `event` table is a TimescaleDB hypertable partitioned on `created` (1-day chunks). Events are **immutable after insert** — there is no `updated` column, no update trigger, and no `Update` repository impl. The `Event` model has no `updated` field. Compression is segmented by `trigger_ref` (after 7 days) and retention is 90 days. The `event_volume_hourly` continuous aggregate queries the `event` table directly.
- **Enforcement Table (TimescaleDB Hypertable)**: The `enforcement` table is a TimescaleDB hypertable partitioned on `created` (1-day chunks). Enforcements are updated **exactly once** — the executor sets `status` from `created` to `processed` or `disabled` within ~1 second of creation, well before the 7-day compression window. The `resolved_at` column (nullable `TIMESTAMPTZ`) records when this transition occurred; it is `NULL` while status is `created`. There is no `updated` column. Compression is segmented by `rule_ref` (after 7 days) and retention is 90 days. The `enforcement_volume_hourly` continuous aggregate queries the `enforcement` table directly.
- **Execution Table (TimescaleDB Hypertable)**: The `execution` table is a TimescaleDB hypertable partitioned on `created` (1-day chunks). Executions are updated **~4 times** during their lifecycle (requested → scheduled → running → completed/failed), completing within at most ~1 day — well before the 7-day compression window. The `updated` column and its BEFORE UPDATE trigger are preserved (used by timeout monitor and UI). The `started_at` column (nullable `TIMESTAMPTZ`) records when the worker picked up the execution (status → `running`); it is `NULL` until then. **Duration** in the UI is computed as `updated - started_at` (not `updated - created`) so that queue/scheduling wait time is excluded. Compression is segmented by `action_ref` (after 7 days) and retention is 90 days. The `execution_volume_hourly` continuous aggregate queries the execution hypertable directly. The `execution_history` hypertable (field-level diffs) and its continuous aggregates (`execution_status_hourly`, `execution_throughput_hourly`) are preserved alongside — they serve complementary purposes (change tracking vs. volume monitoring). The nullable `timeout_seconds` column is snapshotted at creation and **immutable** afterward, so it is intentionally excluded from both `UpdateExecutionInput` and the `execution_history` trigger.
- **Entity History Tracking (TimescaleDB)**: Append-only `<table>_history` hypertables track field-level changes to `execution` and `worker` tables. Populated by PostgreSQL `AFTER INSERT OR UPDATE OR DELETE` triggers — no Rust code changes needed for recording. Uses JSONB diff format (`old_values`/`new_values`) with a `changed_fields TEXT[]` column for efficient filtering. Worker heartbeat-only updates are excluded. There are **no `event_history` or `enforcement_history` tables** — events are immutable and enforcements have a single deterministic status transition, so both tables are hypertables themselves. See `docs/plans/timescaledb-entity-history.md` for full design. The execution history trigger tracks: `status`, `result`, `executor`, `workflow_task`, `env_vars`, `started_at`.
- **History Large-Field Guardrails**: The `execution` history trigger stores a compact **digest summary** instead of the full value for the `result` column (which can be arbitrarily large). The digest is produced by the `_jsonb_digest_summary(JSONB)` helper function and has the shape `{"digest": "md5:<hex>", "size": <bytes>, "type": "<jsonb_typeof>"}`. This preserves change-detection semantics while avoiding history table bloat. The full result is always available on the live `execution` row. When adding new large JSONB columns to history triggers, use `_jsonb_digest_summary()` instead of storing the raw value.
- **Nullable FK Fields**: `rule.action` and `rule.trigger` are nullable (`Option<Id>` in Rust) — a rule with NULL action/trigger is non-functional but preserved for traceability. `execution.action`, `execution.parent`, `execution.enforcement`, `execution.started_at`, and `event.source` are also nullable. `enforcement.event` is nullable but has no FK constraint (event is a hypertable). `execution.enforcement` is nullable but has no FK constraint (enforcement is a hypertable). All FK columns on the execution table (`action`, `parent`, `original_execution`, `enforcement`, `executor`, `workflow_def`) have no FK constraints (execution is a hypertable). `inquiry.execution` and `workflow_execution.execution` also have no FK constraints. `enforcement.resolved_at` is nullable — `None` while status is `created`, set when resolved. `execution.started_at` is nullable — `None` until the worker sets status to `running`.
**Table Count**: 44 tables total in the schema (including `runtime_version`, `artifact_version`, `integration_token`, `sensor_process`, `work_queue`, `work_queue_item`, `work_queue_dispatch`, `dashboard`, `dashboard_version`, `pack_registry_index`, runtime retention config tables, `supervisor_run`, 3 `*_history` hypertables, and the `event`, `enforcement`, + `execution` hypertables)
**Migration Count**: 12 timestamped core migrations — see `migrations/` directory
- **Artifact System**: The `artifact` table stores metadata + structured data (progress entries via JSONB `data` column). The `artifact_version` table stores immutable content snapshots — either on disk (via `file_path` column) or in DB (via `content` BYTEA / `content_json` JSONB). Version numbering is auto-assigned via `next_artifact_version()` SQL function. A DB trigger (`enforce_artifact_retention`) auto-deletes oldest versions when count exceeds the artifact's `retention_limit`. `artifact_version.execution` is a plain BIGINT (no FK — execution is a hypertable) that links each version to the execution that produced it; execution-scoped artifact list APIs query through this per-version association. Progress-type artifacts use `artifact.data` (atomic JSON array append); file-type artifacts use `artifact_version` rows with `file_path` set. Binary content is excluded from default queries for performance (`SELECT_COLUMNS` vs `SELECT_COLUMNS_WITH_CONTENT`). **Visibility and classification**: Each artifact has `visibility` (`artifact_visibility_enum`: `public` or `private`, DB default `private`) plus `classification` (`artifact_classification_enum`: `general` or `runtime_log`, DB default `general`). Runtime-log classification is applied to worker-created execution stdout/stderr artifacts and sensor stdout/stderr artifacts so they remain the private raw source of truth while downstream systems consume only metadata signals. The `CreateArtifactRequest` DTO still accepts `visibility` as `Option<ArtifactVisibility>` — when omitted the API route handler applies a type-aware default, but runtime-log refs are always forced private. The search/list API can filter by both `visibility` and `classification`. Execution tokens get implicit access to artifacts owned by their executing pack and are subject to a cross-pack write guard. **Notifications**: `artifact_created` and `artifact_updated` DB triggers fire PostgreSQL NOTIFY with entity_type `artifact` and include both `visibility` and `classification` in the payload. The `artifact_updated` trigger extracts a progress summary (`progress_percent`, `progress_message`, `progress_entries`) from the last entry of the `data` JSONB array for progress-type artifacts; `artifact_version` insert/finalization notifications also emit `artifact_updated` payloads with the producing `execution` id so the web UI can invalidate execution-specific artifact caches for stdout/stderr logs.
- **File-Based Artifact Storage**: File-type artifacts (FileBinary, FileDataTable, FileImage, FileText) use a shared filesystem volume instead of PostgreSQL BYTEA. The `artifact_version.file_path` column stores the relative path from the `artifacts_dir` root (e.g., `mypack/build_log/v1.txt`). Pattern: `{ref_with_dots_as_dirs}/v{version}.{ext}`. The artifact ref (globally unique) is used as the directory key — no execution ID in the path, so artifacts can outlive executions and be shared across them. **Endpoint**: `POST /api/v1/artifacts/{id}/versions/file` allocates a version number and file path without any file content; the execution process writes the file to `$ATTUNE_ARTIFACTS_DIR/{file_path}`. Execution-token calls that omit `execution` are automatically stamped with the token's execution id so the worker can finalize the files. **Download**: `GET /api/v1/artifacts/{id}/download` and version-specific downloads check `file_path` first (read from disk), fall back to DB BYTEA/JSON. **Finalization**: After execution exits, the worker stats all file-backed versions for that execution and updates `size_bytes` on both `artifact_version` and parent `artifact` rows via direct DB access. In standalone/API-transport mode, if the execution wrote the file to the worker-local `$ATTUNE_ARTIFACTS_DIR`, finalization uploads it to the API-accessible artifact transport before updating size metadata. Execution stderr log artifacts are lazy: the worker writes stderr to an internal pending path and creates/promotes the `artifact` + `artifact_version` rows to the final file path as soon as stderr bytes are observed, so clients can stream real stderr logs while executions run but executions with no stderr output create no stderr artifact rows. **Cleanup**: Delete endpoints remove disk files before deleting DB rows; empty parent directories are cleaned up. **Backward compatible**: Existing DB-stored artifacts (`file_path = NULL`) continue to work unchanged.
- **Artifact File Transport**: Abstracts HOW file content is transferred between workers/sensors and the API, enabling operation without shared Docker volumes. Defined in `crates/common/src/artifact_transport/`. Two implementations:
  - **`VolumeTransport`** (`volume.rs`): Direct filesystem I/O (shared volume, fast path). Used when the API and worker share a mounted `artifacts_dir`. It normalizes artifact directories to `0o2775` and files to `0o664` on write/append so API containers can read files created by root-running worker/sensor containers even when ownership differs.
  - **`ApiTransport`** (`api.rs`): HTTP-based file transfer via internal API endpoints. Used by remote workers/sensors without shared volumes. Uses `reqwest::Client` with `TokenType::Worker` JWT auth. `ApiBufferedWriter` batches writes with 4KB flush threshold.
  - **Auto-detection** (`detection.rs`): At startup, the API writes a sentinel file `.attune-api-sentinel` (JSON with `api_url`, `instance_id`, `timestamp`) to `artifacts_dir`. Workers/sensors check for this file — present = shared volume (`VolumeTransport`), missing = remote (`ApiTransport`). Config override: `artifacts.transport: auto | volume | api` (default: `auto`).
  - **Internal file endpoints** (`crates/api/src/routes/internal_files.rs`): `PUT/GET/PATCH/HEAD/DELETE /api/v1/internal/files/{*file_path}` — raw file content transfer for workers using `ApiTransport`. Authenticated via `RequireAuth` (accepts `TokenType::Worker`).
  - **Worker integration**: `ActionExecutor` carries `Arc<dyn ArtifactFileTransport>`. File operations (artifact finalization, stderr log promotion, directory creation) use the transport instead of direct `tokio::fs` calls. In API-transport mode, locally staged files under `$ATTUNE_ARTIFACTS_DIR` are copied to the API transport at execution finalization so downstream tasks and downloads can read them from the API volume.
  - **Worker token**: `TokenType::Worker` JWT (24h TTL) generated at worker startup for internal file operations. Accepted by `RequireAuth` middleware. Not accepted by notifier WebSocket or execution-specific endpoints.
- **Sensor Per-Process Rotating Logs**: Each sensor gets dedicated stdout/stderr log artifacts managed by `RotatingLogWriter` (`crates/sensor/src/sensor_log.rs`) through the configured `ArtifactFileTransport`, so shared-volume sensors write directly and standalone sensors append through the API transport. Artifact-backed sensor logs are the authoritative record; the corresponding artifact rows are explicitly classified as `runtime_log` and forced private. Per-line stdout/stderr mirroring into tracing is disabled by default to avoid duplicate ingestion, while metadata-only lifecycle events (allocation/finalization/write failure/stream closure) are still logged through tracing. Sensor processes receive `ATTUNE_ARTIFACTS_DIR`; in API-transport mode, sensor-owned file-backed artifact versions that were staged locally are copied to the API transport when the sensor process stops or exits. Sensor log artifacts are auto-registered in DB with refs `sensor.{sensor_ref}.stdout` / `sensor.{sensor_ref}.stderr` (scope: Sensor, type: FileText); each active segment is an `artifact_version.file_path`, and size-based rotation allocates a new artifact version rather than only renaming local files. Sensor log artifact retention defaults to `versions` / `4` unless the sensor row sets `log_retention_policy` and/or `log_retention_limit`; for version-count retention, stale DB-pruned log segment files are removed through the transport. Action rows support the same nullable log retention override fields for execution stdout/stderr artifact versions, with service defaults controlled by `worker.execution_log_retention_policy` / `worker.execution_log_retention_limit` (`days` / `7`). API endpoints: `GET /api/v1/sensors/{sensor_ref}/logs` (list streams) and `GET /api/v1/sensors/{sensor_ref}/logs/{stream}` (download retained segments, with legacy raw-path fallback).
- **Pack Component Loading Order**: Runtimes → Triggers → Actions (+ workflow definitions) → Work Queues → Policies → Rules → Sensors (dependency order). Queue definitions live in `queues/*.yaml`, load after actions so `dispatch_action` refs can be resolved, and use `is_adhoc = false` cleanup semantics during pack reload/delete. Policy definitions live in `policies/*.yaml`, load after actions/queues so action/pack scopes can be resolved, and support `ref`, `name`/`label`, `description`, `enabled`, `priority`, `tags`, inferred scope from `action_ref` / `pack_ref` / global fallback, `concurrency`, `rate_limit`, and supported `quotas`. Rule definitions live in `rules/*.yaml`, load after triggers/actions so `trigger_ref` and `action_ref` can be resolved, are stored as non-ad-hoc pack-owned rules (`is_adhoc = false`, `owner_identity = NULL`), and are removed during pack reload when deleted from the pack while preserving API/UI-created ad-hoc rules. Both `PackComponentLoader` (Rust) and `load_core_pack.py` (Python) should follow this order. When an action YAML contains a `workflow_file` field, the loader creates/updates the referenced `workflow_definition` record and links it to the action during the Actions phase.

### Workflow Execution Orchestration
- **Detection**: The `ExecutionScheduler` checks `action.workflow_def.is_some()` before dispatching to a worker. Workflow actions are orchestrated by the executor, not sent to workers.
- **Work Queue Dispatching**: The executor owns business queue dispatch via `crates/executor/src/queue_dispatcher.rs`. A dedicated polling task scans enabled `work_queue` rows, resolves dispatch tunables (`concurrency`, `batch_size`) from literal values / pack config / keystore, honors optional `config.dispatch.inter_execution_delay_seconds` cooldowns for sequential queues (`concurrency = 1`) using the latest terminal `work_queue_dispatch.updated` timestamp, leases ready `work_queue_item` rows in `priority DESC, created ASC, id ASC` order, creates normal `execution` records, inserts `work_queue_dispatch` lineage rows, publishes standard `ExecutionRequested` messages, and marks dispatch rows `dispatched` after publish. Leased-but-unpublished dispatch rows are republished on the next poll. Queue input shaping is defined by the queue's top-level `action_params` JSONB column, not `config.input_mapping`: the dispatcher renders `action_params` through `WorkflowContext.render_json()` so queue templates can use workflow-style type-preserving expressions like `{{ item }}` (single dispatch payload), `{{ items }}` (batch payload array), `{{ queue_item }}` / `{{ queue_items }}` (metadata-rich queue item records including queue item ids, payloads, metadata, and provenance), `{{ queue }}` (queue metadata such as ref, batch mode, leased item count, and ack-contract version — **not the leased items themselves**), and `{{ config.* }}` (pack config). Work queues can also declare a top-level `item_schema` JSONB column using the same flat schema format as trigger `param_schema`; the API validates queue item payloads against that schema on enqueue and mutable pending-item updates. The `enabled` flag controls executor processing, while `accepting_new_items` separately controls whether enqueue requests are accepted. If `action_params` is empty, the dispatcher falls back to the default payload contract (`{...payload}` for single-object payloads, `{item: ...}` for single scalar payloads, `{items: [...]}` for batch dispatches). On completion, `CompletionListener::handle_queue_dispatch_completion()` validates `execution.result.queue_ack`, applies per-item terminal/retry outcomes, caps any retry transition using `config.dispatch.retry_limit` (default `0`), stores `ack_summary` / `last_error`, and updates the `work_queue_dispatch` row accordingly.
- **Orchestration Flow**: Scheduler loads the `WorkflowDefinition`, builds a `TaskGraph`, creates a `workflow_execution` record, marks the parent execution as Running, builds an initial `WorkflowContext` from execution parameters and workflow vars, then dispatches entry-point tasks as child executions via MQ with rendered inputs.
- **Template Resolution**: Task inputs are rendered through `WorkflowContext.render_json()` before dispatching. Uses the expression engine for full operator/function support inside `{{ }}`. Canonical namespaces: `parameters`, `workflow` (mutable vars), `task` (results), `config` (pack config), `keystore` (secrets), `item`, `index`, `system`. Backward-compat aliases: `vars`/`variables` → `workflow`, `tasks` → `task`, bare names → `workflow` fallback. **Type-preserving**: pure template expressions like `"{{ item }}"` preserve the JSON type (integer `5` stays as `5`, not string `"5"`). Mixed expressions like `"Sleeping for {{ item }} seconds"` remain strings.
- **Function Expressions**: `{{ result() }}` returns the last completed task's result. `{{ result().field.subfield }}` navigates into it. `{{ succeeded() }}`, `{{ failed() }}`, `{{ timed_out() }}` return booleans. These are evaluated by `WorkflowContext.try_evaluate_function_call()`.
- **Publish Directives**: Transition `publish` directives are evaluated when a transition fires. Published variables are persisted to the `workflow_execution.variables` column and available to subsequent tasks via the `workflow` namespace (e.g., `{{ workflow.number_list }}`). Values can be **any JSON-compatible type**: string templates (e.g., `number_list: "{{ result().data.items }}"`), booleans (`validation_passed: true`), numbers (`count: 42`), arrays, objects, or null. The `PublishDirective::Simple` variant stores `HashMap<String, serde_json::Value>`. String values are template-rendered with type preservation (pure `{{ }}` expressions preserve the underlying JSON type); non-string values (booleans, numbers, null) pass through `render_json` unchanged — `true` stays as boolean `true`, not string `"true"`. The `PublishVar` struct in `graph.rs` uses a `value: JsonValue` field (with `#[serde(alias = "expression")]` for backward compat with stored task graphs).
- **Child Task Dispatch**: Each workflow task becomes a child execution with the task's actual action ref (e.g., `core.echo`), `workflow_task` metadata linking it to the `workflow_execution` record, and a parent reference to the workflow execution. Child executions re-enter the normal scheduling pipeline, so nested workflows work recursively.
- **Workflow Task Retry**: Task-level `retry` config is enforced by the executor before normal workflow advancement. Failed or timed-out workflow child executions with retry attempts remaining create a new child execution for the same task/action/config, preserve parent workflow lineage, set `execution.original_execution`, increment `workflow_task.retry_count`, and publish the retry after the configured delay/backoff (`constant`, `linear`, or `exponential`, capped by `max_delay`). The workflow advances only after a successful attempt or retry exhaustion.
- **Workflow-Pausing Inquiry Action**: The core pack includes `core.ask`, a native workflow action intercepted by the scheduler. Dispatching `core.ask` creates an inquiry for the child execution, marks that child `running`, and does not send it to a worker. `POST /api/v1/inquiries/{id}/respond` publishes `InquiryResponded`; the executor marks the waiting child `completed` with result shape `{"response": ...}` and publishes `ExecutionCompleted` so workflow transitions can evaluate `result().response.*`. If the inquiry reaches `timeout`, the executor marks the child `timeout`, sets `workflow_task.timed_out = true`, and publishes completion so `timed_out()` transitions can fire.
- **with_items Expansion**: Tasks declaring `with_items: "{{ expr }}"` are expanded into child executions. The expression is resolved via the `WorkflowContext` to produce a JSON array, then each item gets its own child execution with `item`/`index` set on the context and `task_index` in `WorkflowTaskMetadata`. Completion tracking waits for ALL sibling items to finish before marking the task as completed/failed and advancing the workflow.
- **with_items Concurrency Limiting**: ALL child execution records are created in the database up front (with fully-rendered inputs), but only the first `N` are published to the message queue where `N` is the task's `concurrency` value (**default: 1**, i.e. serial execution). The remaining children stay at `Requested` status in the DB. As each item completes, `advance_workflow` counts in-flight siblings (`scheduling`/`scheduled`/`running`), calculates free slots (`concurrency - in_flight`), and calls `publish_pending_with_items_children()` which queries for `Requested`-status siblings ordered by `task_index` and publishes them. The DB `status = 'requested'` query is the authoritative source of undispatched items — no auxiliary state in workflow variables needed. The task is only marked complete when all siblings reach a terminal state. To run all items in parallel, explicitly set `concurrency` to the list length or a suitably large number.
- **Advancement**: The `CompletionListener` detects when a completed execution has `workflow_task` metadata and calls `ExecutionScheduler::advance_workflow()`. The scheduler rebuilds the `WorkflowContext` from persisted `workflow_execution.variables` plus all completed child execution results, sets `last_task_outcome`, evaluates transitions (succeeded/failed/always/timed_out/custom with context-based condition evaluation), processes publish directives, schedules successor tasks with rendered inputs, and completes the workflow when all tasks are done.
- **Transition Evaluation**: `succeeded()`, `failed()`, `timed_out()`, and `always` (no condition) are supported. Custom conditions are evaluated via `WorkflowContext.evaluate_condition()` with fallback to fire-on-success if evaluation fails.
- **Legacy Coordinator**: The prototype `WorkflowCoordinator` in `crates/executor/src/workflow/coordinator.rs` is bypassed — it has hardcoded schema prefixes and is not integrated with the MQ pipeline.

### Pack File Loading & Action Execution
- **Pack Base Directory**: Configured via `packs_base_dir` in config (defaults to `/opt/attune/packs`, development uses `./packs`)
- **Pack Volume Strategy**: Packs are mounted as volumes (NOT copied into Docker images)
  - Host `./packs/` → `packs_data` volume via `init-packs` service → mounted at `/opt/attune/packs` in all services
  - Development packs in `./packs.dev/` are bind-mounted directly for instant updates
- **Pack Binaries**: Native binaries (sensors) automatically built by the `init-pack-binaries` Docker Compose service (statically-linked musl, cross-arch via `PACK_BINARIES_RUST_TARGET`). Can also be built manually with `./scripts/build-pack-binaries.sh` or `make docker-build-pack-binaries`.
- **Pack Icons**: Pack authors may include `pack-icon.svg`, `pack-icon.png`, `pack-icon.jpg`/`.jpeg`, or `pack-icon.ico` at the pack root. The API serves the first matching file by priority (`svg`, `png`, `jpg`, `jpeg`, `ico`) from `GET /api/v1/packs/{ref}/icon`; the web UI uses it beside pack-owned metadata/runtime rows and workflow-builder nodes, falling back to a gear glyph when no icon is present.
- **Action Script Resolution**: Worker constructs file paths as `{packs_base_dir}/{pack_ref}/actions/{entrypoint}`
- **Execution-local agent MCP pattern**: AI/agent actions running inside a worker should prefer the execution-scoped `ATTUNE_API_TOKEN` when the action has been granted execution permission sets, and spawn the local `/opt/attune/agent/attune-mcp` binary over stdio. The worker intentionally omits `ATTUNE_API_TOKEN` for executions whose snapshotted `permission_set_refs` are empty. The core pack reference action `core.run_agent_command` exports `ATTUNE_MCP_COMMAND`, `ATTUNE_MCP_TRANSPORT=stdio`, and `ATTUNE_AGENT_STATE_DIR` for this purpose.
- **Workflow Action YAML (`workflow_file` field)**: An action YAML may include a `workflow_file` field (e.g., `workflow_file: workflows/timeline_demo.yaml`) pointing to a workflow definition file relative to the `actions/` directory. When present, the `PackComponentLoader` reads and parses the referenced workflow YAML, creates/updates a `workflow_definition` record, and links the action to it via `action.workflow_def`. This separates action-level metadata (ref, label, parameters, policies) from the workflow graph (tasks, transitions, variables), and allows **multiple actions to reference the same workflow file** with different parameter schemas or policy configurations. Workflow actions have no `runner_type` (runtime is `None`) — the executor orchestrates child task executions rather than sending to a worker.
- **Action-level worker capability requirements**: Actions may declare `required_worker_runtimes` in YAML / API / DB as a runtime-name/alias -> constraint map for additional worker runtimes that must be present **in addition to** the action's own `runner_type` runtime. Use `*` for "any available version" (for example `{ node: ">=20", python: "*" }`). This is for cases like a shell action that still needs a Node-capable worker so it can use pack-installed Node.js tools. The scheduler normalizes aliases (`node`, `nodejs`, `node.js`) via `normalize_runtime_name()` and filters workers against `worker.capabilities.runtimes`, applying semver checks for any entries whose constraint is not `*`. Pack environment setup also treats these required runtimes as pack environment inputs, so a shell action with `required_worker_runtimes: { node: ">=20" }` can still trigger creation of the pack's matching Node.js environment.
- **Action-level worker placement constraints**: Actions may declare Kubernetes-style placement fields in YAML / API / DB. `worker_selector` is an exact worker label map (all labels must match `worker.capabilities.labels`); `worker_tolerations` allows scheduling onto workers with matching `worker.capabilities.taints`; `worker_affinity` supports `required`, `preferred`, and `anti_affinity` selector terms over worker labels. Worker config exposes `worker.labels` and `worker.taints`; registration stores them under the `labels` and `taints` capability keys. Manual execution requests and workflow tasks may also provide the same three placement fields as per-execution overrides. Execution override columns are nullable JSONB: `NULL` inherits the action default for that field, while explicit `{}` / `[]` clears that field for the execution. The executor enforces selector, taint/toleration, required affinity, and anti-affinity as hard filters after runtime compatibility and before status/heartbeat checks, then scores eligible workers by preferred affinity and round-robins among the highest-scoring workers.
- **Sensor-level worker placement constraints**: Pack sensor YAML may declare `worker_selector`, `worker_tolerations`, and `worker_affinity` with the same shape as actions. Sensor worker config exposes `sensor.labels` and `sensor.taints`; registration stores them under the same worker capability keys. `SensorManager::sensor_matches_this_worker()` enforces placement before starting or restarting a sensor process.
- **Action-linked workflow files omit action-level metadata**: Workflow files referenced via `workflow_file` should contain **only the execution graph**: `version`, `vars`, `tasks`, `output_map`. The `ref`, `label`, `description`, `parameters`, `output`, and `tags` fields are omitted — the action YAML is the single authoritative source for those values. The `WorkflowDefinition` parser accepts empty `ref`/`label` (defaults to `""`), and the loader / registrar fall back to the action YAML (or filename-derived values) when they are missing. Standalone workflow files (in `workflows/`) still carry their own `ref`/`label` since they have no companion action YAML.
- **Workflow File Storage**: The visual workflow builder save endpoints (`POST /api/v1/packs/{pack_ref}/workflow-files` and `PUT /api/v1/workflows/{ref}/file`) write **two files** per workflow:
  1. **Action YAML** at `{packs_base_dir}/{pack_ref}/actions/{name}.yaml` — action-level metadata (`ref`, `label`, `description`, `parameters`, `output`, `tags`, `workflow_file` reference, `enabled`). Built by `build_action_yaml()` in `crates/api/src/routes/workflows.rs`.
  2. **Workflow YAML** at `{packs_base_dir}/{pack_ref}/actions/workflows/{name}.workflow.yaml` — graph-only (`version`, `vars`, `tasks`, `output_map`). The `strip_action_level_fields()` function removes `ref`, `label`, `description`, `parameters`, `output`, and `tags` from the definition before writing.
  Pack-bundled workflows use the same directory layout and are discovered during pack registration when their companion action YAML contains `workflow_file`.
- **Workflow File Discovery (dual-directory scanning)**: The `WorkflowLoader` scans **two** directories when loading workflows for a pack: (1) `{pack_dir}/workflows/` (legacy standalone workflow files), and (2) `{pack_dir}/actions/workflows/` (visual-builder and action-linked workflow files). Files with `.workflow.yaml` suffix have the `.workflow` portion stripped when deriving the workflow name/ref (e.g., `deploy.workflow.yaml` → name `deploy`, ref `pack.deploy`). If the same ref appears in both directories, `actions/workflows/` wins. The `reload_workflow` method searches `actions/workflows/` first, trying `.workflow.yaml`, `.yaml`, `.workflow.yml`, and `.yml` extensions.
- **Task Model (Orquesta-aligned)**: Tasks are purely action invocations — there is no task `type` field or task-level `when` condition in the UI model. Parallelism is implicit (multiple `do` targets in a transition fan out into parallel branches). Conditions belong exclusively on transitions (`next[].when`). Each task has: `name`, `action`, `input`, `permission_set_refs`, `next` (transitions), `delay`, `retry`, `timeout`, `with_items`, `batch_size`, `concurrency`, `join`.
  - **Task execution permission sets**: Workflow tasks may declare `permission_set_refs` (or singular alias `permission_set_ref`) to override the task action's default execution permission sets for that child execution. The field is rendered through `WorkflowContext.render_json()` like task input, so it may be a literal string ref, an array of string refs, or a template resolving to either shape (for example `permission_set_refs: "{{ workflow.agent_permission_sets }}"`). Use the reserved ref `standard` to grant the child execution action/pack-scoped key and artifact access. In workflow task executions, `standard` includes both the task action's action/pack scope and the containing workflow action's action/pack scope, enabling a workflow in one pack to pass its pack-scoped credentials/artifacts to task actions in another pack through the API. In `with_items` tasks, rendering runs per item with `item` and `index` available. Omit the field to use the action's `default_execution_permission_set_refs`; set it to an empty string/null/empty array to force no execution token permissions for that child.
  - **Task worker placement overrides**: Workflow tasks may declare `worker_selector`, `worker_tolerations`, and/or `worker_affinity` to override the task action's placement defaults for the child execution. These fields are rendered through `WorkflowContext.render_json()` like task input, so they may be literal JSON/YAML or templates resolving to the expected object/array shape. In `with_items` tasks, rendering runs per item with `item` and `index` available. Omit a field to inherit the action default for that field; set it to `{}` (`worker_selector` / `worker_affinity`) or `[]` (`worker_tolerations`) to explicitly clear it.
  - The backend `Task` struct (`crates/common/src/workflow/parser.rs`) still supports `type` and task-level `when` for backward compatibility, but the UI never sets them.
- **Task Transition Model (Orquesta-style)**: Tasks use an ordered `next` array of transitions instead of flat `on_success`/`on_failure`/`on_complete`/`on_timeout` fields. Each transition has:
  - `when` — condition expression (e.g., `{{ succeeded() }}`, `{{ failed() }}`, `{{ timed_out() }}`, or custom). Omit for unconditional.
  - `publish` — key-value pairs to publish into the workflow context (e.g., `- result: "{{ result() }}"`)
  - `do` — list of next task names to invoke when the condition is met
  - `label` — optional custom display label (overrides auto-derived label from `when` expression)
  - `color` — optional custom CSS color for the transition edge (e.g., `"#ff6600"`)
  - `edge_waypoints` — optional `Record<string, NodePosition[]>` of intermediate routing points per target task name (chart-only, stored in `__chart_meta__`)
  - `label_positions` — optional `Record<string, NodePosition>` of custom label positions per target task name (chart-only, stored in `__chart_meta__`)
  - **Example YAML**:
    ```
    next:
      - when: "{{ succeeded() }}"
        label: "main path"
        color: "#22c55e"
        publish:
          - msg: "task done"
        do:
          - log
          - next_task
      - when: "{{ failed() }}"
        do:
          - error_handler
    ```
  - **Legacy format support**: The parser (`crates/common/src/workflow/parser.rs`) auto-converts legacy `on_success`/`on_failure`/`on_complete`/`on_timeout`/`decision` fields into `next` transitions during parsing. The canonical internal representation always uses `next`.
  - **Frontend types**: `TaskTransition` in `web/src/types/workflow.ts` (includes `edge_waypoints`, `label_positions` for visual routing); `TransitionPreset` ("succeeded" | "failed" | "always") for quick-access drag handles; `WorkflowEdge` includes per-edge `waypoints` and `labelPosition` derived from the transition; `SelectedEdgeInfo` and `EdgeHoverInfo` (includes `targetTaskId`) in `WorkflowEdges.tsx`
  - **Backend types**: `TaskTransition` in `crates/common/src/workflow/parser.rs`; `GraphTransition` in `crates/executor/src/workflow/graph.rs`
  - **NOT this** (legacy format): `on_success: task2` / `on_failure: error_handler` — still parsed for backward compat but normalized to `next`
- **Runtime YAML Loading**: Pack registration reads `runtimes/*.yaml` files and inserts them into the `runtime` table. Runtime refs use format `{pack_ref}.{name}` (e.g., `core.python`, `core.shell`). If the YAML includes a `versions` array, each entry is inserted into the `runtime_version` table with its own `execution_config`, `distributions`, and optional `is_default` flag.
- **Runtime Version Constraints**: Actions and sensors can declare `runtime_version: ">=3.12"` (or any semver constraint like `~3.12`, `^3.12`, `>=3.12,<4.0`) in their YAML. This is stored in the `runtime_version_constraint` column. At execution time the worker can select the highest available version satisfying the constraint. A bare version like `"3.12"` is treated as tilde (`~3.12` → >=3.12.0, <3.13.0).
- **Version Matching Module**: `crates/common/src/version_matching.rs` provides `parse_version()` (lenient semver parsing), `parse_constraint()`, `matches_constraint()`, `select_best_version()`, and `extract_version_components()`. Uses the `semver` crate internally.
- **Runtime Version Table**: `runtime_version` stores version-specific execution configs per runtime. Each row has: `runtime` (FK), `version` (string), `version_major/minor/patch` (ints for range queries), `execution_config` (complete, not a diff), `distributions` (verification metadata), `is_default`, `available`, `verified_at`, `meta`. Unique on `(runtime, version)`.
- **Runtime Selection**: Determined by action's runtime field (e.g., "Shell", "Python") - compared case-insensitively; when an explicit `runtime_name` is set in execution context, it is authoritative (no fallback to extension matching). When the action also declares a `runtime_version_constraint`, the executor queries `runtime_version` rows, calls `select_best_version()`, and passes the selected version's `execution_config` as an override through `ExecutionContext.runtime_config_override`. The `ProcessRuntime` uses this override instead of its built-in config.
- **Worker Runtime Loading**: Worker loads all runtimes from DB that have a non-empty `execution_config` (i.e., runtimes with an interpreter configured). Native runtimes (e.g., `core.native` with empty config) are automatically skipped since they execute binaries directly.
- **Worker Startup Sequence**: (1) Connect to DB and MQ, (2) Load runtimes from DB → create `ProcessRuntime` instances, (3) Register worker and set up MQ infrastructure (including configured `worker.labels` / `worker.taints` in capabilities), (4) **Verify runtime versions** — run verification commands from `distributions` JSONB for each `RuntimeVersion` row and update `available` flag (`crates/worker/src/version_verify.rs`), (5) **Set up runtime environments** — create per-version environments for packs, (6) Start heartbeat, execution consumer, and pack registration consumer.
- **Agent Startup Sequence** (`attune-agent`): (0) **Auto-detect runtimes** — probes the container for interpreter binaries using `runtime_detect::detect_runtimes()`, sets `ATTUNE_WORKER_RUNTIMES` env var with discovered names, (0b) **Dynamic runtime registration** — calls `auto_register_detected_runtimes()` to ensure each detected runtime has a DB entry (from template or minimal), then (1–6) follows the same startup sequence as `attune-worker`. If `ATTUNE_WORKER_RUNTIMES` is already set, auto-detection is skipped (explicit override). The `--detect-only` flag runs detection, prints a report, and exits without starting the worker.
- **Agent Runtime Auto-Detection** (`crates/worker/src/runtime_detect.rs`): Database-free runtime discovery for the agent. Probes 8 interpreter families in order: shell (`bash`/`sh`), python (`python3`/`python`), node (`node`/`nodejs`), ruby, go, java, r (`Rscript`), perl. Uses `which`-style PATH lookup with fallbacks for absolute paths (`/bin/bash`, `/bin/sh`) and `command -v`. Captures version strings via interpreter-specific version commands. Returns `Vec<DetectedRuntime>` with name, path, and optional version. The `format_as_env_value()` helper converts to comma-separated format for `ATTUNE_WORKER_RUNTIMES`.
- **Dynamic Runtime Registration** (`crates/worker/src/dynamic_runtime.rs`): When the agent detects a runtime that has no corresponding entry in the database, `auto_register_detected_runtimes()` auto-registers it before `WorkerService::new()`. Strategy: (1) look up by normalized name — if found, skip; (2) look for a template runtime in loaded packs (e.g., `core.ruby`) — if found, clone with `auto_detected = true` and the detected binary path substituted into the execution config; (3) if no template, create a minimal runtime with just the interpreter binary and file extension. Auto-registered runtimes use ref format `auto.<name>` (e.g., `auto.ruby`). The `Runtime` model has `auto_detected: bool` and `detection_config: JsonDict` columns (migration `000002`). The `detection_config` JSONB stores `detected_path`, `detected_name`, and optional `detected_version`.
- **Runtime Name Normalization**: The `ATTUNE_WORKER_RUNTIMES` filter (e.g., `shell,node`) uses alias-aware matching via `normalize_runtime_name()` in `crates/common/src/runtime_detection.rs`. This ensures that filter value `"node"` matches DB runtime name `"Node.js"` (lowercased to `"node.js"`). Alias groups: `node`/`nodejs`/`node.js` → `node`, `python`/`python3` → `python`, `shell`/`bash`/`sh` → `shell`, `native`/`builtin`/`standalone` → `native`, `ruby`/`rb` → `ruby`, `go`/`golang` → `go`, `java`/`jdk`/`openjdk` → `java`, `perl`/`perl5` → `perl`, `r`/`rscript` → `r`. Used in worker service runtime loading and environment setup.
- **Runtime Execution Environment Variables**: `RuntimeExecutionConfig.env_vars` (HashMap<String, String>) specifies template-based environment variables injected during action execution. Example: `{"NODE_PATH": "{env_dir}/node_modules"}` ensures Node.js finds packages in the isolated environment. Template variables (`{env_dir}`, `{pack_dir}`, `{interpreter}`, `{manifest_path}`) are resolved at execution time by `ProcessRuntime::execute`.
- **Native Runtime Detection**: Runtime detection is purely data-driven via `execution_config` in the runtime table. A runtime with empty `execution_config` (or empty `interpreter.binary`) is native — the entrypoint is executed directly without an interpreter. There is no special "builtin" runtime concept.
- **Sensor Runtime Assignment**: Sensors declare their `runner_type` in YAML (e.g., `python`, `native`). The pack loader resolves this to the correct runtime from the database. Default is `native` (compiled binary, no interpreter). Legacy values `standalone` and `builtin` map to `core.native`.
- **Runtime Environment Setup**: Worker creates isolated environments (virtualenvs, node_modules) proactively at startup and via `pack.registered` MQ events at `{runtime_envs_dir}/{pack_ref}/{runtime_name}`; setup is idempotent. Environment `create_command` and dependency `install_command` templates MUST use `{env_dir}` (not `{pack_dir}`) since pack directories are mounted read-only in Docker. For Node.js, `create_command` copies `package.json` to `{env_dir}` and `install_command` uses `npm install --prefix {env_dir}`.
- **Per-Version Environment Isolation**: When runtime versions are registered, the worker creates per-version environments at `{runtime_envs_dir}/{pack_ref}/{runtime_name}-{version}` (e.g., `python-3.12`). This ensures different versions maintain isolated environments with their own interpreter binaries and installed dependencies. A base (unversioned) environment is also created for backward compatibility. The `ExecutionContext.runtime_env_dir_suffix` field controls which env dir the `ProcessRuntime` uses at execution time.
- **Runtime Version Verification**: At worker startup, `version_verify::verify_all_runtime_versions()` runs each version's verification commands (from `distributions.verification.commands` JSONB) and updates the `available` and `verified_at` columns in the database. Only versions marked `available = true` are considered by `select_best_version()`. Verification respects the `ATTUNE_WORKER_RUNTIMES` filter.
- **Schema Format (Unified)**: ALL schemas (`param_schema`, `out_schema`, `conf_schema`) use the same flat format with `required` and `secret` inlined per-parameter (NOT standard JSON Schema). Stored as JSONB columns.
  - **Example YAML**: `parameters:\n  url:\n    type: string\n    required: true\n  token:\n    type: string\n    secret: true`
  - **Stored JSON**: `{"url": {"type": "string", "required": true}, "token": {"type": "string", "secret": true}}`
  - **NOT this** (legacy JSON Schema): `{"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}`
  - **Web UI**: `extractProperties()` in `ParamSchemaForm.tsx` is the single extraction function for all schema types. Only handles flat format.
  - **SchemaBuilder**: Visual schema editor reads and writes flat format with `required` and `secret` checkboxes per parameter.
  - **Backend Validation**: `flat_to_json_schema()` in `crates/api/src/validation/params.rs` converts flat format to JSON Schema internally for `jsonschema` crate validation. This conversion is an implementation detail — external interfaces always use flat format.
- **Execution Config Format (Flat)**: The `execution.config` JSONB column always stores parameters in **flat format** — the object itself IS the parameters map (e.g., `{"url": "https://...", "method": "GET"}`). This is consistent across all execution sources: manual API calls, rule-triggered enforcements, and workflow task children. There is **no `{"parameters": {...}}` wrapper** — never nest parameters under a `"parameters"` key. The worker reads `config` as a flat object and passes each key-value pair as an action parameter. The scheduler's `extract_workflow_params()` helper treats the config object directly as the parameters map.
- **Parameter Delivery**: Actions receive parameters via stdin as JSON (never environment variables)
- **Output Format**: Actions declare output format (text/json/yaml) - json/yaml are parsed into execution.result JSONB
- **Execution API Permission Sets**: Execution-scoped API access is opt-in. Manual execution requests can provide `permission_set_refs`; when omitted, the API snapshots the action's `default_execution_permission_set_refs`. Rules, work queues, and workflow tasks can also provide `permission_set_refs` overrides for executions they create; omitted/NULL on rules/queues inherits the target action default, while an empty array forces no execution API token. Retries preserve the original execution refs. The reserved ref `standard` is always delegable because it is constrained to the executing action/pack (and containing workflow action/pack for workflow task children); all other refs must correspond to database permission sets the caller can delegate. The `/auth/me` response exposes `assigned_permission_set_refs` (direct + role-derived metadata refs) so the web client can limit manual execution overrides; identities with `core.admin` may select any permission set, while non-admin identities may select `standard` plus refs assigned to them. `GET /api/v1/actions?executable_with_current_access=true` filters actions to those the caller can execute with their configured default execution permission sets.
- **Standard Environment Variables**: Worker provides execution context via `ATTUNE_*` environment variables:
  - `ATTUNE_ACTION` - Action ref (always present)
  - `ATTUNE_PACK_REF` - Pack ref derived from the action ref (always present)
  - `ATTUNE_EXEC_ID` - Execution database ID (always present)
  - `ATTUNE_API_TOKEN` - Execution-scoped API token (only present when `execution.permission_set_refs` is non-empty). Carries the triggering identity's `sub` claim for attribution, plus token metadata containing the snapshotted token access refs. API authorization for execution tokens uses only those token-listed refs; it does not inherit the triggering identity's full effective RBAC. Named refs load database permission sets. The reserved ref `standard` expands to key/artifact grants constrained to the executing action/pack plus, for workflow child executions, the containing workflow action/pack. When refs are empty, the worker omits this variable entirely.
  - `ATTUNE_TRACE_TAG` - Optional execution trace tag snapshotted at execution creation and propagated to child/nested automated executions.
  - `ATTUNE_API_URL` - API base URL (always present)
  - `ATTUNE_ARTIFACTS_DIR` - Absolute path to shared artifact volume (always present, e.g., `/opt/attune/artifacts`)
  - `ATTUNE_RUNTIME_ENVS_DIR` - Absolute path to the shared runtime environment root (always present, e.g., `/opt/attune/runtime_envs`)
  - `ATTUNE_LOG_LEVEL` / `ATTUNE_LOG_FORMAT` - Managed sensor processes inherit the parent sensor service log directive/format (`json` in Docker/distributable configs, `pretty` in development unless overridden)
  - `ATTUNE_RULE` - Rule ref (if triggered by rule)
  - `ATTUNE_TRIGGER` - Trigger ref (if triggered by event/trigger)
- **Custom Environment Variables**: Optional, set via `execution.env_vars` JSONB field (for debug flags, runtime config only)

### API Service (`crates/api`)
- **Structure**: `routes/` (endpoints) + `dto/` (request/response) + `auth/` + `middleware/`
- **Responses**: Standardized `ApiResponse<T>` wrapper with `data` field
- **Protected Routes**: Apply `RequireAuth` middleware
- **OpenAPI**: Documented with `utoipa` attributes (`#[utoipa::path]`)
- **Error Handling**: Custom `ApiError` type with proper HTTP status codes
- **Trace search/report APIs**: Exact trace-tag filters are supported on execution/event/enforcement list endpoints (`trace_tag`), and `GET /api/v1/traces/{trace_tag}` returns a consolidated cross-system report (executions, enforcements, events, queue dispatches, queue items) for that trace.
- **Available at**: `http://localhost:8080` (dev), `/api-spec/openapi.json` for spec

### Common Library (`crates/common`)
- **Modules**: `models`, `repositories`, `db`, `config`, `error`, `mq`, `metadata_cache`, `crypto`, `utils`, `workflow` (includes `expression` sub-module), `pack_registry`, `template_resolver`, `version_matching`, `runtime_detection`, `scheduling`
- **Exports**: Commonly used types re-exported from `lib.rs`
- **Repository Layer**: All DB access goes through repositories in `repositories/`
- **Message Queue**: Abstractions in `mq/` for RabbitMQ communication
- **Template Resolver**: Resolves `{{ }}` template variables in rule `action_params` during enforcement creation. Re-exported from `attune_common::{TemplateContext, resolve_templates}`.

### Template Variable Syntax
Rule `action_params` support Jinja2-style `{{ source.path }}` templates resolved at enforcement creation time:

| Namespace | Example | Description |
|-----------|---------|-------------|
| `event.payload.*` | `{{ event.payload.service }}` | Event payload fields |
| `event.id` | `{{ event.id }}` | Event database ID |
| `event.trigger` | `{{ event.trigger }}` | Trigger ref that generated the event |
| `event.created` | `{{ event.created }}` | Event creation timestamp (RFC 3339) |
| `pack.config.*` | `{{ pack.config.api_token }}` | Pack configuration values |
| `system.*` | `{{ system.timestamp }}` | System variables (timestamp, rule info) |

- **Implementation**: `crates/common/src/template_resolver.rs` (also re-exported from `attune_sensor::template_resolver`)
- **Integration**: `crates/executor/src/event_processor.rs` calls `resolve_templates()` in `create_enforcement()`
- **IMPORTANT**: The old `trigger.payload.*` syntax was renamed to `event.payload.*` — the payload data comes from the Event, not the Trigger

### Workflow Expression Engine
Workflow templates (`{{ expr }}`) support a full expression language for evaluating conditions, computing values, and transforming data. The engine is in `crates/common/src/workflow/expression/` (tokenizer → parser → evaluator) and is integrated into `WorkflowContext` via the `EvalContext` trait.

**Canonical Namespaces** — all data inside `{{ }}` expressions is organised into well-defined, non-overlapping namespaces:

| Namespace | Example | Description |
|-----------|---------|-------------|
| `parameters` | `{{ parameters.url }}` | Immutable workflow input parameters |
| `workflow` | `{{ workflow.counter }}` | Mutable workflow-scoped variables (set via `publish`) |
| `task` | `{{ task.fetch.result.data }}` | Completed task results keyed by task name |
| `config` | `{{ config.api_token }}` | Pack configuration values (read-only) |
| `keystore` | `{{ keystore.secret_key }}` | Encrypted secrets from the key store (read-only). Values are `JsonValue` — strings, objects, arrays, etc. Access nested fields with dot notation: `{{ keystore.db_credentials.password }}` |
| `item` | `{{ item }}` / `{{ item.name }}` | Current element in a `with_items` loop |
| `index` | `{{ index }}` | Zero-based iteration index in a `with_items` loop |
| `system` | `{{ system.workflow_start }}` | System-provided variables |

Backward-compatible aliases (kept for existing workflow definitions):
- `vars` / `variables` → same as `workflow`
- `tasks` → same as `task`
- Bare variable names (e.g. `{{ my_var }}`) resolve against the `workflow` variable store as a last-resort fallback.

**IMPORTANT**: New workflow definitions should always use the canonical namespace names. The `config` and `keystore` namespaces are populated by the scheduler from the pack's `config` JSONB column and decrypted `key` table entries (JSONB values) respectively. If not populated, they resolve to `null`. Keystore values preserve their JSON type — a key storing `{"host":"db.example.com","port":5432}` is accessible as `{{ keystore.db_config.host }}` and `{{ keystore.db_config.port }}` (the latter resolves to integer `5432`, not string `"5432"`).

**Operators** (lowest to highest precedence):
1. `or` — logical OR (short-circuit)
2. `and` — logical AND (short-circuit)
3. `not` — logical NOT (unary)
4. `==`, `!=`, `<`, `>`, `<=`, `>=`, `in` — comparison & membership
5. `+`, `-` — addition/subtraction (also string/array concatenation for `+`)
6. `*`, `/`, `%` — multiplication, division, modulo
7. Unary `-` — negation
8. `.field`, `[index]`, `(args)` — postfix access & function calls

**Type Rules**:
- **No implicit type coercion**: `"3" == 3` → `false`, `"hello" + 5` → error
- **Int/float cross-comparison allowed**: `3 == 3.0` → `true`
- **Integer preservation**: `2 + 3` → `5` (int), `2 + 1.5` → `3.5` (float), `10 / 4` → `2.5` (float), `10 / 5` → `2` (int)
- **Python-like truthiness**: `null`, `false`, `0`, `""`, `[]`, `{}` are falsy
- **Deep equality**: `==`/`!=` recursively compare objects and arrays
- **Negative indexing**: `arr[-1]` returns last element

**Built-in Functions**:
- Type conversion: `string(v)`, `number(v)`, `int(v)`, `bool(v)`
- Introspection: `type_of(v)`, `length(v)`, `keys(obj)`, `values(obj)`
- Math: `abs(n)`, `floor(n)`, `ceil(n)`, `round(n)`, `min(a,b)`, `max(a,b)`, `sum(arr)`
- String: `lower(s)`, `upper(s)`, `trim(s)`, `split(s, sep)`, `join(arr, sep)`, `replace(s, old, new)`, `starts_with(s, prefix)`, `ends_with(s, suffix)`, `match(pattern, s)` (regex)
- Collection: `contains(haystack, needle)`, `reversed(v)`, `sort(arr)`, `unique(arr)`, `flat(arr)`, `zip(a, b)`, `range(n)` / `range(start, end)`, `slice(v, start, end)`, `index_of(haystack, needle)`, `count(haystack, needle)`, `merge(obj_a, obj_b)`, `chunks(arr, size)`
- Workflow: `result()`, `succeeded()`, `failed()`, `timed_out()` (resolved via `EvalContext` trait)

**Usage in Conditions** (`when:` on transitions):
```
when: "succeeded() and result().code == 200"
when: "length(workflow.items) > 3 and \"admin\" in workflow.roles"
when: "not failed()"
when: "result().status == \"ok\" or result().status == \"accepted\""
when: "config.retries > 0"
```

**Usage in Templates** (`{{ expr }}`):
```
input:
  count: "{{ length(workflow.items) }}"
  greeting: "{{ parameters.first + \" \" + parameters.last }}"
  doubled: "{{ parameters.x * 2 }}"
  names: "{{ join(sort(keys(workflow.data)), \", \") }}"
  auth: "Bearer {{ keystore.api_key }}"
  endpoint: "{{ config.base_url + \"/api/v1\" }}"
  prev_output: "{{ task.fetch.result.data.id }}"
```

**Implementation Files**:
- `crates/common/src/workflow/expression/mod.rs` — module entry point, `eval_expression()`, `parse_expression()`
- `crates/common/src/workflow/expression/tokenizer.rs` — lexer
- `crates/common/src/workflow/expression/parser.rs` — recursive-descent parser
- `crates/common/src/workflow/expression/evaluator.rs` — AST evaluator, `EvalContext` trait, built-in functions
- `crates/common/src/workflow/expression/ast.rs` — AST node types (`Expr`, `BinaryOp`, `UnaryOp`)
- `crates/executor/src/workflow/context.rs` — `WorkflowContext` implements `EvalContext`

### Web UI (`web/`)
- **Generated Client**: OpenAPI client auto-generated from API spec
  - Run: `npm run generate:api` (requires API running on :8080)
  - Location: `src/api/`
- **State Management**: Zustand for global state, TanStack Query for server state
- **Styling**: Tailwind utility classes
- **Dev Server**: `npm run dev` (typically :3000 or :5173)
- **Build**: `npm run build`
- **Dashboard runtime page (`/`)**: spec-driven dashboard renderer that loads `GET /api/v1/dashboards/{ref}` and resolves card data via `POST /api/v1/dashboards/{ref}/data`, with typed contracts in `web/src/types/dashboard.ts`, API client helpers in `web/src/lib/dashboard-client.ts`, query hooks in `web/src/hooks/useDashboards.ts`, and deterministic card renderers/status handling in `web/src/components/dashboard/DashboardCard.tsx`.
- **Workflow Timeline DAG**: Prefect-style workflow run timeline visualization on the execution detail page for workflow executions
  - Components in `web/src/components/executions/workflow-timeline/` (WorkflowTimelineDAG, TimelineRenderer, types, data, layout)
  - Pure SVG renderer — no D3, no React Flow, no additional npm dependencies
  - Renders child task executions as horizontal duration bars on a time axis with curved Bezier dependency edges
  - **Data flow**: `WorkflowTimelineDAG` (orchestrator) fetches child executions via `useChildExecutions` + workflow definition via `useWorkflow(actionRef)` → `data.ts` transforms into `TimelineTask[]`/`TimelineEdge[]`/`TimelineMilestone[]` → `layout.ts` computes lane assignments + positions → `TimelineRenderer` renders SVG
  - **Edge coloring from workflow metadata**: Fetches the workflow definition's `next` transition array, classifies `when` expressions (`{{ succeeded() }}` → green, `{{ failed() }}` → red dashed, `{{ timed_out() }}` → orange dash-dot, unconditional → gray), and reads `__chart_meta__` custom labels/colors
  - **Task bars**: Colored by state (green=completed, blue=running with pulse animation, red=failed, gray=pending, orange=timeout). Left accent bar, text label with ellipsis clipping, timeout indicator badge.
  - **Milestones**: Synthetic start/end diamond nodes + merge/fork junctions when fan-in/fan-out exceeds 3 tasks
  - **Lane packing**: Greedy algorithm assigns tasks to non-overlapping y-lanes sorted by start time, with optional reordering to cluster tasks sharing upstream dependencies
  - **Interactions**: Hover tooltip (name, state, times, duration, retries, upstream/downstream counts), click-to-select with BFS path highlighting, double-click to navigate to child execution, horizontal zoom (mouse wheel anchored to cursor), alt+drag pan, expand/compact toggle
  - **Fallback**: When no workflow definition is available, infers dependency edges from task timing heuristics
  - **Integration**: Rendered in `ExecutionDetailPage.tsx` above `WorkflowTasksPanel`, conditioned on `isWorkflow`. Shares TanStack Query cache with WorkflowTasksPanel. Accepts `ParentExecutionInfo` interface (satisfied by both `ExecutionResponse` and `ExecutionSummary`).
- **Workflow Builder**: Visual node-based workflow editor at `/actions/workflows/new` and `/actions/workflows/:ref/edit`
  - Components in `web/src/components/workflows/` (ActionPalette, WorkflowCanvas, TaskNode, WorkflowEdges, TaskInspector)
  - Types and conversion utilities in `web/src/types/workflow.ts`
  - Hooks in `web/src/hooks/useWorkflows.ts`
  - Saves workflow files to `{packs_base_dir}/{pack_ref}/actions/workflows/{name}.workflow.yaml` via dedicated API endpoints
  - **Visual / Raw YAML toggle**: Toolbar has a segmented toggle to switch between the visual node-based builder and a two-panel read-only YAML preview (generated via `js-yaml`). Raw YAML mode replaces the canvas, palette, and inspector with side-by-side panels: **Action YAML** (left, blue — `actions/{name}.yaml`: ref, label, parameters, output, tags, `workflow_file` reference) and **Workflow YAML** (right, green — `actions/workflows/{name}.workflow.yaml`: version, vars, tasks, output_map — graph only). Each panel has its own copy button and a description bar explaining the file's role. The `builderStateToGraph()` function extracts the graph-only definition, and `builderStateToActionYaml()` extracts the action metadata.
  - **Drag-handle connections**: TaskNode has output handles (green=succeeded, red=failed, gray=always) and an input handle (top). Drag from an output handle to another node's input handle to create a transition.
  - **Transition customization**: Users can rename transitions (custom `label`) and assign custom colors (CSS color string or preset swatches) via the TaskInspector. Custom colors/labels are persisted in the workflow YAML and rendered on the canvas edges.
  - **Edge waypoints & label dragging**: Transition edges support intermediate waypoints for custom routing. Click an edge to select it, then:
    - Drag existing waypoint handles (colored circles) to reposition the edge path
    - Hover near the midpoint of any edge segment to reveal a "+" handle; click or drag it to insert a new waypoint
    - Drag the transition label to reposition it independently of the edge path
    - Double-click a waypoint to remove it; double-click a label to reset its position
    - Waypoints and label positions are stored per-edge (keyed by target task name) in `TaskTransition.edge_waypoints` and `TaskTransition.label_positions`, serialized via `__chart_meta__` in the workflow YAML
    - Edge selection state (`SelectedEdgeInfo`) is managed in `WorkflowCanvas`; only the selected edge shows interactive handles
    - Multi-segment paths use Catmull-Rom → cubic Bezier conversion for smooth curves through waypoints (`buildSmoothPath` in `WorkflowEdges.tsx`)
  - **Orquesta-style `next` transitions**: Tasks use a `next: TaskTransition[]` array instead of flat `on_success`/`on_failure` fields. Each transition has `when` (condition), `publish` (variables), `do` (target tasks), plus optional `label`, `color`, `edge_waypoints`, and `label_positions`. See "Task Transition Model" above.
  - **No task type or task-level condition**: The UI does not expose task `type` or task-level `when` — all tasks are actions (workflows are also actions), and conditions belong on transitions. Parallelism is implicit via multiple `do` targets.
  - **Ref immutability**: When editing an existing workflow, the pack selector and workflow name fields are disabled — the ref cannot be changed after creation.

## Development Workflow

### Common Commands (Makefile)
```bash
make build              # Build all services
make build-release      # Release build
make test               # Run all tests
make test-integration   # Run integration tests
make fmt                # Format code
make clippy             # Run linter
make lint               # fmt + clippy

make run-api            # Run API service
make run-executor       # Run executor service
make run-worker         # Run worker service
make run-agent          # Run universal worker agent
make run-sensor         # Run sensor service
make run-notifier       # Run notifier service
make run-supervisor     # Run supervisor service

make db-create          # Create database
make db-migrate         # Run migrations
make db-reset           # Drop & recreate DB
```

### Database Operations
- **Migrations**: Located in `migrations/`, applied via `sqlx migrate run`
- **Test DB**: Separate `attune_test` database, setup with `make db-test-setup`
- **Schema**: All tables live in the configured application schema (normally `attune`) with auto-updating timestamps
- **Core Pack**: Load with `./scripts/load-core-pack.sh` after DB setup

### Testing
- **Architecture**: Schema-per-test isolation (each test gets unique `test_<uuid>` schema)
- **Parallel Execution**: Tests run concurrently without `#[serial]` constraints (4-8x faster)
- **Unit Tests**: In module files alongside code
- **Integration Tests**: In `tests/` directory
- **Test DB Required**: Use `make db-test-setup` before integration tests
- **Run**: `cargo test` or `make test` (parallel by default)
- **Verbose**: `cargo test -- --nocapture --test-threads=1`
- **Cleanup**: Schemas auto-dropped on test completion; orphaned schemas cleaned via `./scripts/cleanup-test-schemas.sh`
- **SQLx Offline Mode**: Enabled for compile-time query checking without live DB; regenerate with `cargo sqlx prepare`

### CLI Tool
```bash
cargo install --path crates/cli  # Install CLI
attune auth login                # Login
attune auth token-login --token attune_it_...  # Passwordless login with a revokable integration token
attune auth token create --identity-id 42 --label "CI bot"  # Create integration token (plaintext shown once)
attune auth token list --identity-id 42  # List integration token metadata
attune auth token revoke --identity-id 42 7 --reason "rotated"  # Revoke integration token
attune-mcp                       # Launch MCP stdio server using CLI profile/auth config
attune pack list                 # List packs
attune pack create --ref my_pack # Create empty pack (non-interactive)
attune pack create -i            # Create empty pack (interactive prompts)
attune pack upload ./path/to/pack  # Upload local pack to API (works with Docker)
attune pack register /opt/attune/packs/mypak  # Register from API-visible path
attune action execute <ref> --param key=value
attune action enable <ref>         # Enable action execution
attune action disable <ref>        # Disable action execution
attune trigger enable <ref>        # Enable trigger event ingress
attune trigger disable <ref>       # Disable trigger event ingress
attune sensor enable <ref>         # Enable sensor process lifecycle
attune sensor disable <ref>        # Disable sensor process lifecycle
attune queue enable <ref>          # Enable queue dispatch processing
attune queue disable <ref>         # Disable queue dispatch processing
attune policy list                 # List execution policies
attune policy show core.limit_echo # Show policy details
attune policy create --policy-ref core.limit_echo --name "Limit echo" --scope action --action core.echo --concurrency-limit 5 --on-concurrency enqueue --group-by customer_id
attune policy update core.limit_echo --priority 20 --rate-limit-max 100 --rate-limit-window 1h
attune policy enable core.limit_echo
attune policy disable core.limit_echo
attune policy delete core.limit_echo --yes
attune queue items <ref> preview --selector '$.payload.customer_id ? (@ == $id)' --vars-json '{"id":123}'  # Preview pending queue items selected by SQL/JSONPath
attune queue items <ref> update --selector '$.payload.customer_id ? (@ == $id)' --vars-json '{"id":123}' --patch-json '{"reviewed":true}'  # Merge-patch selected pending item payloads
attune queue items <ref> reprioritize --selector '$.metadata.source ? (@ == "import")' --priority 50  # Reprioritize selected pending items
attune queue items <ref> delete --selector '$.payload.customer_id ? (@ == $id)' --vars-json '{"id":123}'  # Cancel selected pending items
attune execution list            # Monitor executions
attune execution list --trace-tag core.timer.1234  # Filter executions by exact trace tag
attune execution trace-report core.timer.1234  # Fetch consolidated cross-system trace report
attune key list                  # List all keys (values redacted)
attune key list --owner-type pack  # Filter keys by owner type
attune key show my_token         # Show key details (value shown as SHA-256 hash)
attune key show my_token -d      # Show key details with decrypted/actual value
attune key create --ref my_token --name "My Token" --value "secret123"  # Create unencrypted string key (default)
attune key create --ref my_token --name "My Token" --value '{"user":"admin","pass":"s3cret"}' # Create unencrypted structured key
attune key create --ref my_token --name "My Token" --value "secret123" -e  # Create encrypted string key
attune key create --ref my_token --name "My Token" --value "secret123" --encrypt --owner-type pack --owner-pack-ref core  # Create encrypted pack-scoped key
attune key update my_token --value "new_secret"  # Update key value (string)
attune key update my_token --value '{"host":"db.example.com","port":5432}'  # Update key value (structured)
attune key update my_token --name "Renamed Token"  # Update key name
attune key delete my_token       # Delete a key (with confirmation)
attune key delete my_token --yes # Delete without confirmation
attune workflow upload actions/deploy.yaml  # Upload workflow action to existing pack
attune workflow upload actions/deploy.yaml --force  # Update existing workflow
attune workflow list             # List all workflows
attune workflow list --pack core # List workflows in a pack
attune workflow show core.install_packs  # Show workflow details + task summary
attune workflow delete core.my_workflow --yes  # Delete a workflow
attune artifact list                 # List all artifacts
attune artifact list --type file_text --visibility public  # Filter artifacts
attune artifact list --execution 42  # List artifacts for an execution
attune artifact show 1               # Show artifact by ID
attune artifact show mypack.build_log  # Show artifact by ref
attune artifact create --ref mypack.build_log --scope action --owner mypack.deploy --type file_text --name "Build Log"
attune artifact upload 1 ./output.log  # Upload file as new version
attune artifact upload 1 ./data.json --content-type application/json --created-by "cli"
attune artifact download 1           # Download latest version to auto-named file
attune artifact download 1 -V 3     # Download specific version
attune artifact download 1 -o ./local.txt  # Download to specific path
attune artifact download 1 -o -     # Download to stdout
attune artifact delete 1             # Delete artifact (with confirmation)
attune artifact delete 1 --yes       # Delete without confirmation
attune artifact version list 1       # List all versions of artifact 1
attune artifact version show 1 3     # Show details of version 3
attune artifact version upload 1 ./new-file.txt  # Upload file as new version
attune artifact version create-json 1 '{"key":"value"}'  # Create JSON version
attune artifact version download 1 2 -o ./v2.txt  # Download version 2
attune artifact version delete 1 2 --yes  # Delete version 2
```

**MCP server binary** (`attune-mcp`):
- Lives in the same crate as the CLI (`crates/cli`) as a second binary target (`src/bin/attune-mcp.rs`)
- Uses the same CLI config/profile/auth state as `attune` (`~/.config/attune/config.yaml`, `ATTUNE_PROFILE`, `ATTUNE_API_URL`)
- Exposes a curated MCP stdio tool surface backed by the existing Attune API: actions, workflows, executions, queues, artifacts, events (read-only), and inquiries
- Intentionally does **not** expose direct event creation because the Attune API restricts event emission to sensor/execution-token flows

**Pack Upload vs Register**:
- `attune pack upload <local-path>` — Tarballs the local directory and POSTs it to `POST /api/v1/packs/upload`. Works regardless of whether the API is local or in Docker. This is the primary way to install packs from your local machine into a Dockerized system.
- `attune pack register <server-path>` — Sends a filesystem path string to the API (`POST /api/v1/packs/register`). Only works if the path is accessible from inside the API container (e.g. `/opt/attune/packs/...` or `/opt/attune/packs.dev/...`).

**Workflow Upload** (`attune workflow upload <action-yaml-path>`):
- Reads the local action YAML file and extracts the `workflow_file` field to find the companion workflow YAML
- Determines the pack from the action ref (e.g., `mypack.deploy` → pack `mypack`, name `deploy`)
- The `workflow_file` path is resolved relative to the action YAML's parent directory (same as how pack loaders resolve it relative to the `actions/` directory)
- Constructs a `SaveWorkflowFileRequest` JSON payload combining action metadata (label, parameters, output, tags) with the workflow definition (version, vars, tasks, output_map) and POSTs to `POST /api/v1/packs/{pack_ref}/workflow-files`
- On 409 Conflict (workflow already exists), fails unless `--force` is passed, in which case it PUTs to `PUT /api/v1/workflows/{ref}/file` to update
- Does NOT require a full pack upload — individual workflow actions can be added to existing packs independently
- **Important**: The action YAML MUST contain a `workflow_file` field; regular (non-workflow) actions should be uploaded as part of a pack

**Pack Upload API endpoint**: `POST /api/v1/packs/upload` — accepts `multipart/form-data` with:
- `pack` (required): a `.tar.gz` archive of the pack directory
- `force` (optional, text): `"true"` to overwrite an existing pack with the same ref
- `skip_tests` (optional, text): `"true"` to skip test execution after registration

The server extracts the archive to a temp directory, finds the `pack.yaml` (at root or one level deep), then moves it to `{packs_base_dir}/{pack_ref}/` and calls `register_pack_internal`.

**Pack Upload Safety**: Archive extraction is hardened against malicious tarballs by `safe_unpack` in `crates/api/src/routes/packs.rs`. It rejects path traversal (`..`), absolute paths, symlinks, hardlinks, character/block devices, and FIFOs; aborts on per-entry size, total extracted size, or file-count overruns; and disables `tar`'s `overwrite`, `unpack_xattrs`, `preserve_permissions`, and `preserve_mtime`. Limits are configurable via the `pack_upload` config block (`PackUploadConfig` in `crates/common/src/config.rs`). Defaults: `max_extracted_size_bytes = 100 MB`, `max_file_count = 10_000`, `max_per_entry_size_bytes = 50 MB`, `allow_symlinks = false`. The endpoint is RBAC-gated on `packs:create`; identities without that grant receive 403.

## Test Failure Protocol

**Proactively investigate and fix test failures when discovered, even if unrelated to the current task.**

### Guidelines:
- **ALWAYS report test failures** to the user with relevant error output
- **ALWAYS run tests** after making changes: `make test` or `cargo test`
- **DO fix immediately** if the cause is obvious and fixable in 1-2 attempts
- **DO ask the user** if the failure is complex, requires architectural changes, or you're unsure of the cause
- **NEVER silently ignore** test failures or skip tests without approval
- **Gather context**: Run with `cargo test -- --nocapture --test-threads=1` for details

### Priority:
- **Critical** (build/compile failures): Fix immediately
- **Related** (affects current work): Fix before proceeding
- **Unrelated**: Report and ask if you should fix now or defer

When reporting, ask: "Should I fix this first or continue with [original task]?"

## Code Quality: Zero Warnings Policy

**Maintain zero compiler warnings across the workspace.** Clean builds ensure new issues are immediately visible.

### Workflow
- **Check after changes:** `cargo check --all-targets --workspace`
- **Before completing work:** Fix or document any warnings introduced
- **End of session:** Verify zero warnings before finishing

### Handling Warnings
- **Fix first:** Remove dead code, unused imports, unnecessary variables
- **Prefix `_`:** For intentionally unused variables that document intent
- **Use `#[allow(dead_code)]`:** For API methods intended for future use (add doc comment explaining why)
- **Never ignore blindly:** Every suppression needs a clear rationale

### Conservative Approach
- Preserve methods that complete a logical API surface
- Keep test helpers that are part of shared infrastructure
- When uncertain about removal, ask the user

### Red Flags
- ❌ Introducing new warnings
- ❌ Blanket `#[allow(warnings)]` without specific justification
- ❌ Accumulating warnings over time

## File Naming & Location Conventions

### When Adding Features:
- **New API Endpoint**:
  - Route handler in `crates/api/src/routes/<domain>.rs`
  - DTO in `crates/api/src/dto/<domain>.rs`
  - Update `routes/mod.rs` and main router
- **New Domain Model**:
  - Add to `crates/common/src/models.rs`
  - Create migration in `migrations/YYYYMMDDHHMMSS_description.sql`
  - Add repository in `crates/common/src/repositories/<entity>.rs`
- **New Service**: Add to `crates/` and update workspace `Cargo.toml` members
- **Configuration**: Update `crates/common/src/config.rs` with serde defaults
- **Documentation**: Add to `docs/` directory

### Important Files
- `crates/common/src/models.rs` - All domain models
- `crates/common/src/error.rs` - Error types
- `crates/common/src/config.rs` - Configuration structure
- `crates/common/src/repositories/retention.rs` - Runtime database retention repository used by supervisor
- `crates/api/src/routes/mod.rs` - API routing
- `crates/supervisor/src/main.rs` - Supervisor maintenance service entrypoint
- `crates/worker/src/agent_main.rs` - Universal worker agent entrypoint
- `crates/worker/src/runtime_detect.rs` - Runtime auto-detection module (probes for interpreters)
- `crates/worker/src/dynamic_runtime.rs` - Dynamic runtime registration (auto-registers detected runtimes into DB)
- `config.development.yaml` - Dev configuration
- `Cargo.toml` - Workspace dependencies
- `Makefile` - Development commands
- `docker/Dockerfile.optimized` - Optimized service builds (api, executor, notifier, supervisor)
- `docker/Dockerfile.agent` - Statically-linked agent binary (musl, for injection into any container)
- `docker/Dockerfile.web` - Web UI build
- `docker/Dockerfile.pack-binaries` - Separate pack binary builder (cargo-zigbuild + musl static linking, 3 stages: builder, output, pack-binaries-init)
- `scripts/build-pack-binaries.sh` - Build pack binaries script
- `crates/common/src/repositories/pack_registry_index.rs` - API-managed pack index configuration repository
- `crates/common/src/artifact_transport/mod.rs` - ArtifactFileTransport trait + build_transport factory
- `crates/common/src/artifact_transport/volume.rs` - VolumeTransport (shared filesystem)
- `crates/common/src/artifact_transport/api.rs` - ApiTransport (HTTP-based file transfer)
- `crates/common/src/artifact_transport/detection.rs` - Transport auto-detection via sentinel file
- `crates/api/src/routes/internal_files.rs` - Internal file transfer endpoints for remote workers
- `crates/api/src/routes/sensor_logs.rs` - Sensor log list/download endpoints
- `crates/api/src/dashboard_data/contracts.rs` - Dashboard source registry contract catalog (`available_now` / `partial` / `planned`)
- `crates/api/src/dashboard_data/query_safety.rs` - Typed dashboard query-binding, ref/path allow-list, and limit guards
- `crates/api/src/dashboard_data/watermark.rs` - Watermark cutover planner for aggregate+tail `[start,end)` execution
- `crates/api/src/dto/dashboard.rs` - Dashboard metadata + data endpoint DTO contract
- `crates/common/src/dashboard_spec.rs` - Shared dashboard spec validation for loader and API boundaries
- `crates/sensor/src/sensor_log.rs` - Per-sensor rotating log writer

## Common Pitfalls to Avoid
1. **NEVER** commit to git - only the user commits. You may stage files (`git add`) but never run `git commit`.
2. **NEVER** bypass repositories - always use the repository layer for DB access
2. **NEVER** forget `RequireAuth` middleware on protected endpoints
3. **NEVER** hardcode service URLs - use configuration
4. **NEVER** commit secrets in config files (use env vars in production)
5. **NEVER** hardcode schema prefixes in SQL queries - rely on PostgreSQL `search_path` mechanism
6. **NEVER** copy packs into Dockerfiles - they are mounted as volumes
7. **NEVER** put workflow definition content directly in action YAML — use a separate `.workflow.yaml` file in `actions/workflows/` and reference it via `workflow_file` in the action YAML
8. **ALWAYS** use PostgreSQL enum type mappings for custom enums
9. **ALWAYS** use transactions for multi-table operations
10. **ALWAYS** start with `attune/` or correct crate name when specifying file paths
11. **ALWAYS** convert runtime names to lowercase for comparison (database may store capitalized)
12. **ALWAYS** use optimized Dockerfiles for new services (selective crate copying)
13. **REMEMBER** IDs are `i64`, not `i32` or `uuid`
14. **REMEMBER** schema is determined by `search_path`, not hardcoded in queries (Docker/development/production configs use the `attune` schema; tests use isolated `test_*` schemas)
15. **REMEMBER** to regenerate SQLx metadata after schema-related changes: `cargo sqlx prepare`
16. **REMEMBER** packs are volumes - update with restart, not rebuild
17. **REMEMBER** pack binaries are automatically built by `init-pack-binaries` in Docker Compose. For manual builds use `make docker-build-pack-binaries` or `./scripts/build-pack-binaries.sh`.
18. **REMEMBER** when adding mutable columns to `execution` or `worker`, add a corresponding `IS DISTINCT FROM` check to the entity's history trigger function in the TimescaleDB migration. Events and enforcements are hypertables without history tables — do NOT add frequently-mutated columns to them. Execution is both a hypertable AND has an `execution_history` table (because it is mutable with ~4 updates per row).
19. **REMEMBER** for large JSONB columns in history triggers (like `execution.result`), use `_jsonb_digest_summary()` instead of storing the raw value — see migration `000009_timescaledb_history`
20. **NEVER** use `SELECT *` on tables that have DB-only columns not in the Rust `FromRow` struct (e.g., `execution.workflow_def` exists in SQL but not in the `Execution` model). Define a `SELECT_COLUMNS` constant in the repository (see `execution.rs`, `pack.rs`, `runtime_version.rs` for examples) and reference it from all queries — including queries outside the repository (e.g., `timeout_monitor.rs` imports `execution::SELECT_COLUMNS`).ause runtime deserialization failures.
21. **REMEMBER** `execution`, `event`, and `enforcement` are all TimescaleDB hypertables — they **cannot be the target of FK constraints**. Any column referencing them (e.g., `inquiry.execution`, `workflow_execution.execution`, `execution.parent`) is a plain BIGINT with no FK and may become a dangling reference.

## Deployment
- **Target**: Distributed deployment with separate service instances
- **Docker**: Dockerfiles for each service (planned in `docker/` dir)
- **Config**: Use environment variables for secrets in production
- **Database**: PostgreSQL 14+ with connection pooling
- **Message Queue**: RabbitMQ required for service communication
- **Web UI**: Static files served separately or via API service

## Current Development Status
- ✅ **Complete**: Database migrations (44 tables, 12 timestamped core migrations), API service (most endpoints), common library, message queue infrastructure, repository layer, JWT auth, CLI tool, Web UI (basic + workflow builder + workflow timeline DAG), Executor service (core functionality + workflow orchestration), Worker service (shell/Python execution), Pack index management (standard JSON index entries with `git`/`archive` install sources, `pack_registry_index` ordered configuration, `/api/v1/pack-indices` management endpoints and web/CLI integration), Runtime version data model, TimescaleDB history/analytics, workflow orchestration/expression features, artifact content system, Runtime database retention supervisor (`attune-supervisor` with configurable per-target policies, PostgreSQL advisory-lock leadership, Timescale chunk drops for hypertables/continuous aggregates, guarded batched deletes for terminal/stale runtime rows, and `supervisor_run` dirty-shutdown detection), work queue foundations/API/executor/web UI, dashboard metadata/data contract foundation (`/api/v1/dashboards/{ref}` and `/api/v1/dashboards/{ref}/data`), CLI artifact management, CLI `--wait`, Workflow Timeline DAG visualization, and Universal Worker Agent phases 1-7.
- 🔄 **In Progress**: Sensor service, advanced workflow features beyond task retry and workflow-pausing inquiries (nested workflow context propagation), Python runtime dependency management, API/UI endpoints for runtime version management, Artifact UI (web UI for browsing/downloading artifacts), richer work queue observability/reporting surfaces (dispatch history, stats visualisation, generated client regeneration), dashboard data endpoint integration on top of the new source planner/query-safety/watermark abstractions, Notifier service WebSocket RBAC tightening (JWT auth on upgrade and a User-filter ACL are enforced; full RBAC integration via `AuthorizationService`, scope-aware execution/inquiry filtering, and per-notification authorization at broadcast time are pending — see TODOs in `crates/notifier/src/websocket_server.rs`)
- 📋 **Planned**: Execution policies, monitoring, export/archival to external storage

## Quick Reference

### Start Development Environment
```bash
# Start PostgreSQL and RabbitMQ
# Load core pack: ./scripts/load-core-pack.sh
# Start API: make run-api
# Start Web UI: cd web && npm run dev
```

### File Path Examples
- Models: `attune/crates/common/src/models.rs`
- API routes: `attune/crates/api/src/routes/actions.rs`
- Repositories: `attune/crates/common/src/repositories/execution.rs`
- Migrations: `attune/migrations/*.sql`
- Web UI: `attune/web/src/`
- Config: `attune/config.development.yaml`

### Documentation Locations
- API docs: `attune/docs/api-*.md`
- Configuration: `attune/docs/configuration.md`
- Architecture: `attune/docs/*-architecture.md`, `attune/docs/*-service.md`
- Testing: `attune/docs/testing-*.md`, `attune/docs/running-tests.md`, `attune/docs/schema-per-test.md`
- Docker optimization: `attune/docs/docker-layer-optimization.md`, `attune/docs/QUICKREF-docker-optimization.md`, `attune/docs/QUICKREF-buildkit-cache-strategy.md`
- Packs architecture: `attune/docs/QUICKREF-packs-volumes.md`, `attune/docs/DOCKER-OPTIMIZATION-SUMMARY.md`
- AI Agent Work Summaries: `attune/work-summary/*.md`
- Deployment: `attune/docs/deployment/structured-logging.md`, `attune/docs/deployment/production-deployment.md`
- DO NOT create additional documentation files in the root of the project. all new documentation describing how to use the system should be placed in the `attune/docs` directory, and documentation describing the work performed should be placed in the `attune/work-summary` directory.

## Work Summary & Reporting

**Avoid redundant summarization - summarize changes once at completion, not continuously.**

### Guidelines:
- **Report progress** during work: brief status updates, blockers, questions
- **Summarize once** at completion: consolidated overview of all changes made
- **Work summaries**: Write to `attune/work-summary/*.md` only at task completion, not incrementally
- **Avoid duplication**: Don't re-explain the same changes multiple times in different formats
- **What changed, not how**: Focus on outcomes and impacts, not play-by-play narration

### Good Pattern:
```
[Making changes with tool calls and brief progress notes]
...
[At completion]
"I've completed the task. Here's a summary of changes: [single consolidated overview]"
```

### Bad Pattern:
```
[Makes changes]
"So I changed X, Y, and Z..."
[More changes]
"To summarize, I modified X, Y, and Z..."
[Writes work summary]
"In this session I updated X, Y, and Z..."
```

## Maintaining the AGENTS.md file

**IMPORTANT: Keep this file up-to-date as the project evolves.**

After making changes to the project, you MUST update this `AGENTS.md` file if any of the following occur:

- **New dependencies added or major dependencies removed** (check package.json, Cargo.toml, requirements.txt, etc.)
- **Project structure changes**: new directories/modules created, existing ones renamed or removed
- **Architecture changes**: new layers, patterns, or major refactoring that affects how components interact
- **New frameworks or tools adopted** (e.g., switching from REST to GraphQL, adding a new testing framework)
- **Deployment or infrastructure changes** (new CI/CD pipelines, different hosting, containerization added)
- **New major features** that introduce new subsystems or significantly change existing ones
- **Style guide or coding convention updates**

### `AGENTS.md` Content inclusion policy
- DO NOT simply summarize changes in the `AGENTS.md` file. If there are existing sections that need updating due to changes in the application architecture or project structure, update them accordingly.
- When relevant, work summaries should instead be written to `attune/work-summary/*.md`

### Update procedure:
1. After completing your changes, review if they affect any section of `AGENTS.md`
2. If yes, immediately update the relevant sections
3. Add a brief comment at the top of `AGENTS.md` with the date and what was updated (optional but helpful)

### Update format:
When updating, be surgical - modify only the affected sections rather than rewriting the entire file. Maintain the existing structure and tone.

**Treat `AGENTS.md` as living documentation.** An outdated `AGENTS.md` file is worse than no `AGENTS.md` file, as it will mislead future AI agents and waste time.

## Project Documentation Index
[Attune Project Documentation Index]
|root: ./
|IMPORTANT: Prefer retrieval-led reasoning over pre-training-led reasoning
|IMPORTANT: This index provides a quick overview - use grep/read_file for details
|
| Format: path/to/dir:{file1,file2,...}
| '...' indicates truncated file list - use grep/list_directory for full contents
|
| To regenerate this index: make generate-agents-index
|
|docs:{MIGRATION-queue-separation-2026-02-03.md,QUICKREF-containerized-workers.md,QUICKREF-rabbitmq-queues.md,QUICKREF-sensor-worker-registration.md,QUICKREF-unified-runtime-detection.md,README.md,docker-deployment.md,pack-runtime-environments.md,worker-containerization.md,worker-containers-quickstart.md}
|docs/agent-personas:{README.md,action-author.md,ai-agent-action-author.md,cli-guide.md,pack-architect.md,pack-test-reviewer.md,rule-author.md,sensor-author.md,workflow-author.md}
|docs/api:{api-actions.md,api-completion-plan.md,api-events-enforcements.md,api-executions.md,api-inquiries.md,api-pack-testing.md,api-pack-workflows.md,api-packs.md,api-rules.md,api-secrets.md,api-triggers-sensors.md,api-work-queues.md,api-workflows.md,openapi-client-generation.md,openapi-spec-completion.md}
|docs/architecture:{executor-service.md,notifier-service.md,pack-management-architecture.md,queue-architecture.md,sensor-service.md,trigger-sensor-architecture.md,web-ui-architecture.md,webhook-system-architecture.md,worker-service.md}
|docs/authentication:{auth-quick-reference.md,authentication.md,secrets-management.md,security-review-2024-01-02.md,service-accounts.md,token-refresh-quickref.md,token-rotation.md}
|docs/cli:{cli-profiles.md,cli.md}
|docs/configuration:{CONFIG_README.md,config-troubleshooting.md,configuration.md,env-to-yaml-migration.md}
|docs/dependencies:{dependency-deduplication-results.md,dependency-deduplication.md,dependency-isolation.md,dependency-management.md,http-client-consolidation-complete.md,http-client-consolidation-plan.md,sea-query-removal.md,serde-yaml-migration.md,workspace-dependency-compliance-audit.md}
|docs/deployment:{operational-visibility.md,ops-runbook-queues.md,production-deployment.md,structured-logging.md,supervisor.md}
|docs/development:{QUICKSTART-vite.md,WORKSPACE_SETUP.md,agents-md-index.md,compilation-notes.md,dead-code-cleanup.md,documentation-organization.md,vite-dev-setup.md}
|docs/examples:{complete-workflow.yaml,pack-test-demo.sh,registry-index.json,rule-parameter-examples.md,simple-workflow.yaml}
|docs/guides:{QUICKREF-timer-happy-path.md,quick-start.md,quickstart-example.md,quickstart-timer-demo.md,timer-sensor-quickstart.md,workflow-quickstart.md}
|docs/migrations:{workflow-task-execution-consolidation.md}
|docs/packs:{PACK_TESTING.md,QUICKREF-git-installation.md,core-pack-integration.md,pack-install-testing.md,pack-installation-git.md,pack-registry-cicd.md,pack-registry-spec.md,pack-structure.md,pack-testing-framework.md}
|docs/performance:{QUICKREF-performance-optimization.md,log-size-limits.md,performance-analysis-workflow-lists.md,performance-before-after-results.md,performance-context-cloning-diagram.md}
|docs/plans:{schema-per-test-refactor.md,timescaledb-entity-history.md,universal-worker-agent.md}
|docs/sensors:{CHECKLIST-sensor-worker-registration.md,COMPLETION-sensor-worker-registration.md,SUMMARY-database-driven-detection.md,database-driven-runtime-detection.md,native-runtime.md,sensor-authentication-overview.md,sensor-interface.md,sensor-lifecycle-management.md,sensor-runtime.md,sensor-service-setup.md,sensor-worker-registration.md}
|docs/testing:{e2e-test-plan.md,running-tests.md,schema-per-test.md,test-user-setup.md,testing-authentication.md,testing-dashboard-rules.md,testing-status.md}
|docs/web-ui:{web-ui-pack-testing.md,websocket-usage.md}
|docs/webhooks:{webhook-manual-testing.md,webhook-testing.md}
|docs/workflows:{dynamic-parameter-forms.md,execution-hierarchy.md,inquiry-handling.md,parameter-mapping-status.md,rule-parameter-mapping.md,rule-trigger-params.md,workflow-execution-engine.md,workflow-implementation-plan.md,workflow-orchestration.md,workflow-summary.md}
|scripts:{check-workspace-deps.sh,cleanup-test-schemas.sh,create-test-user.sh,create_test_user.sh,generate-python-client.sh,generate_agents_md_index.py,load-core-pack.sh,load_core_pack.py,quick-test-happy-path.sh,seed_core_pack.sql,seed_runtimes.sql,setup-db.sh,setup-e2e-db.sh,setup_timer_echo_rule.sh,start-all-services.sh,start-e2e-services.sh,start_services_test.sh,status-all-services.sh,stop-all-services.sh,stop-e2e-services.sh,...}
|work-summary:{2025-01-console-logging-cleanup.md,2025-01-token-refresh-improvements.md,2025-01-websocket-duplicate-connection-fix.md,2026-02-02-unified-runtime-verification.md,2026-02-03-canonical-message-types.md,2026-02-03-inquiry-queue-separation.md,2026-02-04-event-generation-fix.md,README.md,auto-populate-ref-from-label.md,buildkit-cache-implementation.md,collapsible-navigation-implementation.md,containerized-workers-implementation.md,docker-build-race-fix.md,docker-containerization-complete.md,docker-migrations-startup-fix.md,empty-pack-creation-ui.md,git-pack-installation.md,pack-runtime-environments.md,sensor-service-cleanup-standalone-only.md,sensor-worker-registration.md,...}
|work-summary/changelogs:{API-COMPLETION-SUMMARY.md,CHANGELOG.md,CLEANUP_SUMMARY_2026-01-27.md,FIFO-ORDERING-COMPLETE.md,MIGRATION_CONSOLIDATION_SUMMARY.md,cli-integration-tests-summary.md,core-pack-setup-summary.md,web-ui-session-summary.md,webhook-phase3-summary.md,webhook-testing-summary.md,workflow-loader-summary.md}
|work-summary/features:{AUTOMATIC-SCHEMA-CLEANUP-ENHANCEMENT.md,TESTING-TIMER-DEMO.md,e2e-test-schema-issues.md,openapi-spec-verification.md,sensor-runtime-implementation.md,sensor-service-implementation.md}
|work-summary/migrations:{2026-01-17-orquesta-refactoring.md,2026-01-24-generated-client-migration.md,2026-01-27-workflow-migration.md,DEPLOYMENT-READY-performance-optimization.md,MIGRATION_NEXT_STEPS.md,migration_comparison.txt,migration_consolidation_status.md}
|work-summary/phases:{2025-01-policy-ordering-plan.md,2025-01-secret-passing-fix-plan.md,2025-01-workflow-performance-analysis.md,PHASE-5-COMPLETE.md,PHASE_1_1_SUMMARY.txt,PROBLEM.md,Pitfall-Resolution-Plan.md,SENSOR_SERVICE_README.md,StackStorm-Lessons-Learned.md,StackStorm-Pitfalls-Analysis.md,orquesta-refactor-plan.md,phase-1-1-complete.md,phase-1.2-models-repositories-complete.md,phase-1.2-repositories-summary.md,phase-1.3-test-infrastructure-summary.md,phase-1.3-yaml-validation-complete.md,phase-1.4-COMPLETE.md,phase-1.4-loader-registration-progress.md,phase-1.5-COMPLETE.md,phase-1.6-pack-integration-complete.md,...}
|work-summary/sessions:{2024-01-13-event-enforcement-endpoints.md,2024-01-13-inquiry-endpoints.md,2024-01-13-integration-testing-setup.md,2024-01-13-route-conflict-fix.md,2024-01-13-secret-management-api.md,2024-01-17-sensor-runtime.md,2024-01-17-sensor-service-session.md,2024-01-20-core-pack-unit-tests.md,2024-01-20-pack-testing-framework-phase1.md,2024-01-21-pack-registry-phase1.md,2024-01-21-pack-registry-phase2.md,2024-01-22-pack-registry-phase3.md,2024-01-22-pack-registry-phase4.md,2024-01-22-pack-registry-phase5.md,2024-01-22-pack-registry-phase6.md,2025-01-13-phase-1.4-session.md,2025-01-13-yaml-configuration.md,2025-01-16_migration_consolidation.md,2025-01-17-performance-optimization-complete.md,2025-01-18-timer-triggers.md,...}
|work-summary/status:{ACCOMPLISHMENTS.md,COMPILATION_STATUS.md,FIFO-ORDERING-STATUS.md,FINAL_STATUS.md,PROGRESS.md,SENSOR_STATUS.md,TEST-STATUS.md,TODO.OLD.md,TODO.md}
