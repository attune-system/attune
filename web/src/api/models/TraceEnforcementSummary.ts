/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { EnforcementCondition } from "./EnforcementCondition";
import type { EnforcementStatus } from "./EnforcementStatus";
import type { i64 } from "./i64";
export type TraceEnforcementSummary = {
  condition: EnforcementCondition;
  created: string;
  event?: null | i64;
  id: i64;
  resolved_at?: string | null;
  rule?: null | i64;
  rule_ref: string;
  status: EnforcementStatus;
  trigger_ref: string;
};
