/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AuthorizationBasis } from "./AuthorizationBasis";
import type { DashboardSourceParamSchemaResponse } from "./DashboardSourceParamSchemaResponse";
import type { FreshnessMode } from "./FreshnessMode";
import type { SourceAvailability } from "./SourceAvailability";
import type { SourceType } from "./SourceType";
export type DashboardSourceContractResponse = {
  authorization_basis: AuthorizationBasis;
  availability: SourceAvailability;
  default_freshness_mode: FreshnessMode;
  notes?: string | null;
  ordering: Array<string>;
  param_schema: DashboardSourceParamSchemaResponse;
  response_shape: string;
  source_type: SourceType;
};
