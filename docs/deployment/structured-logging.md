# Structured service logging contract

This document defines the canonical Attune operator contract for structured logs.
It closes the loop on the current logging work: JSON `stdout` defaults, shared
tracing initialization, service-log forwarding via Docker/container streams, and
private artifact-backed runtime logs.

## Two log planes: keep them separate

| Plane | Transport / storage | Primary content | Audience | Canonical source of truth |
| --- | --- | --- | --- | --- |
| **Service logs** | Container `stdout` / `stderr`, forwarded by Docker logging driver or host agent | Attune service lifecycle, scheduling, metadata-only runtime-log lifecycle events, warnings, errors | Operators / observability platforms | Forwarded container log stream |
| **Runtime logs** | Artifact-backed `artifact` + `artifact_version` rows/files, classified `runtime_log` | Raw action stdout/stderr and raw sensor stdout/stderr | Authorized Attune users investigating executions/sensors | Private artifact store |

Rules:

1. **Service logs are for platform behavior and metadata**, not raw task output.
2. **Runtime logs are the authoritative raw stdout/stderr record** and remain
   private source-of-truth artifacts with `classification=runtime_log`.
3. Forwarders such as Datadog and Splunk should ingest **service logs** from the
   container stream. They may ingest runtime-log *metadata* if needed, but they
   should not rely on mirrored raw stdout/stderr content from service logs.

## Canonical service-log schema

Attune service logs are emitted by the shared tracing initializer. In JSON mode
(the Docker/distribution default), each event uses the `tracing_subscriber` JSON
shape with a stable Attune field contract layered inside it.

### Required JSON envelope fields

These keys are required on every JSON service-log event emitted by Attune
services:

| Field | Location | Type | Notes |
| --- | --- | --- | --- |
| `timestamp` | top-level | string | RFC 3339 timestamp emitted by the tracing formatter |
| `level` | top-level | string | Log severity (`TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`) |
| `fields` | top-level | object | Event payload emitted by `tracing` |
| `fields.message` | nested | string | Human-readable event message |
| `threadId` | top-level | string | Included because shared tracing enables thread ids |

Additional top-level keys such as `span` / `spans` may appear when a log is
emitted inside instrumented spans. Preserve them if present.

### Required collector/container metadata

The application JSON body does **not** carry service identity tags by itself.
Operators must preserve the container metadata shipped by Compose:

| Metadata | Source | Purpose |
| --- | --- | --- |
| `com.attune.service` | Compose label per service | Stable Attune service identifier (`api`, `executor`, `worker-shell`, etc.) |
| `com.attune.log.contract=container-stdout-stderr` | Compose shared label | Declares the supported forwarding contract |
| `com.attune.log.transport=docker` | Compose shared label | Declares current transport |
| `com.attune.log.volume_hint=non-forwarding` | Compose shared label | Reminds operators that named log volumes are not the forwarding interface |
| `service` | Datadog label `com.datadoghq.tags.service` or mapped equivalent | Platform service grouping |
| `env` | Datadog label `com.datadoghq.tags.env` or mapped equivalent | Deployment environment |
| `version` | Datadog label `com.datadoghq.tags.version` or mapped equivalent | Build / release version |

### Canonical example

```json
{
  "timestamp": "2026-06-21T18:44:17.983Z",
  "level": "INFO",
  "fields": {
    "message": "Tracing initialized",
    "level": "info",
    "level_source": "config.log.level",
    "format": "json",
    "initialized": true
  },
  "threadId": "ThreadId(01)"
}
```

A forwarder should enrich that event with container metadata such as:

- `com.attune.service=api`
- `com.attune.log.contract=container-stdout-stderr`
- `service=attune-api`
- `env=docker`
- `version=local`

## Recommended optional domain fields

Use snake_case structured fields inside `fields` whenever a log event needs
Attune domain context.

| Field | Type | Use when |
| --- | --- | --- |
| `component` | string | Emitting subsystem name inside a service (`scheduler`, `artifact_transport`, `sensor_manager`) |
| `status` / `outcome` | string | State transitions and result summaries |
| `execution_id` | integer | Referring to a specific execution |
| `parent_execution_id` | integer | Linking workflow children to a parent execution |
| `workflow_execution_id` | integer | Referring to the workflow execution record |
| `event_id`, `enforcement_id`, `inquiry_id`, `artifact_id` | integer | Referring to Attune database records |
| `action_ref`, `trigger_ref`, `rule_ref`, `pack_ref`, `queue_ref`, `sensor_ref`, `artifact_ref` | string | Ref-based correlation |
| `worker_id` | integer | Referring to a worker row |
| `worker_name`, `worker_role` | string | Operational worker context |
| `dispatch_id`, `queue_item_id` | integer | Queue dispatch / leased item context |
| `trace_tag`, `correlation_id` | string | End-to-end or incident correlation |
| `stream` | string | Runtime log stream name (`stdout`, `stderr`) |
| `classification` | string | Artifact/log class (`runtime_log`, `general`) |
| `path` / `file_path` | string | Artifact or transport file path |
| `size_bytes`, `bytes_written`, `bytes_truncated` | integer | File/log sizing metrics |
| `truncated`, `cordoned`, `heartbeat_stale` | boolean | Boolean state flags |
| `observed_at`, `started_at`, `finished_at`, `next_restart_at` | string | Domain timestamps beyond the event `timestamp` |
| `attempt`, `retry_count`, `active_rule_count` | integer | Retry / concurrency counters |

