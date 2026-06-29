/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { PaginationMeta } from "./PaginationMeta";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_AuditEventSummary = {
  /**
   * The page items
   */
  items: Array<{
    actor_identity?: null | i64;
    actor_login?: string | null;
    category: string;
    created: string;
    event_type: string;
    http_method?: string | null;
    http_path?: string | null;
    http_status?: number | null;
    id: i64;
    outcome: string;
    request_id?: string | null;
    resource_ref?: string | null;
    resource_type?: string | null;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
