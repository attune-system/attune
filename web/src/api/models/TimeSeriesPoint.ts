/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * A single data point in an hourly time series.
 */
export type TimeSeriesPoint = {
  /**
   * Start of the 1-hour bucket (ISO 8601)
   */
  bucket: string;
  /**
   * The series label (e.g., status name, action ref). Null for aggregate totals.
   */
  label?: string | null;
  /**
   * The count value for this bucket
   */
  value: number;
};
