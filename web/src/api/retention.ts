import type { CancelablePromise } from "./core/CancelablePromise";
import { OpenAPI } from "./core/OpenAPI";
import { request as __request } from "./core/request";
import type { CacheRetentionConfig as GeneratedCacheRetentionConfig } from "./models/CacheRetentionConfig";
import type { NativeMaintenanceConfig as GeneratedNativeMaintenanceConfig } from "./models/NativeMaintenanceConfig";
import type { NativeMaintenanceStatus } from "./models/NativeMaintenanceStatus";

export type { NativeMaintenanceStatus };
// Rust serializes every setting on reads. Generated input types allow omitted
// fields because the API fills them from the corresponding Rust defaults.
export type NativeMaintenanceConfig =
  Required<GeneratedNativeMaintenanceConfig>;
export type CacheRetentionConfig = Required<GeneratedCacheRetentionConfig>;

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

export interface RetentionConfig {
  enabled: boolean;
  check_interval_seconds: number;
  batch_size: number;
  max_batches_per_target: number;
  dry_run: boolean;
  advisory_lock_key: number;
  targets: RetentionTargetsConfig;
  cache_retention: CacheRetentionConfig;
  native_maintenance: NativeMaintenanceConfig;
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

export const retentionTargetKeys = [
  "events",
  "enforcements",
  "executions",
  "execution_history",
  "worker_history",
  "sensor_process_history",
  "audit_events",
  "notifications",
  "webhook_event_logs",
  "inquiries",
  "work_queue_items",
  "work_queue_dispatches",
  "pack_test_executions",
  "execution_admission",
  "workers",
  "sensor_processes",
] satisfies Array<keyof RetentionTargetsConfig>;

export class RetentionService {
  public static getNativeMaintenanceStatus(): CancelablePromise<
    ApiResponse<NativeMaintenanceStatus>
  > {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/retention-config/native-status",
      errors: { 401: "Unauthorized", 403: "Insufficient permissions" },
    });
  }
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
        422: "Malformed retention configuration",
      },
    });
  }
}
