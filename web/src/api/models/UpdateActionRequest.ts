/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
import type { LogRetentionLimitPatch } from "./LogRetentionLimitPatch";
import type { LogRetentionPolicyPatch } from "./LogRetentionPolicyPatch";
import type { RuntimeVersionConstraintPatch } from "./RuntimeVersionConstraintPatch";
import type { TimeoutSecondsPatch } from "./TimeoutSecondsPatch";
import type { WorkerAffinity } from "./WorkerAffinity";
/**
 * Request DTO for updating an action
 */
export type UpdateActionRequest = {
  /**
   * Hint that this action may invoke the Attune MCP server and spawn child executions.
   */
  accesses_mcp?: boolean | null;
  artifact_retention_limit?: null | LogRetentionLimitPatch;
  artifact_retention_policy?: null | LogRetentionPolicyPatch;
  /**
   * Default permission set refs for execution-scoped API tokens.
   */
  default_execution_permission_set_refs?: any[] | null;
  /**
   * Action description
   */
  description?: string | null;
  /**
   * Whether this action is enabled.
   */
  enabled?: boolean | null;
  /**
   * Entry point for action execution
   */
  entrypoint?: string | null;
  /**
   * Human-readable label
   */
  label?: string | null;
  log_retention_limit?: null | LogRetentionLimitPatch;
  log_retention_policy?: null | LogRetentionPolicyPatch;
  /**
   * Output schema
   */
  out_schema?: any | null;
  /**
   * Parameter schema (StackStorm-style with inline required/secret)
   */
  param_schema?: any | null;
  /**
   * Replace the restricted visibility allow-list.
   */
  reference_allowed_pack_refs?: any[] | null;
  reference_visibility?: null | ActionReferenceVisibility;
  /**
   * Additional worker runtime requirements keyed by runtime name/alias. Use "*" for any available version.
   */
  required_worker_runtimes?: any | null;
  /**
   * Runtime ID
   */
  runtime?: number | null;
  /**
   * Runtime reference
   */
  runtime_ref?: string | null;
  runtime_version_constraint?: null | RuntimeVersionConstraintPatch;
  timeout_seconds?: null | TimeoutSecondsPatch;
  worker_affinity?: null | WorkerAffinity;
  /**
   * Exact worker label requirements. All labels must match the selected worker.
   */
  worker_selector?: any | null;
  /**
   * Tolerations that allow scheduling onto workers with matching taints.
   */
  worker_tolerations?: any[] | null;
};
