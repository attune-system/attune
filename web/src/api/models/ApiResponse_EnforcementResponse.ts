/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { EnforcementCondition } from "./EnforcementCondition";
import type { EnforcementStatus } from "./EnforcementStatus";
import type { i64 } from "./i64";
/**
 * Standard API response wrapper
 */
export type ApiResponse_EnforcementResponse = {
  /**
   * Full enforcement response with all details
   */
  data: {
    /**
     * Enforcement condition
     */
    condition: EnforcementCondition;
    /**
     * Enforcement conditions (rule evaluation criteria)
     */
    conditions: Record<string, any>;
    /**
     * Enforcement configuration
     */
    config: any | null;
    /**
     * Creation timestamp
     */
    created: string;
    event?: null | i64;
    /**
     * Enforcement ID
     */
    id: i64;
    /**
     * Enforcement payload
     */
    payload: Record<string, any>;
    /**
     * Timestamp when the enforcement was resolved (status changed from created to processed/disabled)
     */
    resolved_at?: string | null;
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
     * Trace tag associated to this enforcement via linked executions.
     */
    trace_tag?: string | null;
    /**
     * Trigger reference
     */
    trigger_ref: string;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
