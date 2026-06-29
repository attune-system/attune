/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { WorkflowSyncResult } from "./WorkflowSyncResult";
/**
 * Response for pack workflow sync operation
 */
export type PackWorkflowSyncResponse = {
  /**
   * Any errors encountered during sync
   */
  errors: Array<string>;
  /**
   * Number of workflows loaded from filesystem
   */
  loaded_count: number;
  /**
   * Pack reference
   */
  pack_ref: string;
  /**
   * Number of workflows registered/updated in database
   */
  registered_count: number;
  /**
   * Individual workflow registration results
   */
  workflows: Array<WorkflowSyncResult>;
};
