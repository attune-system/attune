/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { DashboardDescriptionPatch } from "./DashboardDescriptionPatch";
import type { DashboardScopeType } from "./DashboardScopeType";
import type { DashboardVisibility } from "./DashboardVisibility";
export type UpdateDashboardRequest = {
  description?: null | DashboardDescriptionPatch;
  enabled?: boolean | null;
  expected_revision: number;
  is_default_home?: boolean | null;
  label?: string | null;
  scope_ref?: string | null;
  scope_type?: null | DashboardScopeType;
  spec?: Record<string, any> | null;
  spec_version?: number | null;
  tags?: Array<string> | null;
  visibility?: null | DashboardVisibility;
};
