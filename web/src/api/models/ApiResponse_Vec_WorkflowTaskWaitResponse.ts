/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { WorkflowTaskWaitKind } from "./WorkflowTaskWaitKind";
import type { WorkflowTaskWaitState } from "./WorkflowTaskWaitState";
/**
 * Standard API response wrapper
 */
export type ApiResponse_Vec_WorkflowTaskWaitResponse = {
  data: Array<{
    created: string;
    id: number;
    kind: WorkflowTaskWaitKind;
    resolved_at?: string | null;
    state: WorkflowTaskWaitState;
    target_id?: number | null;
    task_name: string;
    updated: string;
    work_queue_ref?: string | null;
  }>;
  /**
   * Optional message
   */
  message?: string | null;
};
