/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for creating a new workflow
 */
export type CreateWorkflowRequest = {
  /**
   * Workflow definition (complete workflow YAML structure as JSON)
   */
  definition: Record<string, any>;
  /**
   * Workflow description
   */
  description?: string | null;
  /**
   * Human-readable label
   */
  label: string;
  /**
   * Output schema (flat format) defining expected outputs with inline required/secret
   */
  out_schema: Record<string, any>;
  /**
   * Pack reference this workflow belongs to
   */
  pack_ref: string;
  /**
   * Parameter schema (StackStorm-style) defining expected inputs with inline required/secret
   */
  param_schema: Record<string, any>;
  /**
   * Unique reference identifier (e.g., "core.notify_on_failure", "slack.incident_workflow")
   */
  ref: string;
  /**
   * Tags for categorization and search
   */
  tags?: any[] | null;
  /**
   * Workflow version (semantic versioning recommended)
   */
  version: string;
};
