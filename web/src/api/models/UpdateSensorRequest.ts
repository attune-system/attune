/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { LogRetentionLimitPatch } from './LogRetentionLimitPatch';
import type { LogRetentionPolicyPatch } from './LogRetentionPolicyPatch';
import type { WorkerAffinity } from './WorkerAffinity';
/**
 * Request DTO for updating a sensor
 */
export type UpdateSensorRequest = {
    artifact_retention_limit?: (null | LogRetentionLimitPatch);
    artifact_retention_policy?: (null | LogRetentionPolicyPatch);
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
    log_retention_limit?: (null | LogRetentionLimitPatch);
    log_retention_policy?: (null | LogRetentionPolicyPatch);
    /**
     * Parameter schema (StackStorm-style with inline required/secret)
     */
    param_schema?: any | null;
    worker_affinity?: (null | WorkerAffinity);
    /**
     * Worker labels required for this sensor process.
     */
    worker_selector?: any | null;
    /**
     * Worker taints tolerated by this sensor process.
     */
    worker_tolerations?: any[] | null;
};

