/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { DashboardAuthorizationMode } from "./DashboardAuthorizationMode";
import type { DashboardFreshnessMode } from "./DashboardFreshnessMode";
export type DashboardSourceMeta = {
  aggregate_watermark?: string | null;
  authorization_mode: DashboardAuthorizationMode;
  authorized_refs: any | null;
  bucket_size?: string | null;
  cache_hit: boolean;
  freshness_mode: DashboardFreshnessMode;
  ordering: Array<string>;
  truncated: boolean;
  unit_hints: Record<string, any>;
};
