/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { InquiryStatus } from "./InquiryStatus";
import type { PaginationMeta } from "./PaginationMeta";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_InquirySummary = {
  /**
   * The page items
   */
  items: Array<{
    assigned_to?: null | i64;
    assigned_to_display_name?: string | null;
    assigned_to_login?: string | null;
    /**
     * Creation timestamp
     */
    created: string;
    created_by_action_ref?: string | null;
    /**
     * Execution ID that created this inquiry
     */
    created_by_execution: i64;
    created_by_pack_ref?: string | null;
    /**
     * Whether a response has been provided
     */
    has_response: boolean;
    /**
     * Inquiry ID
     */
    id: i64;
    /**
     * Prompt text
     */
    prompt: string;
    /**
     * Inquiry status
     */
    status: InquiryStatus;
    /**
     * Timeout timestamp
     */
    timeout_at?: string | null;
    workflow_action_ref?: string | null;
    workflow_execution?: null | i64;
    workflow_pack_ref?: string | null;
    workflow_root_execution?: null | i64;
    workflow_task_name?: string | null;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