## Naming conventions

Use these conventions for all structured service-log fields:

- **snake_case only** for nested event fields (`execution_id`, not
  `executionId`).
- **`*_id`** for numeric database identifiers.
- **`*_ref`** for stable logical refs (`core.echo`, `core.alert`, etc.).
- **`*_at`** for RFC 3339 timestamps representing domain times.
- **`*_count`** for counts, **`*_bytes`** for byte sizes.
- **Booleans stay booleans** (`truncated: true`), not stringified flags.
- Prefer **singular, stable nouns** (`stream`, `status`, `component`) over
  free-form label names.
- Put detail-rich prose in `fields.message`; use separate typed fields for data
  the collector may parse.

## Indexing and tagging guidance

Treat indexing/faceting/tagging separately from simple JSON extraction.

### Safe low-cardinality tags / facets

These fields are appropriate for always-on tags, indexed fields, or facets:

- `service`, `env`, `version`
- `com.attune.service`
- `level`
- `component`
- `status`, `outcome`
- `worker_role`
- `stream`
- `classification`

### Bounded-cardinality fields (opt-in facets)

These can be useful as searchable facets when your estate is small enough or the
value set is intentionally bounded:

- `pack_ref`
- `action_ref`
- `trigger_ref`
- `rule_ref`
- `queue_ref`
- `sensor_ref`

### High-cardinality fields (attributes only by default)

Do **not** make these default indexed tags/facets unless you have a deliberate
reason and retention budget:

- `execution_id`, `parent_execution_id`, `workflow_execution_id`
- `event_id`, `enforcement_id`, `artifact_id`, `dispatch_id`, `queue_item_id`
- `artifact_ref`
- `trace_tag`, `correlation_id`
- `path`, `file_path`
- `fields.message`
- stack traces, error strings, stderr excerpts, raw payload snippets

## Operator mapping guidance

### Datadog

Recommended mapping with the current Compose contract:

1. Collect **container logs** from Docker `stdout` / `stderr`.
2. Parse the JSON body emitted by Attune services.
3. Preserve / map these Compose labels:
   - `com.datadoghq.tags.service` → Datadog `service`
   - `com.datadoghq.tags.env` → Datadog `env`
   - `com.datadoghq.tags.version` → Datadog `version`
   - `com.attune.service` → normal tag/facet for Attune-internal service identity
   - `com.attune.log.contract`, `com.attune.log.transport` → informational tags
4. Promote only low-cardinality Attune fields to facets by default.
5. Keep high-cardinality fields as searchable attributes.
6. Do not expect raw execution/sensor stdout in service-log events; retrieve raw
   runtime output from `runtime_log` artifacts.

### Splunk

Recommended mapping with the current Compose contract:

1. Ingest the same Docker container stream either by:
   - tailing Docker `json-file` logs from the host/agent layer, or
   - using a Compose override with the Splunk Docker logging driver.
2. Extract the inner Attune JSON body as the event payload.
3. Preserve / promote Compose label metadata into Splunk fields, for example:
   - `service`
   - `env`
   - `version`
   - `attune_service` (mapped from `com.attune.service`)
   - `attune_log_contract` (mapped from `com.attune.log.contract`)
4. Make low-cardinality fields searchable/indexed first; keep `message`, ids,
   trace tags, paths, and stack traces as search-time fields unless you have a
   specific operational reason to index them.
5. Use Attune APIs / UI for private raw runtime logs instead of expecting them in
   service-log forwarding.

## Compose contract summary

Current checked-in Compose defaults are the supported operator contract:

- `log.format: json` in Docker/distribution configs
- Docker `logging.driver=${ATTUNE_DOCKER_LOGGING_DRIVER:-json-file}`
- Rotation via `ATTUNE_DOCKER_LOG_MAX_SIZE` and `ATTUNE_DOCKER_LOG_MAX_FILE`
- Stable per-service label: `com.attune.service`
- Stable shared labels:
  - `com.attune.log.contract=container-stdout-stderr`
  - `com.attune.log.transport=docker`
  - `com.attune.log.volume_hint=non-forwarding`
- Datadog compatibility labels:
  - `com.datadoghq.tags.service`
  - `com.datadoghq.tags.env`
  - `com.datadoghq.tags.version`

Named `/opt/attune/logs` volumes are kept for compatibility and local
inspection, but they are **not** the forwarding contract for operators.

## Operator validation checklist

- [ ] `docker inspect <container> --format '{{json .HostConfig.LogConfig}}'`
      shows the expected logging driver/options.
- [ ] `docker inspect <container> --format '{{json .Config.Labels}}'`
      contains `com.attune.service`, `com.attune.log.contract`, and the expected
      service/env/version tags.
- [ ] `docker logs --tail 5 <container>` shows JSON events with `timestamp`,
      `level`, `fields.message`, and `threadId`.
- [ ] Datadog or Splunk receives those container events with preserved service
      metadata.
- [ ] Only low-cardinality fields are promoted to always-on tags/facets by
      default.
- [ ] Raw action/sensor stdout/stderr is **not** expected in service logs.
- [ ] Runtime log artifacts remain private and are discoverable as
      `classification=runtime_log` through Attune APIs/UI.
