/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Response for the execution failure rate summary.
 */
export type FailureRateResponse = {
  /**
   * Number of completed executions
   */
  completed_count: number;
  /**
   * Number of failed executions
   */
  failed_count: number;
  /**
   * Failure rate as a percentage (0.0 – 100.0)
   */
  failure_rate_pct: number;
  /**
   * Time range start
   */
  since: string;
  /**
   * Number of timed-out executions
   */
  timeout_count: number;
  /**
   * Total executions reaching a terminal state in the window
   */
  total_terminal: number;
  /**
   * Time range end
   */
  until: string;
};
