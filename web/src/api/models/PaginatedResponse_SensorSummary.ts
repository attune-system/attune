/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PaginationMeta } from './PaginationMeta';
import type { RetentionPolicyType } from './RetentionPolicyType';
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_SensorSummary = {
    /**
     * The page items
     */
    items: Array<{
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
         * Pack reference (optional)
         */
        pack_ref?: string | null;
        /**
         * Unique reference identifier
         */
        ref: string;
        /**
         * Last update timestamp
         */
        updated: string;
    }>;
    /**
     * Pagination metadata
     */
    pagination: PaginationMeta;
};

