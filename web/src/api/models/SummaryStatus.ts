/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { SummaryKind } from "./SummaryKind";
export type SummaryStatus = {
  coverage_hours: number;
  /**
   * Extrema only. They do not assert continuous coverage.
   */
  covered_since?: string | null;
  covered_until?: string | null;
  dirty_hours: number;
  dirty_notifications: number;
  kind: SummaryKind;
  latest_success?: string | null;
  oldest_dirty_bucket?: string | null;
  oldest_notification?: string | null;
};
