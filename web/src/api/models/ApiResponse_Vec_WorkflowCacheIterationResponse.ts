/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { WorkflowCacheIterationState } from "./WorkflowCacheIterationState";
/**
 * Standard API response wrapper
 */
export type ApiResponse_Vec_WorkflowCacheIterationResponse = {
  data: Array<{
    batch_size: number;
    completed_at?: string | null;
    concurrency: number;
    created: string;
    dispatched_count: number;
    error_summary?: string | null;
    generation_id: number;
    namespace_id: number;
    page_size: number;
    scanned_count: number;
    state: WorkflowCacheIterationState;
    task_name: string;
    updated: string;
  }>;
  /**
   * Optional message
   */
  message?: string | null;
};
