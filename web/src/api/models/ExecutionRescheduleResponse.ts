/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ExecutionResponse } from "./ExecutionResponse";
/**
 * Response DTO for manual execution reschedule requests.
 */
export type ExecutionRescheduleResponse = {
  /**
   * Number of reschedule attempts recorded for this execution.
   */
  attempt_count: number;
  /**
   * Current execution row after republish.
   */
  execution: ExecutionResponse;
  /**
   * Timestamp for the recorded reschedule attempt.
   */
  last_attempt_at: string;
  /**
   * Human-readable status of the republish request.
   */
  message: string;
};
