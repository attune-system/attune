/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { WorkQueueItemResponse } from "./WorkQueueItemResponse";
/**
 * Standard API response wrapper
 */
export type ApiResponse_PreviewWorkQueueItemsResponse = {
  data: {
    items: Array<WorkQueueItemResponse>;
    matched_count: number;
    preview_count: number;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
