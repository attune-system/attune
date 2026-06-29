/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Individual workflow sync result
 */
export type WorkflowSyncResult = {
  /**
   * Whether the workflow was created (false = updated)
   */
  created: boolean;
  /**
   * Workflow reference name
   */
  ref_name: string;
  /**
   * Any warnings during registration
   */
  warnings: Array<string>;
  /**
   * Workflow definition ID
   */
  workflow_def_id: number;
};
