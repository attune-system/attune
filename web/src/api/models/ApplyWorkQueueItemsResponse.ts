/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { WorkQueueItemBulkOperation } from "./WorkQueueItemBulkOperation";
import type { WorkQueueItemResponse } from "./WorkQueueItemResponse";
export type ApplyWorkQueueItemsResponse = {
  affected_count: number;
  items: Array<WorkQueueItemResponse>;
  matched_count: number;
  operation: WorkQueueItemBulkOperation;
  preview_count: number;
  skipped_count: number;
};
