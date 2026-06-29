/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
import type { RetentionPolicyType } from "./RetentionPolicyType";
import type { WorkerAffinity } from "./WorkerAffinity";
import type { WorkerToleration } from "./WorkerToleration";
/**
 * Standard API response wrapper
 */
export type ApiResponse_ActionResponse = {
  /**
   * Response DTO for action information
   */
  data: {
    /**
     * Hint that this action may invoke the Attune MCP server and spawn child executions.
     */
    accesses_mcp: boolean;
    /**
     * Per-action retention limit override for non-log artifacts created by executions.
     */
    artifact_retention_limit?: number | null;
    artifact_retention_policy?: null | RetentionPolicyType;
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
     * Whether this action is enabled
     */
    enabled: boolean;
    /**
     * Entry point
     */
    entrypoint: string;
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
    log_retention_policy?: null | RetentionPolicyType;
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
     * Pack refs allowed to reference this action when visibility is restricted.
     */
    reference_allowed_pack_refs?: Array<string>;
    /**
     * Pack-level visibility for references from rules, workflows, and queues.
     */
    reference_visibility: ActionReferenceVisibility;
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
     * Default execution timeout (seconds) snapshotted onto executions of this action.
     */
    timeout_seconds?: number | null;
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
  /**
   * Optional message
   */
  message?: string | null;
};
