/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { WorkerAffinity } from './WorkerAffinity';
import type { WorkerToleration } from './WorkerToleration';
/**
 * Request DTO for creating a new action
 */
export type CreateActionRequest = {
    /**
     * Hint that this action may invoke the Attune MCP server and spawn child executions.
     * When true, consumers (UI, CLI, timeline charts) render subtask views eagerly.
     */
    accesses_mcp?: boolean | null;
    artifact_retention_limit?: number | null;
    artifact_retention_policy?: 'versions' | 'days' | 'hours' | 'minutes' | null;
    /**
     * Default permission set refs for execution-scoped API tokens.
     * Empty or omitted means executions of this action receive no API token by default.
     */
    default_execution_permission_set_refs?: Array<string>;
    /**
     * Action description
     */
    description?: string | null;
    /**
     * Entry point for action execution (e.g., path to script, function name)
     */
    entrypoint: string;
    /**
     * Whether the action is enabled for execution. Defaults to true when omitted.
     */
    enabled?: boolean | null;
    /**
     * Human-readable label
     */
    label: string;
    log_retention_limit?: number | null;
    log_retention_policy?: 'versions' | 'days' | 'hours' | 'minutes' | null;
    /**
     * Output schema (flat format) defining expected outputs with inline required/secret
     */
    out_schema?: any | null;
    /**
     * Pack reference this action belongs to
     */
    pack_ref: string;
    /**
     * Parameter schema (StackStorm-style) defining expected inputs with inline required/secret
     */
    param_schema?: any | null;
    /**
     * Unique reference identifier (e.g., "core.http", "aws.ec2.start_instance")
     */
    ref: string;
    /**
     * Additional worker runtime requirements keyed by runtime name/alias. Use "*" for any available version.
     */
    required_worker_runtimes?: Record<string, any>;
    /**
     * Optional runtime ID for this action
     */
    runtime?: number | null;
    /**
     * Optional runtime reference for this action
     */
    runtime_ref?: string | null;
    /**
     * Optional semver version constraint for the runtime (e.g., ">=3.12", ">=3.12,<4.0", "~18.0")
     */
    runtime_version_constraint?: string | null;
    worker_affinity?: WorkerAffinity;
    /**
     * Exact worker label requirements. All labels must match the selected worker.
     */
    worker_selector?: Record<string, any>;
    /**
     * Tolerations that allow scheduling onto workers with matching taints.
     */
    worker_tolerations?: Array<WorkerToleration>;
};
