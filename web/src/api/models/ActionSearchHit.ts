/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
/**
 * Lean search hit for action discovery — designed to minimize context bloat
 * for AI agents and humans browsing large action catalogs. Excludes ID,
 * timestamps, schemas, and runtime internals.
 */
export type ActionSearchHit = {
  /**
   * Hint that this action may invoke the Attune MCP server and spawn child executions.
   */
  accesses_mcp: boolean;
  /**
   * Action description
   */
  description?: string | null;
  /**
   * True when this action is a workflow (orchestrates child executions)
   */
  is_workflow: boolean;
  /**
   * Human-readable label
   */
  label: string;
  /**
   * Pack reference
   */
  pack_ref: string;
  /**
   * Action reference (globally unique identifier, e.g., "slack.post_message")
   */
  ref: string;
  /**
   * Pack-level visibility for references from rules, workflows, and queues.
   */
  reference_visibility: ActionReferenceVisibility;
  /**
   * Runtime reference (e.g., "core.python"). None for workflow actions.
   */
  runtime_ref?: string | null;
};
