/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for updating a workflow
 */
export type UpdateWorkflowRequest = {
  /**
   * Workflow definition
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
