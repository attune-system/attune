/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AnalyticsReadRange } from "./AnalyticsReadRange";
import type { DashboardFreshnessMode } from "./DashboardFreshnessMode";
/**
 * Coverage describes only this read's source-time bounds, including ledger holes.
 */
export type AnalyticsReadMetadata = {
  mode: DashboardFreshnessMode;
  /**
   * Oldest refresh actually used. This does not guarantee global coverage.
   */
  oldest_refresh?: string | null;
  raw_ranges: Array<AnalyticsReadRange>;
  summary_ranges: Array<AnalyticsReadRange>;
};
