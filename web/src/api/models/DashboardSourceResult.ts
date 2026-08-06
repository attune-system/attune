/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { DashboardSourceError } from "./DashboardSourceError";
import type { DashboardSourceMeta } from "./DashboardSourceMeta";
import type { DashboardSourceStatus } from "./DashboardSourceStatus";
export type DashboardSourceResult = {
  data: any | null;
  error?: null | DashboardSourceError;
  meta: DashboardSourceMeta;
  source_id: string;
  source_type: string;
  status: DashboardSourceStatus;
};
