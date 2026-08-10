/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { DashboardTimeRangeRequest } from "./DashboardTimeRangeRequest";
export type DashboardDataRequest = {
  card_ids?: any[] | null;
  filters?: Record<string, any>;
  include_meta?: boolean;
  request_id?: string | null;
  /**
   * Optional source selector.
   *
   * Membership only: request order is ignored. The response emits `sources[]`
   * in canonical `source_id` ascending order.
   */
  source_ids?: any[] | null;
  time_range?: null | DashboardTimeRangeRequest;
  time_window?: string | null;
  timezone?: string | null;
};
