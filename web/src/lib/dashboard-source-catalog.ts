import type { DashboardSourceCatalogResponse, DashboardSourceContract } from "@/types/dashboard";

function kindForParam(name: string): "text" | "number" | "boolean" {
  if (["decrypt", "include_in_flight", "include_cancelled", "history"].includes(name)) {
    return "boolean";
  }
  if (["assigned_to", "sla_target_seconds", "worker_id"].includes(name)) {
    return "number";
  }
  return "text";
}

function contract(
  source_type: string,
  availability: DashboardSourceContract["availability"],
  authorization_basis: string,
  default_freshness_mode: string,
  required: string[],
  optional: string[],
  ordering: string[],
  response_shape: string,
  notes?: string,
): DashboardSourceContract {
  return {
    source_type,
    availability,
    authorization_basis,
    default_freshness_mode,
    params: [
      ...required.map((name) => ({ name, required: true, input: kindForParam(name) })),
      ...optional.map((name) => ({ name, required: false, input: kindForParam(name) })),
    ],
    ordering,
    response_shape,
    notes,
  };
}

export const DASHBOARD_SOURCE_CATALOG_FALLBACK: DashboardSourceCatalogResponse = {
  source: "fallback",
  contracts: [
    contract("key_value", "available_now", "keys", "raw_only", ["ref"], ["owner_type", "owner_ref", "decrypt"], ["ref", "name", "value", "owner_type", "owner_ref", "updated_at"], "object", "Returns one key payload; encrypted values stay null unless decrypt=true and keys:decrypt is allowed."),
    contract("latest_action_result", "available_now", "executions", "raw_only", [], ["action_ref", "pack_ref", "status"], ["action_ref", "execution_id", "status", "updated_at", "result"], "array", "Latest execution result row per action; defaults to terminal statuses."),
    contract("action_result_path", "available_now", "executions", "raw_only", ["path"], ["action_ref", "pack_ref"], ["action_ref", "execution_id", "status", "updated_at", "path", "value"], "array", "Extracts an allow-listed dot path from the latest terminal result per action."),
    contract("execution_count", "available_now", "executions", "aggregate_plus_tail", [], ["action_ref", "pack_ref", "status", "bucket_size"], ["bucket_start", "series"], "array", "Counts terminal outcomes by default semantics."),
    contract("execution_timeseries", "available_now", "executions", "aggregate_plus_tail", [], ["action_ref", "pack_ref", "status", "bucket_size"], ["bucket_start", "series"], "array", "Same semantics as execution_count."),
    contract("execution_status_breakdown", "available_now", "executions", "aggregate_plus_tail", [], ["action_ref", "pack_ref", "mode", "include_cancelled", "bucket_size"], ["bucket_start", "status"], "array", "Defaults to terminal outcome breakdown."),
    contract("execution_duration_stats", "available_now", "executions", "raw_only", [], ["action_ref", "pack_ref", "bucket_size"], ["bucket_start", "series"], "array", "Hourly terminal execution duration stats grouped by action_ref series; duration uses updated - started_at and bucket_size is currently fixed to 1h."),
    contract("last_execution", "available_now", "executions", "raw_only", [], ["action_ref", "pack_ref", "include_in_flight"], ["action_ref", "execution_id", "status", "created_at", "started_at", "updated_at", "trace_tag", "result"], "array", "Latest execution snapshot per action; include_in_flight=true widens beyond terminal statuses."),
    contract("event_count", "available_now", "events", "aggregate_plus_tail", [], ["trigger_ref", "pack_ref", "bucket_size"], ["bucket_start", "series"], "array"),
    contract("event_timeseries", "available_now", "events", "aggregate_plus_tail", [], ["trigger_ref", "pack_ref", "bucket_size"], ["bucket_start", "series"], "array"),
    contract("last_event", "available_now", "events", "raw_only", [], ["trigger_ref", "pack_ref"], ["trigger_ref", "event_id"], "array", "Latest event in the requested time range per trigger."),
    contract("enforcement_count", "available_now", "enforcements", "raw_only", [], ["rule_ref", "pack_ref", "bucket_size"], ["bucket_start", "series"], "array", "Hourly terminal enforcement counts grouped by rule."),
    contract("enforcement_timeseries", "available_now", "enforcements", "raw_only", [], ["rule_ref", "pack_ref", "bucket_size"], ["bucket_start", "series"], "array", "Hourly terminal enforcement timeseries grouped by rule."),
    contract("last_enforcement", "available_now", "enforcements", "raw_only", [], ["rule_ref", "pack_ref"], ["rule_ref", "enforcement_id"], "array", "Latest enforcement in the requested time range per rule."),
    contract("queue_backlog", "available_now", "queue_items", "raw_only", [], ["queue_ref", "pack_ref"], ["queue_ref"], "array", "Snapshot over queued/retry/leased statuses."),
    contract("queue_throughput", "available_now", "queue_items", "raw_only", [], ["queue_ref", "pack_ref"], ["bucket_start", "queue_ref"], "array", "Hourly terminal queue-item throughput grouped by queue."),
    contract("queue_dispatch_stats", "available_now", "queues", "raw_only", [], ["queue_ref", "pack_ref"], ["bucket_start", "queue_ref", "status"], "array", "Hourly terminal dispatch execution outcomes grouped by queue and status."),
    contract("inquiry_backlog", "available_now", "inquiries", "raw_only", [], ["assigned_to", "pack_ref"], ["pack_ref", "assigned_to"], "array", "Snapshot of pending inquiries grouped by pack and assignee; overdue_count uses timeout_at < now()."),
    contract("inquiry_sla", "available_now", "inquiries", "raw_only", [], ["assigned_to", "pack_ref", "sla_target_seconds", "bucket_size"], ["bucket_start", "pack_ref", "assigned_to"], "array", "Hourly inquiry SLA cohorts grouped by pack and assignee; pending inquiries use current age and bucket_size is currently fixed to 1h."),
    contract("worker_health", "available_now", "workers", "raw_only", [], ["worker_role", "status", "history", "bucket_size"], ["worker_role", "worker_id"], "array", "History mode can use aggregate_plus_tail where configured."),
    contract("sensor_health", "available_now", "sensors", "raw_only", [], ["sensor_ref", "worker_id", "window"], ["sensor_ref", "worker_id"], "array", "Latest durable sensor-process state per sensor/worker; window filters by recent updates."),
  ],
};
