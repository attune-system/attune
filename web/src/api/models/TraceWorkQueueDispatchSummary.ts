/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { WorkQueueDispatchStatus } from "./WorkQueueDispatchStatus";
export type TraceWorkQueueDispatchSummary = {
  created: string;
  execution: i64;
  id: i64;
  leased_item_count: number;
  queue: i64;
  queue_ref: string;
  status: WorkQueueDispatchStatus;
  updated: string;
};
