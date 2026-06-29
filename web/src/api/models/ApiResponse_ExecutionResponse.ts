/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ExecutionStatus } from "./ExecutionStatus";
import type { RetentionPolicyType } from "./RetentionPolicyType";
/**
 * Standard API response wrapper
 */
export type ApiResponse_ExecutionResponse = {
  /**
   * Response DTO for execution information
   */
  data: {
    /**
     * Action ID (optional, may be null for ad-hoc executions)
     */
    action?: number | null;
    /**
     * Action reference
     */
    action_ref: string;
    /**
     * Retention limit override for non-log artifacts created by this execution.
     */
    artifact_retention_limit?: number | null;
    artifact_retention_policy?: null | RetentionPolicyType;
    /**
     * Execution configuration/parameters
     */
    config: Record<string, any>;
    /**
     * Creation timestamp
     */
    created: string;
    /**
     * Enforcement ID (rule enforcement that triggered this)
     */
    enforcement?: number | null;
    /**
     * Identity ID that initiated this execution
     */
    executor?: number | null;
    /**
     * Execution ID
     */
    id: number;
    /**
     * ID of the original execution if this execution is a retry.
     */
    original_execution?: number | null;
    /**
     * Parent execution ID (for nested/child executions)
     */
    parent?: number | null;
    /**
     * Permission set refs embedded in the execution-scoped API token.
     */
    permission_set_refs?: Array<string>;
    /**
     * Execution result/output
     */
    result: Record<string, any>;
    /**
     * When the execution actually started running (worker picked it up).
     * Null if the execution hasn't started running yet.
     */
    started_at?: string | null;
    /**
     * Execution status
     */
    status: ExecutionStatus;
    /**
     * Resolved execution timeout in seconds, snapshotted at creation time.
     */
    timeout_seconds?: number | null;
    /**
     * Last update timestamp
     */
    updated: string;
    /**
     * Worker ID currently assigned to this execution
     */
    worker?: number | null;
    /**
     * Worker affinity override stored on the execution, if any.
     */
    worker_affinity?: any | null;
    /**
     * Worker selector override stored on the execution, if any.
     */
    worker_selector?: any | null;
    /**
     * Worker tolerations override stored on the execution, if any.
     */
    worker_tolerations?: any[] | null;
    /**
     * Workflow task metadata (only populated for workflow task executions)
     */
    workflow_task?: any | null;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
