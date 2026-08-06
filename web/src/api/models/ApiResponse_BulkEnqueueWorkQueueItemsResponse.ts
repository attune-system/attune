/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { WorkQueueItemResponse } from "./WorkQueueItemResponse";
/**
 * Standard API response wrapper
 */
export type ApiResponse_BulkEnqueueWorkQueueItemsResponse = {
  data: {
    created_count: number;
    items: Array<WorkQueueItemResponse>;
    updated_count: number;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
