/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { WorkQueueItemBulkOperation } from "./WorkQueueItemBulkOperation";
import type { WorkQueueItemJsonPathSelector } from "./WorkQueueItemJsonPathSelector";
export type ApplyWorkQueueItemsRequest = {
  operation: WorkQueueItemBulkOperation;
  payload_patch: any | null;
  preview_limit?: number;
  priority?: number | null;
  selector: WorkQueueItemJsonPathSelector;
};
