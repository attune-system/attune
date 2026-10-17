/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ActionReferenceVisibility } from "./ActionReferenceVisibility";
/**
 * Request DTO for saving a workflow file to disk and syncing to DB
 */
export type SaveWorkflowFileRequest = {
  /**
   * The full workflow definition as JSON. Tasks with a templated action must
   * declare allowed_action_refs, an array of 1 to 256 literal action refs.
   */
  definition: Record<string, any>;
  /**
   * Workflow description
   */
  description?: string | null;
  /**
   * Whether the companion workflow action is enabled. Omitted defaults to true.
   */
  enabled?: boolean | null;
  /**
   * Human-readable label
   */
  label: string;
  /**
   * Workflow name (becomes filename: {name}.workflow.yaml)
   */
  name: string;
  /**
   * Output schema (flat format)
   */
  out_schema: Record<string, any> | null;
  /**
   * Pack reference this workflow belongs to
   */
  pack_ref: string;
  /**
   * Parameter schema (flat format with inline required/secret)
   */
  param_schema: Record<string, any> | null;
  /**
   * Pack refs allowed to reference the companion workflow action when visibility is restricted.
   */
  reference_allowed_pack_refs?: Array<string>;
  reference_visibility?: null | ActionReferenceVisibility;
  /**
   * Tags for categorization
   */
  tags?: Array<string> | null;
  /**
   * Workflow version (semantic versioning recommended)
   */
  version: string;
};
