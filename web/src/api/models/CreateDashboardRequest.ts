/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { DashboardScopeType } from "./DashboardScopeType";
import type { DashboardVisibility } from "./DashboardVisibility";
export type CreateDashboardRequest = {
  description?: string | null;
  enabled?: boolean | null;
  is_default_home?: boolean | null;
  label: string;
  ref: string;
  scope_ref?: string | null;
  scope_type: DashboardScopeType;
  spec: Record<string, any>;
  spec_version?: number | null;
  tags?: Array<string>;
  visibility: DashboardVisibility;
};
