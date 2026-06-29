/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { OwnerType } from "./OwnerType";
import type { PaginationMeta } from "./PaginationMeta";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_KeySummary = {
  /**
   * The page items
   */
  items: Array<{
    /**
     * Creation timestamp
     */
    created: string;
    /**
     * Whether the value is encrypted
     */
    encrypted: boolean;
    /**
     * Unique key ID
     */
    id: i64;
    /**
     * Human-readable name
     */
    name: string;
    /**
     * Owner identifier
     */
    owner?: string | null;
    /**
     * Type of owner
     */
    owner_type: OwnerType;
    /**
     * Unique reference identifier
     */
    ref: string;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
