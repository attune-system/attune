/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { EventSummary } from "./EventSummary";
import type { ExecutionSummary } from "./ExecutionSummary";
import type { TraceEnforcementSummary } from "./TraceEnforcementSummary";
import type { TraceWorkQueueDispatchSummary } from "./TraceWorkQueueDispatchSummary";
import type { WorkQueueItemResponse } from "./WorkQueueItemResponse";
/**
 * Standard API response wrapper
 */
export type ApiResponse_TraceReportResponse = {
  data: {
    enforcements: Array<TraceEnforcementSummary>;
    events: Array<EventSummary>;
    executions: Array<ExecutionSummary>;
    origins: Array<string>;
    queue_dispatches: Array<TraceWorkQueueDispatchSummary>;
    queue_items: Array<WorkQueueItemResponse>;
    trace_tag: string;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
