/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { RetentionPolicyType } from './RetentionPolicyType';
import type { WorkerAffinity } from './WorkerAffinity';
import type { WorkerToleration } from './WorkerToleration';
/**
 * Response DTO for sensor information
 */
export type SensorResponse = {
    /**
     * Per-sensor retention limit override for non-log artifacts created by sensor-owned executions.
     */
    artifact_retention_limit?: number | null;
    artifact_retention_policy?: (null | RetentionPolicyType);
    /**
     * Creation timestamp
     */
    created: string;
    /**
     * Sensor description
     */
    description?: string | null;
    /**
     * Whether the sensor is enabled
     */
    enabled: boolean;
    /**
     * Entry point
     */
    entrypoint: string;
    /**
     * Sensor ID
     */
    id: number;
    /**
     * Human-readable label
     */
    label: string;
    /**
     * Per-sensor retention limit override for registered stdout/stderr log artifacts.
     */
    log_retention_limit?: number | null;
    log_retention_policy?: (null | RetentionPolicyType);
    /**
     * Pack ID (optional)
     */
    pack?: number | null;
    /**
     * Pack reference (optional)
     */
    pack_ref?: string | null;
    /**
     * Parameter schema (StackStorm-style with inline required/secret)
     */
    param_schema: any | null;
    /**
     * Unique reference identifier
     */
    ref: string;
    /**
     * Runtime ID
     */
    runtime: number;
    /**
     * Runtime reference
     */
    runtime_ref: string;
    /**
     * Last update timestamp
     */
    updated: string;
    /**
     * Worker label affinity and anti-affinity for this sensor process.
     */
    worker_affinity: WorkerAffinity;
    /**
     * Worker labels required for this sensor process.
     */
    worker_selector: Record<string, string>;
    /**
     * Worker taints tolerated by this sensor process.
     */
    worker_tolerations: Array<WorkerToleration>;
};

