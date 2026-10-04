/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PaginationMeta } from "./PaginationMeta";
import type { PermissionBindingTarget } from "./PermissionBindingTarget";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_PermissionBindingResponse = {
  /**
   * The page items
   */
  items: Array<{
    created: string;
    id: number;
    permission_set_id: number;
    permission_set_ref: string;
    target: PermissionBindingTarget;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
