/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { DashboardScopeType } from "./DashboardScopeType";
import type { DashboardVisibility } from "./DashboardVisibility";
export type DashboardListItemResponse = {
  description?: string | null;
  id: number;
  is_default_home: boolean;
  label: string;
  ref: string;
  revision: number;
  scope_ref: string;
  scope_type: DashboardScopeType;
  tags: Array<string>;
  updated: string;
  visibility: DashboardVisibility;
};
