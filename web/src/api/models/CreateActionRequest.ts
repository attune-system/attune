/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
import type { RetentionPolicyType } from "./RetentionPolicyType";
import type { WorkerAffinity } from "./WorkerAffinity";
import type { WorkerToleration } from "./WorkerToleration";
/**
 * Request DTO for creating a new action
 */
export type CreateActionRequest = {
  /**
   * Hint that this action may invoke the Attune MCP server and spawn child executions.
   * When true, consumers (UI, CLI, timeline charts) render subtask views eagerly.
   */
  accesses_mcp?: boolean | null;
  /**
   * Optional per-action retention limit override for non-log artifacts created by executions.
   */
  artifact_retention_limit?: number | null;
  artifact_retention_policy?: null | RetentionPolicyType;
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
   * Whether this action is enabled. Omitted defaults to true.
   */
  enabled?: boolean | null;
  /**
   * Entry point for action execution (e.g., path to script, function name)
   */
  entrypoint: string;
  /**
   * Human-readable label
   */
  label: string;
  /**
   * Optional per-action retention limit override for stdout/stderr execution log artifacts.
   */
  log_retention_limit?: number | null;
  log_retention_policy?: null | RetentionPolicyType;
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
   * Pack refs allowed to reference this action when visibility is restricted.
   */
  reference_allowed_pack_refs?: Array<string>;
  reference_visibility?: null | ActionReferenceVisibility;
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
  /**
   * Optional default execution timeout in seconds for executions of this action.
   * When omitted, executions fall back to the app-level
   * `default_execution_timeout_seconds`.
   */
  timeout_seconds?: number | null;
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
