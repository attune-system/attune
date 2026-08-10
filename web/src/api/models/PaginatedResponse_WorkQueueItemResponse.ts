/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { PaginationMeta } from "./PaginationMeta";
import type { WorkQueueItemStatus } from "./WorkQueueItemStatus";
/**
 * Paginated response wrapper
 */
export type PaginatedResponse_WorkQueueItemResponse = {
  /**
   * The page items
   */
  items: Array<{
    ack_summary: any | null;
    attempt_count: number;
    created: string;
    enqueue_source: string;
    id: i64;
    item_key?: string | null;
    last_error: any | null;
    lease_expires_at?: string | null;
    lease_token?: string | null;
    leased_execution?: null | i64;
    metadata: Record<string, any>;
    payload: Record<string, any>;
    priority: number;
    queue: i64;
    queue_ref: string;
    requested_by_enforcement?: null | i64;
    requested_by_execution?: null | i64;
    requested_by_identity?: null | i64;
    status: WorkQueueItemStatus;
    trace_tag?: string | null;
    updated: string;
  }>;
  /**
   * Pagination metadata
   */
  pagination: PaginationMeta;
};
