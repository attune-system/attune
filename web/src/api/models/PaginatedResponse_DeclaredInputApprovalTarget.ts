/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { InvocationBinding } from "./InvocationBinding";
import type { PaginationMeta } from "./PaginationMeta";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_DeclaredInputApprovalTarget = {
  /**
   * The page items
   */
  items: Array<{
    action_id: i64;
    action_ref: string;
    declaration_revision: string;
    /**
     * Authored metadata only, never the evaluated input or source configuration.
     */
    expression: string;
    invocation_binding: InvocationBinding;
    metadata_revision?: null | i64;
    pack_id: i64;
    pack_ref: string;
    policy_revision: i64;
    release_id: i64;
    source_path: string;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
