import type { CancelablePromise } from "@/api";
import type { PaginationMeta } from "@/api";
import { OpenAPI } from "@/api";
import { request as __request } from "@/api/core/request";

export type WorkerType = "local" | "remote" | "container";
export type WorkerStatus = "active" | "inactive" | "busy" | "error";
export type WorkerRole = "action" | "sensor";
export type WorkerHealthState =
  "active" | "busy" | "cordoned" | "offline" | "error" | "inactive";

export interface WorkerRuntimeSupport {
  name: string;
  versions: string[];
}

export interface WorkerLoadSnapshot {
  requested: number;
  scheduling: number;
  scheduled: number;
  running: number;
  canceling: number;
  total_active: number;
  max_concurrent_executions?: number | null;
  utilization_percent?: number | null;
  queue_depth?: number | null;
  sensor_processes_monitored?: number | null;
  sensor_processes_running?: number | null;
  active_rules?: number | null;
  max_concurrent_sensors?: number | null;
}

export interface WorkerSummary {
  id: number;
  name: string;
  worker_type: WorkerType;
  worker_role: WorkerRole;
  host?: string | null;
  port?: number | null;
  status?: WorkerStatus | null;
  last_heartbeat?: string | null;
  heartbeat_age_seconds?: number | null;
  heartbeat_stale: boolean;
  cordoned: boolean;
  cordon_reason?: string | null;
  cordoned_by?: number | null;
  cordoned_at?: string | null;
  health_state: WorkerHealthState;
  supported_runtimes: WorkerRuntimeSupport[];
  load: WorkerLoadSnapshot;
  created: string;
  updated: string;
}

export interface PaginatedResponseWorkerSummary {
  items: WorkerSummary[];
  pagination: PaginationMeta;
}

export class WorkersService {
  public static getWorker({
    id,
  }: {
    id: number;
  }): CancelablePromise<WorkerSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/workers/{id}",
      path: { id },
    });
  }

  public static listWorkers({
    page,
    pageSize,
    role,
    status,
    cordoned,
    healthState,
  }: {
    page?: number;
    pageSize?: number;
    role?: WorkerRole;
    status?: WorkerStatus;
    cordoned?: boolean;
    healthState?: WorkerHealthState;
  }): CancelablePromise<PaginatedResponseWorkerSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/workers",
      query: {
        page,
        page_size: pageSize,
        role,
        status,
        cordoned,
        health_state: healthState,
      },
    });
  }

  public static cordonWorker({
    id,
    reason,
  }: {
    id: number;
    reason?: string;
  }): CancelablePromise<WorkerSummary> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workers/{id}/cordon",
      path: { id },
      body: { reason },
      mediaType: "application/json",
    });
  }

  public static uncordonWorker({
    id,
  }: {
    id: number;
  }): CancelablePromise<WorkerSummary> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workers/{id}/uncordon",
      path: { id },
    });
  }
}
