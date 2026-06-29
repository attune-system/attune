/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Response DTO for workflow information
 */
export type WorkflowResponse = {
  /**
   * Creation timestamp
   */
  created: string;
  /**
   * Workflow definition
   */
  definition: Record<string, any>;
  /**
   * Workflow description
   */
  description?: string | null;
  /**
   * Workflow ID
   */
  id: number;
  /**
   * Human-readable label
   */
  label: string;
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
   * Tags
   */
  tags: Array<string>;
  /**
   * Last update timestamp
   */
  updated: string;
  /**
   * Workflow version
   */
  version: string;
};
