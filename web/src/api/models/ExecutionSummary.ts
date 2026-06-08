/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ExecutionStatus } from './ExecutionStatus';
/**
 * Simplified execution response (for list endpoints)
 */
export type ExecutionSummary = {
    /**
     * Action reference
     */
    action_ref: string;
    /**
     * Creation timestamp
     */
    created: string;
    /**
     * Enforcement ID
     */
    enforcement?: number | null;
    /**
     * Execution ID
     */
    id: number;
    /**
     * ID of the original execution if this execution is a retry.
     */
    original_execution?: number | null;
    /**
     * Parent execution ID
     */
    parent?: number | null;
    /**
     * Rule reference (if triggered by a rule)
     */
    rule_ref?: string | null;
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
     * Trigger reference (if triggered by a trigger)
     */
    trigger_ref?: string | null;
    /**
     * Last update timestamp
     */
    updated: string;
    /**
     * Workflow task metadata (only populated for workflow task executions)
     */
    workflow_task?: any | null;
};

