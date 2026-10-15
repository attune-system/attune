/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { LogRetentionLimitPatch } from "./LogRetentionLimitPatch";
import type { LogRetentionPolicyPatch } from "./LogRetentionPolicyPatch";
import type { WorkerAffinity } from "./WorkerAffinity";
import type { WorkerToleration } from "./WorkerToleration";
/**
 * Request DTO for updating a sensor
 */
export type UpdateSensorRequest = {
  artifact_retention_limit?: null | LogRetentionLimitPatch;
  artifact_retention_policy?: null | LogRetentionPolicyPatch;
  /**
   * Complete replacement configuration. Omit to preserve the current encrypted revision.
   */
  config?: Record<string, any> | null;
  /**
   * Sensor description
   */
  description?: string | null;
  /**
   * Whether the sensor is enabled
   */
  enabled?: boolean | null;
  /**
   * Entry point for sensor execution
   */
  entrypoint?: string | null;
  /**
   * Human-readable label
   */
  label?: string | null;
  log_retention_limit?: null | LogRetentionLimitPatch;
  log_retention_policy?: null | LogRetentionPolicyPatch;
  /**
   * Parameter schema (StackStorm-style with inline required/secret)
   */
  param_schema?: Record<string, any> | null;
  worker_affinity?: null | WorkerAffinity;
  /**
   * Worker labels required for this sensor process.
   */
  worker_selector?: Record<string, any> | null;
  /**
   * Worker taints tolerated by this sensor process.
   */
  worker_tolerations?: Array<WorkerToleration> | null;
};
