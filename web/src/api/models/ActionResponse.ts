/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { WorkerAffinity } from './WorkerAffinity';
import type { WorkerToleration } from './WorkerToleration';
/**
 * Response DTO for action information
 */
export type ActionResponse = {
    /**
     * Hint that this action may invoke the Attune MCP server and spawn child executions.
     */
    accesses_mcp: boolean;
    /**
     * Per-action retention limit override for non-log artifacts created by executions.
     */
    artifact_retention_limit?: number | null;
    /**
     * Per-action retention policy override for non-log artifacts created by executions.
     */
    artifact_retention_policy?: 'versions' | 'days' | 'hours' | 'minutes' | null;
    /**
     * Creation timestamp
     */
    created: string;
    /**
     * Default permission set refs used when executions do not explicitly override token permissions.
     */
    default_execution_permission_set_refs?: Array<string>;
    /**
     * Action description
     */
    description?: string | null;
    /**
     * Entry point
     */
    entrypoint: string;
    /**
     * Whether the action is enabled for execution
     */
    enabled: boolean;
    /**
     * Action ID
     */
    id: number;
    /**
     * Whether this is an ad-hoc action (not from pack installation)
     */
    is_adhoc: boolean;
    /**
     * Human-readable label
     */
    label: string;
    /**
     * Per-action retention limit override for stdout/stderr execution log artifacts.
     */
    log_retention_limit?: number | null;
    /**
     * Per-action retention policy override for stdout/stderr execution log artifacts.
     */
    log_retention_policy?: 'versions' | 'days' | 'hours' | 'minutes' | null;
    /**
     * Output schema
     */
    out_schema: any | null;
    /**
     * Pack ID
     */
    pack: number;
    /**
     * Pack reference
     */
    pack_ref: string;
    /**
     * Parameter schema (StackStorm-style with inline required/secret)
     */
    param_schema: any | null;
    /**
     * Unique reference identifier
     */
    ref: string;
    /**
     * Additional worker runtime requirements keyed by runtime name/alias. Use "*" for any available version.
     */
    required_worker_runtimes?: Record<string, any>;
    /**
     * Runtime ID
     */
    runtime?: number | null;
    /**
     * Runtime reference (stable identifier, e.g., "core.python")
     */
    runtime_ref?: string | null;
    /**
     * Semver version constraint for the runtime (e.g., ">=3.12", ">=3.12,<4.0", "~18.0")
     */
    runtime_version_constraint?: string | null;
    /**
     * Last update timestamp
     */
    updated: string;
    /**
     * Required/preferred worker label affinity and required anti-affinity.
     */
    worker_affinity?: WorkerAffinity;
    /**
     * Exact worker label requirements.
     */
    worker_selector?: Record<string, any>;
    /**
     * Tolerations for worker taints.
     */
    worker_tolerations?: Array<WorkerToleration>;
    /**
     * Workflow definition ID (non-null if this action is a workflow)
     */
    workflow_def?: number | null;
};
