/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { WorkQueueItemResponse } from "./WorkQueueItemResponse";
export type BulkEnqueueWorkQueueItemsResponse = {
  created_count: number;
  items: Array<WorkQueueItemResponse>;
  updated_count: number;
};
