/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PaginationMeta } from "./PaginationMeta";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_RuleSummary = {
  /**
   * The page items
   */
  items: Array<{
    /**
     * Parameters to pass to the action when rule is triggered
     */
    action_params: Record<string, any>;
    /**
     * Action reference
     */
    action_ref: string;
    /**
     * Creation timestamp
     */
    created: string;
    /**
     * Rule description
     */
    description?: string | null;
    /**
     * Whether the rule is enabled
     */
    enabled: boolean;
    /**
     * Rule ID
     */
    id: number;
    /**
     * Human-readable label
     */
    label: string;
    /**
     * Pack reference
     */
    pack_ref: string;
    /**
     * Optional execution permission override. Null means inherit action default;
     * empty array means force no execution API token.
     */
    permission_set_refs?: any[] | null;
    /**
     * Unique reference identifier
     */
    ref: string;
    /**
     * Parameters for trigger configuration and event filtering
     */
    trigger_params: Record<string, any>;
    /**
     * Trigger reference
     */
    trigger_ref: string;
    /**
     * Last update timestamp
     */
    updated: string;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
