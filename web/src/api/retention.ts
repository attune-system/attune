import type { CancelablePromise } from "./core/CancelablePromise";
import { OpenAPI } from "./core/OpenAPI";
import { request as __request } from "./core/request";

export interface ApiResponse<T> {
  data: T;
  message?: string | null;
}

export interface RetentionTargetConfig {
  max_age_seconds?: number | null;
}

export interface RetentionTargetsConfig {
  events: RetentionTargetConfig;
  enforcements: RetentionTargetConfig;
  executions: RetentionTargetConfig;
  execution_history: RetentionTargetConfig;
  worker_history: RetentionTargetConfig;
  sensor_process_history: RetentionTargetConfig;
  audit_events: RetentionTargetConfig;
  continuous_aggregates: RetentionTargetConfig;
  notifications: RetentionTargetConfig;
  webhook_event_logs: RetentionTargetConfig;
  inquiries: RetentionTargetConfig;
  work_queue_items: RetentionTargetConfig;
  work_queue_dispatches: RetentionTargetConfig;
  pack_test_executions: RetentionTargetConfig;
  execution_admission: RetentionTargetConfig;
  workers: RetentionTargetConfig;
  sensor_processes: RetentionTargetConfig;
}

export interface CacheRetentionConfig {
  enabled: boolean;
  batch_size: number;
  max_batches_per_generation: number;
  max_generations_per_cycle: number;
  max_namespaces_per_cycle: number;
  min_traversal_window_seconds: number;
  staging_expiry_seconds: number;
  dry_run: boolean;
  freshness_alerts_enabled: boolean;
  freshness_alert_grace_seconds: number;
  staging_failure_alert_threshold: number;
  alert_cooldown_seconds: number;
  alert_limit_per_cycle: number;
}

export interface RetentionConfig {
  enabled: boolean;
  check_interval_seconds: number;
  batch_size: number;
  dry_run: boolean;
  advisory_lock_key: number;
  targets: RetentionTargetsConfig;
  cache_retention: CacheRetentionConfig;
}

export const retentionTargetLabels: Record<
  keyof RetentionTargetsConfig,
  string
> = {
  events: "Events",
  enforcements: "Enforcements",
  executions: "Executions",
  execution_history: "Execution history",
  worker_history: "Worker history",
  sensor_process_history: "Sensor process history",
  audit_events: "Audit log",
  continuous_aggregates: "Continuous aggregates",
  notifications: "Notifications",
  webhook_event_logs: "Webhook event logs",
  inquiries: "Inquiries",
  work_queue_items: "Work queue items",
  work_queue_dispatches: "Work queue dispatches",
  pack_test_executions: "Pack test executions",
  execution_admission: "Execution admission",
  workers: "Workers",
  sensor_processes: "Sensor processes",
};

export const retentionTargetKeys = Object.keys(retentionTargetLabels) as Array<
  keyof RetentionTargetsConfig
>;

export class RetentionService {
  public static getRetentionConfig(): CancelablePromise<
    ApiResponse<RetentionConfig>
  > {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/retention-config",
      errors: {
        403: "Insufficient permissions",
      },
    });
  }

  public static updateRetentionConfig({
    requestBody,
  }: {
    requestBody: RetentionConfig;
  }): CancelablePromise<ApiResponse<RetentionConfig>> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/retention-config",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: "Invalid retention configuration",
        403: "Insufficient permissions",
      },
    });
  }
}
