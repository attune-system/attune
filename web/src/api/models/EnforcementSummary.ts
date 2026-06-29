/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { EnforcementCondition } from "./EnforcementCondition";
import type { EnforcementStatus } from "./EnforcementStatus";
import type { i64 } from "./i64";
/**
 * Summary enforcement response for list views
 */
export type EnforcementSummary = {
  /**
   * Enforcement condition
   */
  condition: EnforcementCondition;
  trace_tag?: string | null;
  /**
   * Creation timestamp
   */
  created: string;
  event?: null | i64;
  /**
   * Enforcement ID
   */
  id: i64;
  rule?: null | i64;
  /**
   * Rule reference
   */
  rule_ref: string;
  /**
   * Enforcement status
   */
  status: EnforcementStatus;
  /**
   * Trigger reference
   */
  trigger_ref: string;
};
