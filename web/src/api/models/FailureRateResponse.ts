/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AnalyticsReadMetadata } from "./AnalyticsReadMetadata";
/**
 * Response for the execution failure rate summary.
 */
export type FailureRateResponse = {
  /**
   * Number of transitions to completed
   */
  completed_count: number;
  /**
   * Number of transitions to failed, including retry attempts
   */
  failed_count: number;
  /**
   * Failure rate as a percentage (0.0 – 100.0)
   */
  failure_rate_pct: number;
  read_coverage: AnalyticsReadMetadata;
  /**
   * Time range start
   */
  since: string;
  /**
   * Number of transitions to timeout, including retry attempts
   */
  timeout_count: number;
  /**
   * Total transitions to completed, failed, or timeout in the included hours
   */
  total_terminal: number;
  /**
   * Time range end
   */
  until: string;
};
