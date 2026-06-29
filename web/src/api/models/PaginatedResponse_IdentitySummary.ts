/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PaginationMeta } from "./PaginationMeta";
import type { Value } from "./Value";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_IdentitySummary = {
  /**
   * The page items
   */
  items: Array<{
    attributes: Value;
    display_name?: string | null;
    frozen: boolean;
    id: number;
    login: string;
    roles: Array<string>;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
