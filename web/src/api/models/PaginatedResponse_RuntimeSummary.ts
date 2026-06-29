/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PaginationMeta } from "./PaginationMeta";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_RuntimeSummary = {
  /**
   * The page items
   */
  items: Array<{
    created: string;
    description?: string | null;
    id: number;
    name: string;
    pack_ref?: string | null;
    ref: string;
    updated: string;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
