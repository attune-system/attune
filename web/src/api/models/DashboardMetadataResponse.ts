/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { DashboardScopeType } from "./DashboardScopeType";
import type { DashboardVisibility } from "./DashboardVisibility";
export type DashboardMetadataResponse = {
  created: string;
  description?: string | null;
  enabled: boolean;
  id: number;
  is_adhoc: boolean;
  is_default_home: boolean;
  label: string;
  owner_identity?: number | null;
  pack?: number | null;
  ref: string;
  revision: number;
  scope_ref: string;
  scope_type: DashboardScopeType;
  spec: Record<string, any>;
  spec_version: number;
  tags: Array<string>;
  updated: string;
  visibility: DashboardVisibility;
};
