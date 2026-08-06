/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
export type EnqueueWorkQueueItemRequest = {
  item_key?: string | null;
  metadata?: Record<string, any>;
  payload: Record<string, any>;
  priority?: number | null;
  /**
   * Optional source trace tag for this queue item.
   * When omitted for execution-token callers, inherits from the parent execution.
   */
  trace_tag?: string | null;
};
