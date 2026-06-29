/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { RetentionPolicyType } from "./RetentionPolicyType";
import type { WorkerAffinity } from "./WorkerAffinity";
import type { WorkerToleration } from "./WorkerToleration";
/**
 * Request DTO for creating a new sensor
 */
export type CreateSensorRequest = {
  /**
   * Optional per-sensor retention limit override for non-log artifacts created by sensor-owned executions.
   */
  artifact_retention_limit?: number | null;
  artifact_retention_policy?: null | RetentionPolicyType;
  /**
   * Configuration values for this sensor instance (conforms to param_schema)
   */
  config?: any | null;
  /**
   * Sensor description
   */
  description?: string | null;
  /**
   * Whether the sensor is enabled
   */
  enabled?: boolean;
  /**
   * Entry point for sensor execution (e.g., path to script, function name)
   */
  entrypoint: string;
  /**
   * Human-readable label
   */
  label: string;
  /**
   * Optional per-sensor retention limit override for registered stdout/stderr log artifacts.
   */
  log_retention_limit?: number | null;
  log_retention_policy?: null | RetentionPolicyType;
  /**
   * Pack reference this sensor belongs to
   */
  pack_ref: string;
  /**
   * Parameter schema (flat format) for sensor configuration
   */
  param_schema?: any | null;
  /**
   * Unique reference identifier (e.g., "mypack.cpu_monitor")
   */
  ref: string;
  /**
   * Runtime reference for this sensor
   */
  runtime_ref: string;
  /**
   * Trigger reference this sensor monitors for
   */
  trigger_ref: string;
  /**
   * Worker label affinity and anti-affinity for this sensor process.
   */
  worker_affinity?: WorkerAffinity;
  /**
   * Worker labels required for this sensor process.
   */
  worker_selector?: Record<string, any>;
  /**
   * Worker taints tolerated by this sensor process.
   */
  worker_tolerations?: Array<WorkerToleration>;
};
