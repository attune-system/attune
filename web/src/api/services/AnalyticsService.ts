/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { FailureRateResponse } from "../models/FailureRateResponse";
import type { TimeSeriesPoint } from "../models/TimeSeriesPoint";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class AnalyticsService {
  /**
   * Get a combined dashboard analytics payload.
   * Returns all key metrics in a single response to avoid multiple round-trips
   * from the dashboard page. Includes execution throughput, status transitions,
   * event volume, enforcement volume, worker status, and failure rate.
   * @returns any Dashboard analytics
   * @throws ApiError
   */
  public static getDashboardAnalytics({
    since,
    until,
    hours,
  }: {
    /**
     * Start of time range (ISO 8601). Defaults to 24 hours ago.
     */
    since?: string | null;
    /**
     * End of time range (ISO 8601). Defaults to now.
     */
    until?: string | null;
    /**
     * Number of hours to look back from now (alternative to since/until).
     * Ignored if `since` is provided.
     */
    hours?: number | null;
  }): CancelablePromise<{
    /**
     * Combined dashboard analytics response.
     *
     * Returns all key metrics in a single response for the dashboard page,
     * avoiding multiple round-trips.
     */
    data: {
      /**
       * Enforcement volume per hour
       */
      enforcement_volume: Array<TimeSeriesPoint>;
      /**
       * Event volume per hour
       */
      event_volume: Array<TimeSeriesPoint>;
      /**
       * Execution status transitions per hour
       */
      execution_status: Array<TimeSeriesPoint>;
      /**
       * Execution throughput per hour
       */
      execution_throughput: Array<TimeSeriesPoint>;
      /**
       * Execution failure rate summary
       */
      failure_rate: FailureRateResponse;
      /**
       * Time range start
       */
      since: string;
      /**
       * Time range end
       */
      until: string;
      /**
       * Worker status transitions per hour
       */
      worker_status: Array<TimeSeriesPoint>;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/analytics/dashboard",
      query: {
        since: since,
        until: until,
        hours: hours,
      },
    });
  }
  /**
   * Get enforcement volume over time.
   * Returns hourly buckets of enforcement creation counts, aggregated across all rules.
   * @returns any Enforcement volume
   * @throws ApiError
   */
  public static getEnforcementVolumeAnalytics({
    since,
    until,
    hours,
  }: {
    /**
     * Start of time range (ISO 8601). Defaults to 24 hours ago.
     */
    since?: string | null;
    /**
     * End of time range (ISO 8601). Defaults to now.
     */
    until?: string | null;
    /**
     * Number of hours to look back from now (alternative to since/until).
     * Ignored if `since` is provided.
     */
    hours?: number | null;
  }): CancelablePromise<{
    /**
     * Response for enforcement volume over time.
     */
    data: {
      /**
       * Data points: one per bucket (total enforcements created)
       */
      data: Array<TimeSeriesPoint>;
      /**
       * Time range start
       */
      since: string;
      /**
       * Time range end
       */
      until: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/analytics/enforcements/volume",
      query: {
        since: since,
        until: until,
        hours: hours,
      },
    });
  }
  /**
   * Get event volume over time.
   * Returns hourly buckets of event creation counts, aggregated across all triggers.
   * @returns any Event volume
   * @throws ApiError
   */
  public static getEventVolumeAnalytics({
    since,
    until,
    hours,
  }: {
    /**
     * Start of time range (ISO 8601). Defaults to 24 hours ago.
     */
    since?: string | null;
    /**
     * End of time range (ISO 8601). Defaults to now.
     */
    until?: string | null;
    /**
     * Number of hours to look back from now (alternative to since/until).
     * Ignored if `since` is provided.
     */
    hours?: number | null;
  }): CancelablePromise<{
    /**
     * Response for event volume over time.
     */
    data: {
      /**
       * Data points: one per bucket (total events created)
       */
      data: Array<TimeSeriesPoint>;
      /**
       * Time range start
       */
      since: string;
      /**
       * Time range end
       */
      until: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/analytics/events/volume",
      query: {
        since: since,
        until: until,
        hours: hours,
      },
    });
  }
  /**
   * Get the execution failure rate summary.
   * Returns aggregate failure/timeout/completion counts and the failure rate
   * percentage over the requested time range.
   * @returns any Failure rate summary
   * @throws ApiError
   */
  public static getFailureRateAnalytics({
    since,
    until,
    hours,
  }: {
    /**
     * Start of time range (ISO 8601). Defaults to 24 hours ago.
     */
    since?: string | null;
    /**
     * End of time range (ISO 8601). Defaults to now.
     */
    until?: string | null;
    /**
     * Number of hours to look back from now (alternative to since/until).
     * Ignored if `since` is provided.
     */
    hours?: number | null;
  }): CancelablePromise<{
    /**
     * Response for the execution failure rate summary.
     */
    data: {
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
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/analytics/executions/failure-rate",
      query: {
        since: since,
        until: until,
        hours: hours,
      },
    });
  }
  /**
   * Get execution status transitions over time.
   * Returns hourly buckets of execution status transitions (e.g., how many
   * executions moved to "completed", "failed", "running" per hour).
   * @returns any Execution status transitions
   * @throws ApiError
   */
  public static getExecutionStatusAnalytics({
    since,
    until,
    hours,
  }: {
    /**
     * Start of time range (ISO 8601). Defaults to 24 hours ago.
     */
    since?: string | null;
    /**
     * End of time range (ISO 8601). Defaults to now.
     */
    until?: string | null;
    /**
     * Number of hours to look back from now (alternative to since/until).
     * Ignored if `since` is provided.
     */
    hours?: number | null;
  }): CancelablePromise<{
    /**
     * Response for execution status transitions over time.
     */
    data: {
      /**
       * Data points: one per (bucket, status) pair
       */
      data: Array<TimeSeriesPoint>;
      /**
       * Time range start
       */
      since: string;
      /**
       * Time range end
       */
      until: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/analytics/executions/status",
      query: {
        since: since,
        until: until,
        hours: hours,
      },
    });
  }
  /**
   * Get execution throughput over time.
   * Returns hourly buckets of execution creation counts.
   * @returns any Execution throughput
   * @throws ApiError
   */
  public static getExecutionThroughputAnalytics({
    since,
    until,
    hours,
  }: {
    /**
     * Start of time range (ISO 8601). Defaults to 24 hours ago.
     */
    since?: string | null;
    /**
     * End of time range (ISO 8601). Defaults to now.
     */
    until?: string | null;
    /**
     * Number of hours to look back from now (alternative to since/until).
     * Ignored if `since` is provided.
     */
    hours?: number | null;
  }): CancelablePromise<{
    /**
     * Response for execution throughput over time.
     */
    data: {
      /**
       * Data points: one per bucket (total executions created)
       */
      data: Array<TimeSeriesPoint>;
      /**
       * Time range start
       */
      since: string;
      /**
       * Time range end
       */
      until: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/analytics/executions/throughput",
      query: {
        since: since,
        until: until,
        hours: hours,
      },
    });
  }
  /**
   * Get worker status transitions over time.
   * Returns hourly buckets of worker status changes (online/offline/draining).
   * @returns any Worker status transitions
   * @throws ApiError
   */
  public static getWorkerStatusAnalytics({
    since,
    until,
    hours,
  }: {
    /**
     * Start of time range (ISO 8601). Defaults to 24 hours ago.
     */
    since?: string | null;
    /**
     * End of time range (ISO 8601). Defaults to now.
     */
    until?: string | null;
    /**
     * Number of hours to look back from now (alternative to since/until).
     * Ignored if `since` is provided.
     */
    hours?: number | null;
  }): CancelablePromise<{
    /**
     * Response for worker status transitions over time.
     */
    data: {
      /**
       * Data points: one per (bucket, status) pair
       */
      data: Array<TimeSeriesPoint>;
      /**
       * Time range start
       */
      since: string;
      /**
       * Time range end
       */
      until: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/analytics/workers/status",
      query: {
        since: since,
        until: until,
        hours: hours,
      },
    });
  }
}
