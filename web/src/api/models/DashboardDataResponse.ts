/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { DashboardEffectiveTimeRange } from "./DashboardEffectiveTimeRange";
import type { DashboardSourceResult } from "./DashboardSourceResult";
export type DashboardDataResponse = {
  contract_version: number;
  dashboard_ref: string;
  dashboard_revision: number;
  effective_time_range: DashboardEffectiveTimeRange;
  partial: boolean;
  request_id?: string | null;
  resolved_at: string;
  /**
   * Source results in canonical `source_id` ascending order.
   */
  sources: Array<DashboardSourceResult>;
  spec_version: number;
};
