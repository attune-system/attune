# Operational visibility

Attune tracks operational health for action workers, sensor workers, running executions, and long-lived pack sensor processes. The current implementation uses existing worker heartbeats plus explicit operator cordon state so expected maintenance can be distinguished from unexpected component loss.

## Worker health and cordon

Action workers and sensor workers both register in the `worker` table. `worker.status` remains the observed lifecycle state (`active`, `inactive`, `busy`, `error`), while cordon intent is stored separately:

- `cordoned`
- `cordon_reason`
- `cordoned_by`
- `cordoned_at`

Cordoned workers may still be alive and heartbeating, but they are treated as intentionally unschedulable. The executor excludes cordoned action workers from new action scheduling. Worker list responses include backend-computed health fields such as `heartbeat_age_seconds`, `heartbeat_stale`, and `health_state`, and the dashboard surfaces unexpected offline action workers, unexpected offline sensor workers, and cordoned workers.

Operators can use:

- `POST /api/v1/workers/{id}/cordon`
- `POST /api/v1/workers/{id}/uncordon`

## Running execution reconciliation

The executor timeout monitor reconciles executions that are already `running` on unavailable workers. If the assigned worker is stale, inactive, or errored, the execution is moved to `abandoned` with structured result metadata describing the worker, last heartbeat, heartbeat age, cordon state, and reconciliation source.

Abandoned executions are not restarted automatically. The executor publishes the normal execution-completed message after reconciliation so workflow advancement, queue completion handling, notifier updates, and execution history see a terminal transition.

## `core.alert`

The core pack defines a `core.alert` trigger for structured operational alerts. System components create these events through the shared `attune_common::system_alert` helper, which inserts an event and publishes `EventCreated` for normal rule evaluation.

Alerts are emitted for unexpected worker unavailability and execution abandonment caused by unexpected worker loss. Alerts are suppressed for cordoned workers because their shutdown is considered expected operator activity.

The alert payload includes fields for severity, category, failure type, component type/id/ref, worker role, observed timestamp, summary, details, and correlation id.

## Work queue lifecycle triggers

The core pack defines two work-queue lifecycle triggers for operators that want to route queue activity to alerts, metrics, or audit workflows:

- `core.queue_started` fires after the executor successfully publishes the first dispatch for a queue whose latest lifecycle state was empty or unknown.
- `core.queue_empty` fires after a queue-processing execution reaches a terminal state, there are no active dispatches for that queue, and there are no queued/retry items left.

Both events are stored as normal `event` rows and published through `EventCreated`, so regular rules can subscribe to them. Payloads include queue id/ref, dispatch id, execution id, dispatch action ref, leased item count, and `observed_at`. The empty event also includes the terminal dispatch status plus active/ready counts observed after finalization.

## Sensor logs

Sensor stdout and stderr are written to per-sensor rotating log files under the configured artifacts directory:

```text
{artifacts_dir}/sensors/{sensor_ref}/stdout.log
{artifacts_dir}/sensors/{sensor_ref}/stderr.log
```

Artifact-backed sensor logs are the authoritative record. Per-line stdout/stderr
mirroring into tracing is disabled by default to avoid duplicate log ingestion,
but lifecycle events such as stream closure and log-write failures still appear
in service logs.

Those sensor log artifacts are explicitly classified as `runtime_log` and remain
private. The same policy now applies to worker-created execution stdout/stderr
artifacts. Operators and downstream observability can filter artifact metadata by
classification to discover log artifacts, while the raw stdout/stderr payload
stays in the artifact store as the source of truth.

The worker and sensor services emit metadata-only lifecycle tracing around log
version allocation, promotion/finalization, truncation, and stream closure. The
events include identifiers, stream names, paths, and sizes only; they do not
mirror raw stdout/stderr content into service logs. See
[`structured-logging.md`](structured-logging.md) for the canonical separation
between forwarded service logs and private artifact-backed runtime logs.

The sensor log API supports tailing current log files:

```http
GET /api/v1/sensors/{sensor_ref}/logs
GET /api/v1/sensors/{sensor_ref}/logs/{stream}?tail=200
```

The sensor detail page exposes stdout/stderr tabs with a configurable tail count and follow polling.

## Sensor process health and restarts

Managed pack sensor process state is persisted in `sensor_process`, with field-level changes mirrored to the `sensor_process_history` hypertable. The live state records the sensor, owning sensor worker, process status, pid, consecutive failure count, last exit code/signal, start/stop timestamps, next restart time, stderr excerpt, active-rule count, and alert bookkeeping.

`SensorManager` actively checks child processes with non-blocking `try_wait`. Unexpected exits while enabled rules still reference the sensor are marked as `backoff`, stderr context is captured from the rotating stderr log, and the sensor is restarted with capped exponential backoff. Intentional stops, disabled/deleted sensors, sensors with no active rules, and placement mismatches are marked stopped and are not restarted.

Repeated exits emit `core.alert` once the failure threshold is crossed. The alert includes the sensor id/ref, worker id/name, exit code or signal, consecutive failure count, active-rule count, restart backoff, next restart timestamp, and stderr excerpt. Alert markers on `sensor_process` prevent the same failure count from alerting repeatedly.

## Sensor worker placement

Pack sensors can declare the same placement vocabulary as actions:

```yaml
worker_selector:
  gpu: "false"
worker_tolerations:
  - key: "specialized"
    value: "true"
    effect: "NoSchedule"
worker_affinity:
  required:
    - match_labels:
        zone: "edge"
```

Sensor workers can be configured with `sensor.labels` and `sensor.taints`; registration stores them under `worker.capabilities.labels` and `worker.capabilities.taints`. `SensorManager` evaluates sensor placement before starting or restarting a pack sensor process.

## Current limitations

The current UI exposes sensor log tail/follow controls but does not yet include a dedicated sensor-process health/history panel. Operators can inspect live process state through the database until a first-class API/UI surface is added.
