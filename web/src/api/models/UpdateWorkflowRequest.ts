/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for updating a workflow
 */
export type UpdateWorkflowRequest = {
  /**
   * Workflow definition. A task with a templated action requires a finite
   * literal allowed_action_refs array. Rendered permissions only reduce its
   * admitted per-target ceiling.
   */
  definition: Record<string, any> | null;
  /**
   * Workflow description
   */
  description?: string | null;
  /**
   * Human-readable label
   */
  label?: string | null;
  /**
   * Output schema
   */
  out_schema: Record<string, any> | null;
  /**
   * Parameter schema (StackStorm-style with inline required/secret)
   */
  param_schema: Record<string, any> | null;
  /**
   * Tags
   */
  tags?: Array<string> | null;
  /**
   * Workflow version
   */
  version?: string | null;
};
