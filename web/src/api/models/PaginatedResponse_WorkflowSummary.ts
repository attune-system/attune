/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PaginationMeta } from "./PaginationMeta";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_WorkflowSummary = {
  /**
   * The page items
   */
  items: Array<{
    /**
     * Creation timestamp
     */
    created: string;
    /**
     * Workflow description
     */
    description?: string | null;
    /**
     * Workflow ID
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
     * Unique reference identifier
     */
    ref: string;
    /**
     * Tags
     */
    tags: Array<string>;
    /**
     * Last update timestamp
     */
    updated: string;
    /**
     * Workflow version
     */
    version: string;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
