/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AnalyticsReadMetadata } from "./AnalyticsReadMetadata";
import type { DashboardAuthorizationMode } from "./DashboardAuthorizationMode";
import type { DashboardFreshnessMode } from "./DashboardFreshnessMode";
export type DashboardSourceMeta = {
  /**
   * End of the continuous summarized prefix of this request, if any.
   * Later covered islands are listed in read_coverage, not implied here.
   */
  aggregate_watermark?: string | null;
  authorization_mode: DashboardAuthorizationMode;
  authorized_refs: any | null;
  bucket_size?: string | null;
  cache_hit: boolean;
  freshness_mode: DashboardFreshnessMode;
  ordering: Array<string>;
  read_coverage?: null | AnalyticsReadMetadata;
  truncated: boolean;
  unit_hints: Record<string, any>;
};
